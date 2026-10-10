//! the DACL/filesystem unsafe grant is one measured Windows module, and its three
//! mandated process/job spellings remain absent.

mod common;

use std::fs;

const GRANTED: &str = "windows.rs";
const GRANTS: &[&str] = &[
    "unsafe_code",
    "unsafe {",
    "unsafe fn",
    "unsafe impl",
    "unsafe trait",
    "unsafe extern",
];
const GRANT_SIZE: &[(&str, usize)] = &[
    ("unsafe_code", 1),
    ("unsafe {", 35),
    ("unsafe fn", 2),
    ("unsafe impl", 0),
    ("unsafe trait", 0),
    ("unsafe extern", 0),
];
const PROCESS_PRIMITIVES: &[&str] = &["TerminateJobObject", "TerminateProcess", "CreateJobObject"];

#[test]
fn the_unsafe_grant_covers_one_module_and_no_other() {
    let sources = common::rust_sources();
    assert!(sources.len() >= 2, "scanned only {} files", sources.len());
    let mut granted_files = Vec::new();
    for path in &sources {
        let text = fs::read_to_string(path).expect("read source");
        let found = common::found_in_code(&text, GRANTS);
        if found.is_empty() {
            continue;
        }
        let name = path.file_name().expect("file name").to_string_lossy();
        assert_eq!(
            name,
            GRANTED,
            "{} contains {found:?}; the unsafe grant is {GRANTED} alone",
            path.display()
        );
        granted_files.push(name.into_owned());
    }
    assert_eq!(granted_files, [GRANTED]);
}

#[test]
fn the_grant_is_exactly_the_recorded_size() {
    let windows = fs::read_to_string(common::src_dir().join(GRANTED)).expect("read windows.rs");
    let measured: Vec<_> = GRANT_SIZE
        .iter()
        .map(|(shape, _)| (*shape, common::count_in_code(&windows, shape)))
        .collect();
    assert_eq!(measured, GRANT_SIZE);
    assert_eq!(
        GRANT_SIZE
            .iter()
            .map(|(shape, _)| *shape)
            .collect::<Vec<_>>(),
        GRANTS
    );
}

#[test]
fn the_scan_catches_every_grant_shape_and_ignores_prose() {
    for (planted, expected) in [
        ("let x = unsafe { call() };", "unsafe {"),
        ("unsafe fn raw() {}", "unsafe fn"),
        ("#![allow(unsafe_code)]", "unsafe_code"),
        ("unsafe impl Send for Guard {}", "unsafe impl"),
        ("unsafe trait Raw {}", "unsafe trait"),
        ("unsafe extern \"C\" { fn raw(); }", "unsafe extern"),
    ] {
        assert!(
            common::found_in_code(planted, GRANTS).contains(&expected),
            "the scan missed {expected}"
        );
    }
    let prose = "//! unsafe fn and unsafe { in module docs\n// unsafe impl Send for Guard {}\n";
    assert!(common::found_in_code(prose, GRANTS).is_empty());
    let doubled = "let (a, b) = (unsafe { one() }, unsafe { two() });";
    assert_eq!(common::count_in_code(doubled, "unsafe {"), 2);
}

#[test]
fn the_dacl_leaf_omits_the_three_mandated_process_spellings() {
    for path in common::rust_sources() {
        let text = fs::read_to_string(&path).expect("read source");
        assert_eq!(
            common::found_in_code(&text, PROCESS_PRIMITIVES),
            Vec::<&str>::new(),
            "{} contains one of the three forbidden process/job spellings",
            path.display()
        );
    }

    let control = "unsafe { TerminateJobObject(job, 1) };";
    assert_eq!(
        common::found_in_code(control, PROCESS_PRIMITIVES),
        ["TerminateJobObject"]
    );
}
