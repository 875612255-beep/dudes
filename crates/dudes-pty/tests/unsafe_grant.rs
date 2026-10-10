//! PTY unsafe is confined to the two native platform modules and every use explains
//! its safety contract. The workspace-wide `cargo xtask lints` gate independently checks the same
//! paths against every crate that overrides `unsafe_code`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use proc_macro2::{TokenStream, TokenTree};

const GRANTED: &[&str] = &["src/tree/unix.rs", "src/tree/windows.rs"];

#[test]
fn unsafe_is_confined_to_the_registered_platform_modules() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let allowed: BTreeSet<&str> = GRANTED.iter().copied().collect();
    let mut found = BTreeSet::new();
    for path in rust_sources(&root) {
        let relative = path
            .strip_prefix(&root)
            .expect("source under crate root")
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path).expect("read source");
        let lines = unsafe_lines(&source);
        if lines.is_empty() {
            continue;
        }
        assert!(
            allowed.contains(relative.as_str()),
            "{relative} contains unsafe at lines {lines:?} outside the registered PTY grant"
        );
        found.insert(relative);
    }
    assert_eq!(
        found,
        GRANTED.iter().map(|path| (*path).to_owned()).collect(),
        "both registered platform modules must exercise their grant"
    );
}

#[test]
fn every_unsafe_operation_has_a_nearby_safety_comment() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for relative in GRANTED {
        let source = fs::read_to_string(root.join(relative)).expect("read granted module");
        let lines: Vec<&str> = source.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if !["unsafe {", "unsafe impl"]
                .iter()
                .any(|shape| code.contains(shape))
            {
                continue;
            }
            let start = index.saturating_sub(6);
            assert!(
                lines[start..index]
                    .iter()
                    .any(|candidate| candidate.trim_start().starts_with("// SAFETY:")),
                "{relative}:{} has no nearby // SAFETY: contract",
                index + 1
            );
        }
    }
}

#[test]
fn the_scan_fires_on_each_unsafe_shape_and_ignores_prose() {
    for planted in [
        "fn f() { unsafe {} }",
        "unsafe fn f() {}",
        "unsafe impl Send for Guard {}",
        "unsafe trait Raw {}",
        "unsafe extern \"C\" { fn f(); }",
    ] {
        assert!(
            !unsafe_lines(planted).is_empty(),
            "scan did not catch {planted}"
        );
    }
    let prose = "//! unsafe impl and unsafe { in prose\n// SAFETY: words only\nfn safe() {}\n";
    assert!(unsafe_lines(prose).is_empty());
}

#[test]
fn the_crate_scan_reaches_integration_tests_outside_src() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src/tree")).expect("source dirs");
    fs::create_dir_all(temp.path().join("tests")).expect("test dir");
    fs::write(temp.path().join("src/lib.rs"), "fn safe() {}\n").expect("safe source");
    fs::write(
        temp.path().join("tests/escape.rs"),
        "unsafe { core::hint::unreachable_unchecked() }\n",
    )
    .expect("unsafe test source");
    let files = rust_sources(temp.path());
    assert!(
        files.iter().any(|path| path.ends_with("tests/escape.rs")),
        "outside-src positive control was not discovered"
    );
    let escape = fs::read_to_string(temp.path().join("tests/escape.rs")).unwrap();
    assert!(!unsafe_lines(&escape).is_empty());
}

#[test]
fn windows_enrolls_the_child_in_its_job_at_creation() {
    let source =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/tree/windows.rs"))
            .expect("read Windows PTY source");
    let compact: String = source.split_whitespace().collect();
    assert!(
        compact.contains("SpawnOptions::new().job(&job).pseudoconsole(&console)"),
        "the Windows spawn must supply both capabilities to one CreateProcess transaction"
    );
    for post_spawn in ["portable_pty", ".assign(", "spawn_suspended", ".resume("] {
        assert!(
            !source.contains(post_spawn),
            "Windows PTY source contains forbidden post-spawn route {post_spawn:?}"
        );
    }
}

#[test]
fn windows_close_proves_the_whole_job_empty_with_the_approved_query() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("read PTY manifest");
    let source = fs::read_to_string(root.join("src/tree/windows.rs")).expect("read Windows source");
    assert!(manifest.contains("\"Win32_System_JobObjects\""));
    for required in [
        "QueryInformationJobObject",
        "JobObjectBasicAccountingInformation",
        "JOBOBJECT_BASIC_ACCOUNTING_INFORMATION",
        "info.ActiveProcesses",
        "Duration::from_secs(2)",
    ] {
        assert!(
            source.contains(required),
            "missing Windows job proof {required}"
        );
    }
}

fn unsafe_lines(source: &str) -> Vec<usize> {
    let stream = TokenStream::from_str(source).expect("Rust token stream");
    let mut lines = Vec::new();
    collect_unsafe(stream, &mut lines);
    lines.sort_unstable();
    lines.dedup();
    lines
}

fn collect_unsafe(stream: TokenStream, lines: &mut Vec<usize>) {
    for token in stream {
        match token {
            TokenTree::Ident(ident) if ident == "unsafe" => lines.push(ident.span().start().line),
            TokenTree::Group(group) => collect_unsafe(group.stream(), lines),
            TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_) => {}
        }
    }
}

fn rust_sources(member_root: &Path) -> Vec<PathBuf> {
    let mut pending = vec![member_root.to_path_buf()];
    let mut sources = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).expect("read source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                if !matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some("target" | ".git")
                ) {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }
    sources.sort();
    sources
}
