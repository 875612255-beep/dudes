//! Windows DACL reads and creator-only filesystem-object creation.
//!
//! This is the only module in this crate allowed to contain `unsafe`, and
//! `tests/unsafe_grant.rs` pins both that location and the size of the grant. In addition, the source guard
//! requires three job/termination spellings to remain absent here, because the dependency graph
//! cannot distinguish API families inside `windows-sys`. That finite guard makes no universal
//! claim about every possible process API.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::fs::{self, File};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::io::{AsHandle as _, AsRawHandle as _, BorrowedHandle, FromRawHandle as _};
use std::path::Path;
use std::{ptr, slice};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
    GetNamedSecurityInfoW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SE_KERNEL_OBJECT,
    SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner,
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_DELETE_ON_CLOSE, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL,
    WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::core::PWSTR;

/// An access-control entry type, spelled out rather than importing the broader `SystemServices`
/// feature for one `u8` constant.
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
/// The deny counterpart.
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
/// The inheritance flags required on the runtime directory's one direct allow entry.
const CHILD_INHERITANCE: u8 = 0x03;

/// What a Windows object's discretionary access control list says.
///
/// A present empty DACL grants nobody. An absent DACL grants everyone every access, which is why
/// absence is represented separately from an empty list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dacl {
    /// No DACL at all. Windows grants every access to everyone.
    Absent,
    /// The list's entries, as SID strings, in list order.
    Present {
        /// Trustees named by access-allowed entries.
        allowed: Vec<String>,
        /// Trustees named by access-denied entries.
        denied: Vec<String>,
        /// Entries of another type. Counted so a caller can require none were skipped.
        unread: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowedEntry {
    sid: String,
    mask: u32,
    flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaclDetails {
    dacl: Dacl,
    allowed: Vec<AllowedEntry>,
    control: u16,
    owner: Option<String>,
}

/// Create `path` with a protected, inheritable DACL granting full access only to the current user,
/// or verify that an existing directory already has exactly that boundary.
///
/// Creation passes the descriptor to `CreateDirectoryW`; there is no create-then-set window. An
/// existing path is read and refused rather than silently re-ACL'd. The direct allow entry must
/// carry both object and container inheritance so files created inside the directory inherit the
/// same sole trustee.
///
/// # Errors
///
/// Whatever reading the current process token, building the descriptor, creating the directory,
/// or reading it back reports. [`io::ErrorKind::PermissionDenied`] when an existing directory is
/// not the protected, inheritable creator-only directory this function creates.
pub fn create_runtime_dir_creator_only(path: &Path) -> io::Result<()> {
    let sid = current_user_sid()?;
    let sddl = creator_only_directory_sddl(&sid);
    match create_directory_with_sddl(path, &sddl) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS.cast_signed()) => {}
        Err(error) => return Err(error),
    }

    if !fs::metadata(path)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("the runtime path {} is not a directory", path.display()),
        ));
    }

    let details = read_named_dacl(path)?;
    if !is_inheritable_creator_only(&details, &sid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the runtime directory {} does not have the protected, inheritable creator-only DACL",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Create an agent-owned directory with a protected, inheritable creator-only DACL, or narrow an
/// existing daemon-owned directory to that same boundary.
///
/// Creation supplies the descriptor to `CreateDirectoryW`, so a new directory is never wider. An
/// existing directory is opened without following a final reparse point, then receives the same
/// owner and DACL through that handle. This differs deliberately from
/// [`create_runtime_dir_creator_only`]: an operator may establish the runtime directory, while the
/// daemon is the sole writer of `agents/<AgentId>/` and re-establishes that projection on spawn.
///
/// # Errors
///
/// Whatever reading the current process token, creating or opening the directory, applying its
/// descriptor, or reading it back reports. A non-directory path is [`io::ErrorKind::InvalidData`].
pub fn create_agent_dir_creator_only(path: &Path) -> io::Result<()> {
    let sid = current_user_sid()?;
    let sddl = creator_only_directory_sddl(&sid);
    match create_directory_with_sddl(path, &sddl) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS.cast_signed()) => {
            if !fs::metadata(path)?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the agent path {} is not a directory", path.display()),
                ));
            }
            let descriptor = descriptor_from_sddl(&sddl)?;
            // SetSecurityInfo needs READ_CONTROL for this existing-directory update.
            let directory = open_path_with_descriptor(
                path,
                &descriptor,
                READ_CONTROL | WRITE_DAC | WRITE_OWNER,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            )?;
            set_creator_only_descriptor(directory.as_handle(), &descriptor)?;
        }
        Err(error) => return Err(error),
    }

    let details = read_named_dacl(path)?;
    if !is_inheritable_creator_only(&details, &sid) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the agent directory {} does not have the protected, inheritable creator-only DACL",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Create a new file whose owner and sole DACL trustee are the current user.
///
/// The owner and protected DACL are supplied to `CreateFileW` together, before the handle exists.
/// The returned [`File`] owns the credential's daemon lifetime. It is opened with write
/// and delete access, shares later reads only, and deletes its path after the last compatible
/// handle closes. Existing files are refused by `CREATE_NEW` rather than opened or replaced.
///
/// # Errors
///
/// Whatever reading the current process token, building the descriptor, or `CreateFileW` reports.
pub fn create_file_creator_only(path: &Path) -> io::Result<File> {
    let sid = current_user_sid()?;
    create_file_with_sddl_options(
        path,
        &creator_only_file_sddl(&sid),
        GENERIC_WRITE | DELETE,
        FILE_SHARE_READ,
        CREATE_NEW,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_DELETE_ON_CLOSE | FILE_FLAG_OPEN_REPARSE_POINT,
    )
}

/// Create a new file whose owner and sole DACL trustee are the current user, and which outlives
/// the returned handle.
///
/// [`create_file_creator_only`]'s owner, DACL, `CREATE_NEW` and reparse-point rule, without
/// delete-on-close: another program opens the file after this handle closes.
///
/// # Errors
///
/// Whatever reading the current process token, building the descriptor, or `CreateFileW` reports.
pub fn create_lasting_file_creator_only(path: &Path) -> io::Result<File> {
    let sid = current_user_sid()?;
    create_file_with_sddl_options(
        path,
        &creator_only_file_sddl(&sid),
        GENERIC_WRITE,
        FILE_SHARE_READ,
        CREATE_NEW,
        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
    )
}

/// Open or create a persistent settings file whose owner and sole DACL trustee are the current
/// user.
///
/// A new file receives its owner and protected DACL in `CreateFileW`, before the handle exists. The
/// same descriptor is then applied through the open handle, which narrows a file left by an older
/// version before a caller can truncate or write it. The opened object remains at offset zero;
/// opening alone does not alter file contents or last-write time.
///
/// # Errors
///
/// Whatever reading the current process token, creating or opening the file, applying its
/// descriptor, or reading it back reports. [`io::ErrorKind::PermissionDenied`] when read-back does
/// not show the exact creator-only owner and DACL.
pub fn open_file_creator_only(path: &Path) -> io::Result<File> {
    let sid = current_user_sid()?;
    let descriptor = descriptor_from_sddl(&creator_only_file_sddl(&sid))?;
    let file = open_path_with_descriptor(
        path,
        &descriptor,
        GENERIC_READ | GENERIC_WRITE | WRITE_DAC | WRITE_OWNER,
        FILE_SHARE_READ,
        OPEN_ALWAYS,
        FILE_ATTRIBUTE_NORMAL,
    )?;
    set_creator_only_descriptor(file.as_handle(), &descriptor)?;
    let details = read_file_handle_details(file.as_handle())?;
    if !details_are_creator_only(&details, &sid) || details.control & SE_DACL_PROTECTED == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "the settings file {} does not have the protected creator-only owner and DACL",
                path.display()
            ),
        ));
    }
    Ok(file)
}

/// Whether `path` is owned by the current user and has a present DACL containing exactly one allow
/// entry for that user and no deny or unread entries.
///
/// This predicate deliberately concerns the object's trustees, not directory inheritance. Runtime
/// directory creation applies the stricter inheritance check itself; a credential file's direct
/// entry need not carry child-inheritance flags.
///
/// # Errors
///
/// Whatever reading the current process token or the path's DACL reports.
pub fn is_creator_only(path: &Path) -> io::Result<bool> {
    let sid = current_user_sid()?;
    Ok(details_are_creator_only(&read_named_dacl(path)?, &sid))
}

/// Peek the pipe behind `pipe` without reading, consuming or blocking.
///
/// # Errors
///
/// Whatever `PeekNamedPipe` reports; `ERROR_BROKEN_PIPE` means the other end has closed.
pub(crate) fn peek_named_pipe(pipe: BorrowedHandle<'_>) -> io::Result<()> {
    // SAFETY: `pipe` stays live for the call. Every optional buffer and count pointer is null with
    // a zero size, which asks only whether the pipe can still be peeked.
    let peeked = unsafe {
        PeekNamedPipe(
            pipe.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if peeked == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Read the DACL of the kernel object behind `object`.
///
/// The by-handle route supports named pipes as well as files and is retained for the control-pipe
/// boundary test. Borrowing the handle opens no second pipe instance.
///
/// # Errors
///
/// Whatever `GetSecurityInfo` or walking the returned ACL reports.
pub fn read_dacl(object: BorrowedHandle<'_>) -> io::Result<Dacl> {
    read_handle_details(object).map(|details| details.dacl)
}

/// Read the owner SID of the kernel object behind `object`.
///
/// The SID is returned in the same standard string form as trustees in [`read_dacl`]. Borrowing the
/// handle opens no second object and keeps the caller's handle live throughout the read.
///
/// # Errors
///
/// Whatever `GetSecurityInfo` or converting its returned owner SID reports. A successful descriptor
/// with no owner is reported as invalid data.
pub fn read_owner(object: BorrowedHandle<'_>) -> io::Result<String> {
    read_handle_details(object)?.owner.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the Windows object has no owner SID",
        )
    })
}

fn read_handle_details(object: BorrowedHandle<'_>) -> io::Result<DaclDetails> {
    read_handle_details_for(object, SE_KERNEL_OBJECT)
}

fn read_file_handle_details(object: BorrowedHandle<'_>) -> io::Result<DaclDetails> {
    read_handle_details_for(object, SE_FILE_OBJECT)
}

fn read_handle_details_for(
    object: BorrowedHandle<'_>,
    object_type: i32,
) -> io::Result<DaclDetails> {
    let mut owner: PSID = ptr::null_mut();
    let mut acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `object` stays live for the call. The three non-null output pointers address locals of
    // the declared types; optional group and SACL outputs are null. On success the returned
    // descriptor owns both the owner SID and ACL and is released exactly once below.
    let status = unsafe {
        GetSecurityInfo(
            object.as_raw_handle(),
            object_type,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut acl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(-1),
        ));
    }
    details_from_descriptor(acl, owner, descriptor)
}

pub(crate) fn current_user_sid() -> io::Result<String> {
    let mut raw: HANDLE = ptr::null_mut();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no close; `TOKEN_QUERY` is a
    // valid mask; `raw` is an output local. On success `TokenHandle` closes the real token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = TokenHandle(raw);

    let mut needed: u32 = 0;
    // SAFETY: documented sizing call: null buffer, zero length, output size in `needed`.
    unsafe {
        GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &raw mut needed);
    }
    let mut buffer = vec![0u8; needed as usize];
    // SAFETY: the buffer is the size requested by the sizing call and outlives this call.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: the successful call filled the byte buffer with a `TOKEN_USER` followed by the SID
    // it points into. `Vec<u8>` guarantees only byte alignment, so the read is unaligned.
    let user = unsafe { ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_USER>()) };
    // SAFETY: the SID points inside `buffer`, which remains live for the call.
    let sid = unsafe { sid_string(user.User.Sid) };
    drop(token);
    sid
}

fn creator_only_directory_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;OICI;FA;;;{sid})")
}

fn creator_only_file_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;;FA;;;{sid})")
}

fn create_directory_with_sddl(path: &Path, sddl: &str) -> io::Result<()> {
    let path = wide_path(path)?;
    let descriptor = descriptor_from_sddl(sddl)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .expect("SECURITY_ATTRIBUTES fits u32"),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: both wide strings are NUL-terminated and live for the call; `attributes` points at a
    // live descriptor allocation owned by `descriptor`. CreateDirectoryW borrows them only.
    if unsafe { CreateDirectoryW(path.as_ptr(), &raw const attributes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
fn create_file_with_sddl(path: &Path, sddl: &str) -> io::Result<File> {
    create_file_with_sddl_options(
        path,
        sddl,
        GENERIC_WRITE,
        0,
        CREATE_NEW,
        FILE_ATTRIBUTE_NORMAL,
    )
}

fn create_file_with_sddl_options(
    path: &Path,
    sddl: &str,
    desired_access: u32,
    share_mode: u32,
    creation: u32,
    flags: u32,
) -> io::Result<File> {
    let descriptor = descriptor_from_sddl(sddl)?;
    open_path_with_descriptor(
        path,
        &descriptor,
        desired_access,
        share_mode,
        creation,
        flags,
    )
}

fn descriptor_from_sddl(sddl: &str) -> io::Result<LocalDescriptor> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    LocalDescriptor::from_sddl(&sddl)
}

fn open_path_with_descriptor(
    path: &Path,
    descriptor: &LocalDescriptor,
    desired_access: u32,
    share_mode: u32,
    creation: u32,
    flags: u32,
) -> io::Result<File> {
    let path = wide_path(path)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
            .expect("SECURITY_ATTRIBUTES fits u32"),
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: `path` is NUL-terminated and remains live for the call. `attributes` points at the
    // live descriptor allocation owned by `descriptor`. The access, sharing, creation and flags
    // are explicit; the call does not inherit the new handle and receives no template handle. On
    // success `File::from_raw_handle` takes the unique returned handle exactly once.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            desired_access,
            share_mode,
            &raw const attributes,
            creation,
            flags,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful `CreateFileW` returned one uniquely owned handle. Ownership moves into
    // the `File`; no guard or other path in this function closes it.
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn set_creator_only_descriptor(
    object: BorrowedHandle<'_>,
    descriptor: &LocalDescriptor,
) -> io::Result<()> {
    let mut owner: PSID = ptr::null_mut();
    let mut owner_defaulted = 0;
    // SAFETY: `descriptor` owns a valid self-relative security descriptor and both output pointers
    // address locals of the documented types. `owner` points inside `descriptor` on success.
    if unsafe { GetSecurityDescriptorOwner(descriptor.0, &raw mut owner, &raw mut owner_defaulted) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if owner.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the creator-only descriptor has no owner",
        ));
    }

    let mut dacl_present = 0;
    let mut dacl: *mut ACL = ptr::null_mut();
    let mut dacl_defaulted = 0;
    // SAFETY: `descriptor` remains live and valid; all three output pointers address locals. The
    // returned ACL points inside `descriptor` and stays live through `SetSecurityInfo` below.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor.0,
            &raw mut dacl_present,
            &raw mut dacl,
            &raw mut dacl_defaulted,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if dacl_present == 0 || dacl.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the creator-only descriptor has no DACL",
        ));
    }

    // SAFETY: `object` remains live for the call. `owner` and `dacl` point inside the live
    // descriptor allocation. Group and SACL are intentionally unchanged. The handle was opened
    // with `WRITE_OWNER | WRITE_DAC`, and the security-information mask protects the supplied DACL
    // from inheritance.
    let status = unsafe {
        SetSecurityInfo(
            object.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            owner,
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(-1),
        ));
    }
    Ok(())
}

fn read_named_dacl(path: &Path) -> io::Result<DaclDetails> {
    let path = wide_path(path)?;
    let mut owner: PSID = ptr::null_mut();
    let mut acl: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `path` is NUL-terminated and live for the call. The two non-null output pointers
    // address locals of the declared types; optional owner, group and SACL outputs are null. On
    // success the returned descriptor owns the ACL and is released exactly once below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | OWNER_SECURITY_INFORMATION,
            &raw mut owner,
            ptr::null_mut(),
            &raw mut acl,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(-1),
        ));
    }
    details_from_descriptor(acl, owner, descriptor)
}

fn details_from_descriptor(
    acl: *mut ACL,
    owner: PSID,
    descriptor: PSECURITY_DESCRIPTOR,
) -> io::Result<DaclDetails> {
    let descriptor = LocalDescriptor(descriptor);
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: `descriptor` owns a valid descriptor returned by a successful Win32 call; both
    // output pointers address locals of the documented types.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &raw mut control, &raw mut revision) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let owner = if owner.is_null() {
        None
    } else {
        // SAFETY: `owner` points inside the live descriptor allocation.
        Some(unsafe { sid_string(owner) }?)
    };
    if acl.is_null() {
        return Ok(DaclDetails {
            dacl: Dacl::Absent,
            allowed: Vec::new(),
            control,
            owner,
        });
    }
    // SAFETY: `acl` points inside `descriptor`, which remains live until this function returns.
    unsafe { entries(acl, control, owner) }
}

/// Walk an ACL's entries.
///
/// # Safety
///
/// `acl` must point at a valid `ACL` whose allocation stays live for the call.
unsafe fn entries(acl: *const ACL, control: u16, owner: Option<String>) -> io::Result<DaclDetails> {
    // SAFETY: the caller guarantees a valid live ACL.
    let count = unsafe { (*acl).AceCount };
    let mut allowed = Vec::new();
    let mut allowed_details = Vec::new();
    let mut denied = Vec::new();
    let mut unread = 0usize;
    for index in 0..u32::from(count) {
        let mut ace: *mut c_void = ptr::null_mut();
        // SAFETY: the index is below the ACL's own entry count; `ace` is an output local.
        if unsafe { GetAce(acl, index, &raw mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: every ACE begins with this header. The packed list does not promise alignment.
        let header = unsafe { ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        if header.AceType != ACCESS_ALLOWED_ACE_TYPE && header.AceType != ACCESS_DENIED_ACE_TYPE {
            unread += 1;
            continue;
        }
        // SAFETY: allow and deny entries share this layout. The SID starts at the address of the
        // inline `SidStart` field; both reads are unaligned because ACL entries are packed.
        let entry = unsafe { ptr::read_unaligned(ace.cast::<ACCESS_ALLOWED_ACE>()) };
        // SAFETY: this address is a SID inside the live ACL allocation.
        let sid = unsafe {
            sid_string(
                (&raw const (*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart)
                    .cast::<c_void>()
                    .cast_mut(),
            )
        }?;
        if header.AceType == ACCESS_ALLOWED_ACE_TYPE {
            allowed.push(sid.clone());
            allowed_details.push(AllowedEntry {
                sid,
                mask: entry.Mask,
                flags: entry.Header.AceFlags,
            });
        } else {
            denied.push(sid);
        }
    }
    Ok(DaclDetails {
        dacl: Dacl::Present {
            allowed,
            denied,
            unread,
        },
        allowed: allowed_details,
        control,
        owner,
    })
}

fn dacl_is_creator_only(dacl: &Dacl, sid: &str) -> bool {
    matches!(
        dacl,
        Dacl::Present { allowed, denied, unread }
            if allowed.as_slice() == [sid] && denied.is_empty() && *unread == 0
    )
}

fn details_are_creator_only(details: &DaclDetails, sid: &str) -> bool {
    details.owner.as_deref() == Some(sid) && dacl_is_creator_only(&details.dacl, sid)
}

fn is_inheritable_creator_only(details: &DaclDetails, sid: &str) -> bool {
    details_are_creator_only(details, sid)
        && details.control & SE_DACL_PROTECTED != 0
        && details.allowed.as_slice()
            == [AllowedEntry {
                sid: sid.to_owned(),
                mask: FILE_ALL_ACCESS,
                flags: CHILD_INHERITANCE,
            }]
}

fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a Windows path cannot contain NUL",
        ));
    }
    wide.push(0);
    Ok(wide)
}

/// One SID, in standard string form.
///
/// # Safety
///
/// `sid` must point at a valid SID whose allocation stays live for the call.
unsafe fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: PWSTR = ptr::null_mut();
    // SAFETY: `sid` is valid per the caller and `text` is an output local. `LocalString` releases
    // the allocation from a successful call exactly once.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let owned = LocalString(text);
    let mut len = 0usize;
    // SAFETY: the successful call returned a NUL-terminated string.
    while unsafe { *text.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` was measured on this same live allocation up to its terminator.
    let wide = unsafe { slice::from_raw_parts(text, len) };
    let string = String::from_utf16_lossy(wide);
    drop(owned);
    Ok(string)
}

struct LocalDescriptor(PSECURITY_DESCRIPTOR);

impl LocalDescriptor {
    fn from_sddl(sddl: &[u16]) -> io::Result<Self> {
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and live for the call; `descriptor` is an output local.
        // On success this function takes ownership of the LocalAlloc allocation.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for LocalDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this allocation came from a documented LocalAlloc-returning API and has not
            // been freed elsewhere; this type is neither Copy nor Clone.
            unsafe {
                let _ = LocalFree(self.0.cast());
            }
        }
    }
}

struct LocalString(PWSTR);

impl Drop for LocalString {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this allocation came from `ConvertSidToStringSidW` and is owned here once.
            unsafe {
                let _ = LocalFree(self.0.cast());
            }
        }
    }
}

struct TokenHandle(HANDLE);

impl Drop for TokenHandle {
    fn drop(&mut self) {
        // SAFETY: this real token handle came from `OpenProcessToken` and has not been closed.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Write as _};

    /// A pipe whose writer has exited peeks fine while bytes wait, and reports a broken pipe once
    /// they are read. Native Windows only.
    #[test]
    fn a_pipe_whose_writer_exited_peeks_broken_once_drained() {
        use crate::{PipePeek as _, pipe_peer_gone};
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/C", "echo hi"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn cmd");
        let mut stdout = child.stdout.take().expect("piped stdout");
        child.wait().expect("child exits");
        assert!(
            !pipe_peer_gone(&stdout.as_handle()),
            "control: output is waiting"
        );
        let mut drained = Vec::new();
        stdout.read_to_end(&mut drained).expect("drain");
        assert!(stdout.as_handle().peek().is_err());
        assert!(pipe_peer_gone(&stdout.as_handle()));
    }

    #[test]
    fn creator_only_builders_name_the_same_sid_as_owner_and_sole_trustee() {
        let sid = "S-1-5-21-100-200-300-400";
        assert_eq!(
            creator_only_directory_sddl(sid),
            "O:S-1-5-21-100-200-300-400D:P(A;OICI;FA;;;S-1-5-21-100-200-300-400)"
        );
        assert_eq!(
            creator_only_file_sddl(sid),
            "O:S-1-5-21-100-200-300-400D:P(A;;FA;;;S-1-5-21-100-200-300-400)"
        );
    }

    #[test]
    fn narrows_an_existing_agent_directory_and_preserves_inheritance() {
        let root = tempfile::tempdir().expect("temporary parent");
        let path = root.path().join("agent");
        let sid = current_user_sid().expect("current user SID");
        let wider = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FR;;;WD)");
        create_directory_with_sddl(&path, &wider).expect("create wider directory control");
        assert!(
            !is_creator_only(&path).expect("read wider directory"),
            "control: the existing directory is wider before the production call"
        );

        create_agent_dir_creator_only(&path).expect("narrow agent directory");

        let details = read_named_dacl(&path).expect("read narrowed directory");
        assert!(is_inheritable_creator_only(&details, &sid), "{details:?}");
    }

    #[test]
    fn opens_an_existing_settings_file_without_truncating_and_narrows_it() {
        let root = tempfile::tempdir().expect("temporary parent");
        let path = root.path().join("settings.json");
        let sid = current_user_sid().expect("current user SID");
        let wider = format!("O:{sid}D:P(A;;FA;;;{sid})(A;;FR;;;WD)");
        let mut original = create_file_with_sddl(&path, &wider).expect("create wider file control");
        original
            .write_all(b"same contents")
            .expect("write wider file control");
        drop(original);
        assert!(
            !is_creator_only(&path).expect("read wider settings file"),
            "control: the existing file is wider before the production call"
        );

        let mut file = open_file_creator_only(&path).expect("narrow settings file");
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .expect("opening did not make the file unreadable");

        assert_eq!(contents, "same contents", "opening must not truncate");
        assert!(is_creator_only(&path).expect("read narrowed settings file"));
    }

    #[test]
    fn a_lasting_creator_only_file_outlives_its_handle_and_refuses_a_second_create() {
        let root = tempfile::tempdir().expect("temporary directory");
        let path = root.path().join("floor-open-launch.html");
        let mut file = create_lasting_file_creator_only(&path).expect("create launch file");
        file.write_all(b"launch").expect("write launch file");
        drop(file);

        assert!(is_creator_only(&path).expect("read path security"));
        assert_eq!(
            std::fs::read(&path).expect("the file outlives its handle"),
            b"launch"
        );
        create_lasting_file_creator_only(&path).expect_err("CREATE_NEW must refuse replacement");
        std::fs::remove_file(&path).expect("the owner removes it");
    }

    #[test]
    fn a_correct_dacl_with_the_wrong_owner_is_not_creator_only() {
        let sid = "S-1-5-21-100-200-300-400";
        let mut details = DaclDetails {
            dacl: Dacl::Present {
                allowed: vec![sid.to_owned()],
                denied: Vec::new(),
                unread: 0,
            },
            allowed: vec![AllowedEntry {
                sid: sid.to_owned(),
                mask: FILE_ALL_ACCESS,
                flags: 0,
            }],
            control: SE_DACL_PROTECTED,
            owner: Some("S-1-5-21-999-888-777-666".to_owned()),
        };
        assert!(
            dacl_is_creator_only(&details.dacl, sid),
            "the control must isolate owner equality from the DACL contents"
        );
        assert!(!details_are_creator_only(&details, sid));
        details.owner = Some(sid.to_owned());
        assert!(details_are_creator_only(&details, sid));
    }

    #[test]
    fn creator_only_file_shares_read_refuses_mutation_and_deletes_on_last_close() {
        let root = tempfile::tempdir().expect("ordinary temporary parent");
        let path = root.path().join("operator.cred");
        let sid = current_user_sid().expect("current user SID");
        let mut file = create_file_creator_only(&path).expect("create protected file");
        file.write_all(b"first").expect("write file");
        file.sync_all().expect("sync file");

        assert_eq!(read_owner(file.as_handle()).expect("read owner"), sid);
        assert_eq!(
            read_dacl(file.as_handle()).expect("read DACL"),
            Dacl::Present {
                allowed: vec![sid],
                denied: Vec::new(),
                unread: 0,
            }
        );
        assert!(is_creator_only(&path).expect("read path security"));

        let reader = File::open(&path).expect("the compatible std reader opens the live file");
        create_file_creator_only(&path).expect_err("CREATE_NEW must refuse replacement");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect_err("the server handle shares no later write access");
        std::fs::remove_file(&path).expect_err("the server handle shares no later delete access");
        let moved = root.path().join("moved.cred");
        std::fs::rename(&path, &moved)
            .expect_err("the server handle shares no later rename access");

        drop(file);
        create_file_creator_only(&path)
            .expect_err("the held reader keeps deletion pending and restart refused");
        let mut contents = Vec::new();
        (&reader)
            .read_to_end(&mut contents)
            .expect("read original file");
        assert_eq!(contents, b"first");
        drop(reader);
        assert!(
            !path.exists(),
            "last compatible close did not delete the file"
        );

        let retry = create_file_creator_only(&path).expect("same-path retry after last close");
        drop(retry);
        assert!(
            !path.exists(),
            "the retry did not own its delete-on-close lifetime"
        );
    }

    #[test]
    fn null_attributes_inherit_a_broad_parent_and_are_not_creator_only() {
        let root = tempfile::tempdir().expect("temporary parent");
        let broad = root.path().join("broad");
        let sid = current_user_sid().expect("current user SID");
        let wider = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;WD)");
        create_directory_with_sddl(&broad, &wider).expect("create broad parent");
        let path = broad.join("operator.cred");
        let path_wide = wide_path(&path).expect("encode path");

        // SAFETY: `path_wide` is NUL-terminated and stays live for the call. This negative control
        // deliberately supplies null security attributes; it still takes unique ownership of the
        // successful `CREATE_NEW` handle exactly once.
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_WRITE,
                0,
                ptr::null(),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        assert_ne!(
            handle, INVALID_HANDLE_VALUE,
            "create null-attributes control"
        );
        // SAFETY: the successful call returned one uniquely owned handle, transferred once.
        let file = unsafe { File::from_raw_handle(handle) };
        assert_ne!(
            read_dacl(file.as_handle()).expect("read inherited broad DACL"),
            Dacl::Present {
                allowed: vec![sid],
                denied: Vec::new(),
                unread: 0,
            }
        );
        assert!(!is_creator_only(&path).expect("check inherited broad DACL"));
    }

    #[test]
    fn a_descriptor_without_an_explicit_dacl_is_not_creator_only() {
        let root = tempfile::tempdir().expect("ordinary temporary parent");
        let path = root.path().join("operator.cred");
        let sid = current_user_sid().expect("current user SID");
        let file =
            create_file_with_sddl(&path, &format!("O:{sid}")).expect("create absent-DACL control");

        assert_eq!(read_owner(file.as_handle()).expect("read owner"), sid);
        assert_ne!(
            read_dacl(file.as_handle()).expect("read resulting DACL"),
            Dacl::Present {
                allowed: vec![sid],
                denied: Vec::new(),
                unread: 0,
            }
        );
        assert!(!is_creator_only(&path).expect("check absent DACL"));
    }

    #[test]
    fn a_null_dacl_is_observed_as_absent_and_refused() {
        use windows_sys::Win32::Security::{
            InitializeSecurityDescriptor, SECURITY_DESCRIPTOR, SetSecurityDescriptorDacl,
        };

        let root = tempfile::tempdir().expect("ordinary temporary parent");
        let path = root.path().join("null-dacl.cred");
        let path_wide = wide_path(&path).expect("encode path");
        let mut descriptor = SECURITY_DESCRIPTOR::default();
        let descriptor_ptr = (&raw mut descriptor).cast();
        assert_ne!(
            // SAFETY: `descriptor_ptr` is non-null, aligned, and writable for the full
            // `SECURITY_DESCRIPTOR` local, which remains live for the call. Revision 1 is
            // `SECURITY_DESCRIPTOR_REVISION`.
            unsafe { InitializeSecurityDescriptor(descriptor_ptr, 1) },
            0,
            "initialize absolute security descriptor: {}",
            io::Error::last_os_error()
        );
        assert_ne!(
            // SAFETY: `descriptor_ptr` still points to the live absolute descriptor initialized
            // above. Win32 permits a null ACL pointer when `dacl_present` is true; that exact
            // combination installs the full-access null DACL this control requires.
            unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, ptr::null(), 0) },
            0,
            "set present null DACL: {}",
            io::Error::last_os_error()
        );
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("SECURITY_ATTRIBUTES fits u32"),
            lpSecurityDescriptor: descriptor_ptr,
            bInheritHandle: 0,
        };
        // SAFETY: the path is NUL-terminated, and `attributes` points to the initialized absolute
        // descriptor that stays live for the call. The new handle is neither shared nor inherited.
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                GENERIC_WRITE,
                0,
                &raw const attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        assert_ne!(handle, INVALID_HANDLE_VALUE, "create null-DACL control");
        // SAFETY: successful `CreateFileW` returned one uniquely owned handle, transferred once.
        let file = unsafe { File::from_raw_handle(handle) };

        assert_eq!(
            read_dacl(file.as_handle()).expect("read null DACL"),
            Dacl::Absent
        );
        assert!(!is_creator_only(&path).expect("check null DACL"));
    }

    #[test]
    fn creates_and_rechecks_an_inheritable_creator_only_directory() {
        let root = tempfile::tempdir().expect("temporary parent");
        let runtime = root.path().join("runtime");

        create_runtime_dir_creator_only(&runtime).expect("create protected runtime directory");
        create_runtime_dir_creator_only(&runtime).expect("recheck protected runtime directory");

        let sid = current_user_sid().expect("current user SID");
        let details = read_named_dacl(&runtime).expect("read runtime DACL");
        assert!(is_inheritable_creator_only(&details, &sid), "{details:?}");
        assert!(is_creator_only(&runtime).expect("check runtime trustees"));
    }

    #[test]
    fn a_non_inheritable_creator_only_directory_is_refused() {
        let root = tempfile::tempdir().expect("temporary parent");
        let runtime = root.path().join("runtime");
        let sid = current_user_sid().expect("current user SID");
        create_directory_with_sddl(&runtime, &creator_only_file_sddl(&sid))
            .expect("create non-inheritable control");

        assert!(
            is_creator_only(&runtime).expect("read trustees"),
            "the directory control must isolate inheritance from trustee contents"
        );
        let error = create_runtime_dir_creator_only(&runtime)
            .expect_err("a non-inheritable runtime directory must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn a_wider_existing_directory_is_refused_without_replacing_its_dacl() {
        let root = tempfile::tempdir().expect("temporary parent");
        let runtime = root.path().join("runtime");
        let sid = current_user_sid().expect("current user SID");
        let wider = format!("D:P(A;OICI;FA;;;{sid})(A;OICI;FR;;;WD)");
        create_directory_with_sddl(&runtime, &wider).expect("create wider control");
        let before = read_named_dacl(&runtime).expect("read wider DACL");

        let error = create_runtime_dir_creator_only(&runtime)
            .expect_err("a wider runtime directory must be refused");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            read_named_dacl(&runtime).expect("read refused DACL again"),
            before,
            "refusal must not replace a DACL this process did not create"
        );
    }

    #[test]
    fn a_directory_inheriting_a_broad_parent_is_not_creator_only() {
        let root = tempfile::tempdir().expect("temporary parent");
        let broad = root.path().join("broad");
        create_directory_with_sddl(&broad, "D:P(A;OICI;FA;;;WD)").expect("create broad parent");
        let inherited = broad.join("inherited");
        fs::create_dir(&inherited).expect("create with default security attributes");

        assert!(
            !is_creator_only(&inherited).expect("read inherited DACL"),
            "null security attributes must not pass the creator-only assertion"
        );
    }
}
