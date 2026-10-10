//! One endpoint name, resolved per platform.
//!
//! The daemon binds what [`local_endpoint`] returns and the client connects to what
//! [`local_endpoint`] returns. Nothing carries the value between the two processes, so the whole
//! contract is that the two calls agree given the same runtime directory and socket name.
//!
//! # Why the return type is a `PathBuf` on both platforms
//!
//! On Windows the value is a pipe name — `\\.\pipe\…` — and **not a filesystem path**, even though
//! it is carried in a `PathBuf`. That is a deliberate churn-minimising call rather than a claim
//! about the value: `DUDES_SOCK`, `Env::socket()`, `duded::settings::socket()` and the
//! `socket_name` setting all hold a `PathBuf` today, and re-typing that chain buys nothing. The hazard it leaves — code that
//! treats the endpoint as a file, `stat`s it or removes it — is contained where it already was: `bins/duded/src/socket.rs`'s `reclaim_stale`
//! and `SocketIdentity` are `#[cfg(unix)]`, and `boot.rs` carries `#[cfg(not(unix))]` no-op arms
//! beside them.
//!
//! # What is *not* resolved here
//!
//! The path is never canonicalised. Canonicalising is I/O and requires the directory to exist,
//! and the client resolves its endpoint before any daemon has created anything — so a call that
//! canonicalised would fail in the one process that most needs an answer. The consequence is
//! worth stating because it differs by platform: on Unix two spellings of one directory join to
//! two `PathBuf`s that nonetheless `bind` and `connect` to the same socket, because the kernel
//! resolves them; on Windows two spellings hash to two *different pipes*, and nothing resolves
//! them together. What keeps that from being two daemons serving one runtime directory is the
//! single-writer state lock, which is taken on the directory rather than on the door.

use std::path::{Path, PathBuf};

/// The Windows named-pipe namespace prefix, and every byte of it counts against the 256-character
/// limit a pipe name has.
#[cfg(any(windows, test))]
const PIPE_PREFIX: &str = r"\\.\pipe\";

/// What the derived part of a pipe name is prefixed with, so a name in the machine-wide namespace
/// says whose it is when an operator lists it.
#[cfg(any(windows, test))]
const PIPE_OWNER: &str = "dudes.";

/// The whole of a pipe name, prefix included, may be no longer than this.
///
/// Windows enforces it at `CreateNamedPipeW`, not at construction, so an over-long name is a
/// runtime failure on a platform no runner reaches yet — which is why the derivation is
/// fixed-length by construction, the assertion below holds that length under this limit at
/// compile time, and a test measures the derived value itself.
#[cfg(any(windows, test))]
const MAX_PIPE_NAME: usize = 256;

// The derived name is the prefix, the owner tag and a 64-character hex digest, so its length is a
// compile-time fact: a change to either constant that pushed a name over the limit fails to build
// on every platform rather than at `CreateNamedPipeW` on the one no runner reaches. This is also
// what makes `MAX_PIPE_NAME` load-bearing in a Windows build, where no test reads it.
#[cfg(any(windows, test))]
const _: () = assert!(PIPE_PREFIX.len() + PIPE_OWNER.len() + 64 <= MAX_PIPE_NAME);

/// The endpoint both ends of the control transport use for `socket_name` inside `runtime_dir`.
///
/// On Unix this is the socket path it has always been: the name joined onto the directory. On
/// Windows it is a name in the machine-wide pipe namespace, derived from both arguments, because
/// there is no directory in that namespace to keep two daemons apart.
///
/// **Pure, and total.** No I/O, no clock, no failure: a client must be able to name the door
/// before anything has created it, and a function that could fail here would have to be handled at
/// every call site to say nothing more than "the arguments were the arguments".
#[must_use]
pub fn local_endpoint(runtime_dir: impl AsRef<Path>, socket_name: impl AsRef<Path>) -> PathBuf {
    #[cfg(unix)]
    {
        runtime_dir.as_ref().join(socket_name.as_ref())
    }
    #[cfg(windows)]
    {
        PathBuf::from(pipe_name(runtime_dir.as_ref(), socket_name.as_ref()))
    }
}

/// The Windows pipe name for `runtime_dir` and `socket_name`.
///
/// Compiled on every platform under `cfg(test)` deliberately: the derivation is a pure function of
/// its two arguments, so every property that matters about it — determinism, distinctness, length,
/// character set — is settled by a unit test on any developer machine, years before a Windows
/// runner executes a single byte of it. Only the *selection* of this branch is behind `cfg`.
///
/// # Three properties, each of which a plausible alternative loses
///
/// - **Fixed length.** The digest is 64 characters whatever the inputs were, so the whole name is
///   79 — held under the 256-character limit by the `const` assertion beside `MAX_PIPE_NAME`, not
///   counted by eye — and a deep runtime directory cannot reach it. Encoding the path itself would
///   be readable and would fail on a long path, at `CreateNamedPipeW`, on the one platform nothing
///   here can run on.
/// - **Lower-case hex, not base64.** The pipe namespace is **case-insensitive**, so two base64
///   digests differing only in the case of one character name one pipe. Hex is already
///   case-folded: distinct names differ somewhere other than in case, by construction.
/// - **Length-prefixed inputs.** Concatenating the two arguments would give `("ab", "c")` and
///   `("a", "bc")` one digest. Prefixing each with its length makes the encoding unambiguous
///   whatever bytes a path holds, which a separator byte does only until a path contains it.
#[cfg(any(windows, test))]
fn pipe_name(runtime_dir: &Path, socket_name: &Path) -> String {
    let dir = runtime_dir.as_os_str().as_encoded_bytes();
    let name = socket_name.as_os_str().as_encoded_bytes();
    let mut hasher = blake3::Hasher::new();
    for part in [dir, name] {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    format!("{PIPE_PREFIX}{PIPE_OWNER}{}", digest.to_hex())
}

#[cfg(test)]
mod tests {
    use super::{MAX_PIPE_NAME, PIPE_OWNER, PIPE_PREFIX, pipe_name};
    use std::path::Path;

    /// The derived part, with the namespace prefix taken off.
    fn derived(dir: &str, name: &str) -> String {
        let full = pipe_name(Path::new(dir), Path::new(name));
        full.strip_prefix(PIPE_PREFIX)
            .expect("every derived name is in the pipe namespace")
            .to_owned()
    }

    /// The prefix is what the 256-character budget is spent against, so its length is pinned here
    /// rather than counted by eye in a doc comment.
    #[test]
    fn the_namespace_prefix_is_nine_characters() {
        assert_eq!(PIPE_PREFIX.len(), 9, r"the prefix is `\\.\pipe\`");
    }

    /// A name is the same on the second call. Trivially true of a hash and worth an assertion
    /// anyway: this is the property the two processes depend on and the only one whose failure is
    /// invisible on Unix.
    #[test]
    fn the_derivation_is_deterministic() {
        assert_eq!(
            pipe_name(Path::new(r"C:\dudes"), Path::new("duded.sock")),
            pipe_name(Path::new(r"C:\dudes"), Path::new("duded.sock"))
        );
    }

    /// Two runtime directories, two names.
    #[test]
    fn two_runtime_directories_derive_different_names() {
        assert_ne!(
            derived(r"C:\dudes\a", "duded.sock"),
            derived(r"C:\dudes\b", "duded.sock")
        );
    }

    /// Two socket names, two names.
    #[test]
    fn two_socket_names_derive_different_names() {
        assert_ne!(
            derived(r"C:\dudes", "duded.sock"),
            derived(r"C:\dudes", "other.sock")
        );
    }

    /// The length prefixes do work, and this is the case that shows it.
    ///
    /// Without them the hasher would see `ab` then `c` and `a` then `bc` as the same byte stream,
    /// and two runtime directories would share a door. The assertion is red against a derivation
    /// that concatenates, and green against nothing else — a separator byte would also pass, which
    /// is why the doc says what a separator costs rather than claiming this test rules it out.
    #[test]
    fn a_boundary_moved_between_the_two_arguments_changes_the_name() {
        assert_ne!(derived("ab", "c"), derived("a", "bc"));
    }

    /// The whole name fits, whatever the runtime directory was.
    ///
    /// The input here is far past any real path — 4000 characters — because the property being
    /// asserted is that input length does not reach the output at all. A derivation that encoded
    /// the path rather than hashing it passes at 30 characters and fails here.
    #[test]
    fn an_absurd_runtime_directory_still_fits_the_pipe_name_limit() {
        let deep = format!(r"C:\{}", "verylongdirectorycomponent\\".repeat(150));
        let full = pipe_name(Path::new(&deep), Path::new("duded.sock"));
        assert!(
            full.len() <= MAX_PIPE_NAME,
            "a pipe name is capped at {MAX_PIPE_NAME} characters including the prefix; this one \
             is {} from an input of {}",
            full.len(),
            deep.len()
        );
        assert_eq!(
            full.len(),
            PIPE_PREFIX.len() + PIPE_OWNER.len() + 64,
            "the derivation is fixed-length: prefix, owner tag and a 64-character digest"
        );
    }

    /// No backslash after the prefix.
    ///
    /// `\` is the one character a pipe name may not contain, and it is also the character every
    /// path on that platform is full of — so a derivation that leaked any part of its input would
    /// fail here rather than at `CreateNamedPipeW` on a runner that does not exist.
    #[test]
    fn the_derived_part_contains_no_backslash() {
        let deep = derived(
            r"C:\Users\someone\AppData\Local\dudes\runtime",
            "duded.sock",
        );
        assert!(
            !deep.contains('\\'),
            "`\\` is the only character a pipe name may not contain, and the derived part has one: \
             {deep}"
        );
    }

    /// Lower-case hex, and the reason is the case-insensitive namespace.
    ///
    /// Two assertions rather than one: the character set, and that the name is already its own
    /// lower-case form. The second is the one that matters — it is what makes "two distinct
    /// derived names are two distinct pipes" true in a namespace that folds case, and it is red
    /// against base64 while the first alone would be red against base64 too but for the wrong
    /// reason.
    #[test]
    fn the_derived_part_is_lower_case_hex_because_the_namespace_folds_case() {
        let name = derived(r"C:\dudes", "duded.sock");
        let digest = name
            .strip_prefix(PIPE_OWNER)
            .expect("the derived part carries the owner tag");
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "the digest must be lower-case hex, got {digest}"
        );
        assert_eq!(
            name,
            name.to_ascii_lowercase(),
            "the pipe namespace is case-insensitive, so a name that is not already case-folded \
             can collide with a different name"
        );
    }

    /// The control for the test above: the scan it uses can fail.
    ///
    /// A charset assertion that never rejects anything is a comment. This plants the two encodings
    /// the doc comment rules out — an upper-case hex digest and a base64 one — and shows each is
    /// caught by the clause that is supposed to catch it.
    #[test]
    fn the_charset_scan_rejects_the_encodings_the_derivation_refuses() {
        let upper = "ABCDEF0123";
        assert!(
            !upper
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "control: an upper-case hex digest must fail the charset clause"
        );
        let base64 = "aGVsbG8gd29ybGQ";
        assert_ne!(
            base64,
            base64.to_ascii_lowercase(),
            "control: a base64 digest must fail the case-folding clause, which is the clause that \
             makes the case-insensitive namespace safe"
        );
    }
}
