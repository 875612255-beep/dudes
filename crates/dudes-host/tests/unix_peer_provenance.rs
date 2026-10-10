//! Native and source-level checks for the Unix process-ancestry relation.

#![cfg(any(target_os = "linux", target_os = "macos"))]

mod common;

use std::fs;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use dudes_host::{PeerRelation, RealHost};
#[cfg(target_os = "linux")]
use dudes_supervisor::host::ProcessHost;

#[test]
fn the_helper_entry_point() {
    common::run_if_helper();
}

#[test]
fn r4_walk_uses_full_adoption_key_and_stops_on_cycle() {
    let source = fs::read_to_string(common::src_dir().join("peer.rs")).expect("read peer.rs");
    for required in [
        "BTreeSet",
        "left.pid == right.pid",
        "left.start_time == right.start_time",
        "left.boot.as_deref() == right.boot.as_deref()",
        "!visited.insert(current)",
    ] {
        assert!(
            source.contains(required),
            "the ancestry source lost `{required}`"
        );
    }
    for forbidden in ["for _ in 0..", "take(MAX", "MAX_HOPS"] {
        assert!(
            !source.contains(forbidden),
            "the ancestry walk gained a fixed cap through `{forbidden}`"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut host = RealHost::with_grace(Duration::from_millis(200));
        let processes = host.peer_processes();
        let mark = dir.path().join("peer.pid");
        let key = common::spawned(
            host.spawn(
                &common::helper_spec(common::PARK, &mark, dir.path()),
                &common::no_install(),
            ),
            "spawn ancestry subject",
        );
        common::await_number(&mark);
        assert_eq!(
            processes.resolve(key.pid, std::slice::from_ref(&key)),
            PeerRelation::Matched(0),
            "a real Linux child matches its full start/boot key"
        );
        let mut mismatched = key.clone();
        mismatched.start_time = mismatched.start_time.wrapping_add(1);
        assert_eq!(
            processes.resolve(key.pid, &[mismatched]),
            PeerRelation::NoMatch,
            "a pid-only match is insufficient"
        );
        common::shutdown_retaining_incomplete(&mut host, "shutdown ancestry subject");
    }
}

fn call_order(source: &str) -> Result<(), &'static str> {
    let read = source
        .find("let buffered = read_process(pid);")
        .ok_or("missing buffered BSDInfo read")?;
    let validate = source
        .find("match validate_pidversion(pid, pidversion)")
        .ok_or("missing pidversion validation")?;
    if read >= validate {
        return Err("pidversion was validated before the BSDInfo row was buffered");
    }
    if source[validate..].contains("read_process(pid)") {
        return Err("the starting pid was read again after validation");
    }
    if !source[validate..].contains("Ok(true) => Read::Found(process)") {
        return Err("the buffered row is not consumed only on validation success");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn copy_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create fixture directory");
    for entry in fs::read_dir(source).expect("read fixture directory") {
        let entry = entry.expect("fixture entry");
        let source = entry.path();
        let destination = destination.join(entry.file_name());
        if source.is_dir() {
            copy_fixture(&source, &destination);
        } else {
            fs::copy(source, destination).expect("copy fixture file");
        }
    }
}

#[test]
fn macos_reads_starting_row_before_pidversion_validation() {
    let source = fs::read_to_string(common::src_dir().join("macos.rs")).expect("read macos.rs");
    call_order(&source).expect("production macOS call order");

    let swapped = source.replacen(
        "let buffered = read_process(pid);",
        "match validate_pidversion(pid, pidversion);\nlet buffered = read_process(pid);",
        1,
    );
    assert!(
        call_order(&swapped).is_err(),
        "the swapped-order mutant passed"
    );

    let reread = source.replacen(
        "Ok(true) => Read::Found(process)",
        "Ok(true) => { let _ = read_process(pid); Read::Found(process) }",
        1,
    );
    assert!(
        call_order(&reread).is_err(),
        "the post-validation-read mutant passed"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_pidversion_rejects_same_pid_after_exec_and_accepts_intact_execution() {
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/libproc")
        .canonicalize()
        .expect("vendored libproc fixture");
    let test_dir = tempfile::tempdir().expect("private test dir");
    let fixture = test_dir.path().join("libproc");
    copy_fixture(&vendor, &fixture);
    let manifest = fixture.join("Cargo.toml");
    let manifest_text = fs::read_to_string(&manifest).expect("read native fixture manifest");
    let unused_target = "[target.'cfg(any(target_os = \"linux\", target_os = \"redox\", target_os = \"android\"))'.dev-dependencies]";
    assert_eq!(
        manifest_text.matches(unused_target).count(),
        1,
        "native fixture manifest drift: expected one non-macOS dev-dependency table"
    );
    let (before, unused) = manifest_text
        .split_once(unused_target)
        .expect("non-macOS dev-dependency table");
    let (_, after) = unused
        .split_once("\n[")
        .expect("native fixture manifest drift: missing table after non-macOS dev-dependencies");
    fs::write(&manifest, format!("{before}\n[{after}"))
        .expect("write native-only fixture manifest");
    let workspace_lock = fs::read_to_string(vendor.join("../../Cargo.lock"))
        .expect("read workspace dependency versions");
    fs::write(fixture.join("Cargo.lock"), &workspace_lock).expect("seed native fixture lock");
    let target = test_dir.path().join("target");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "test",
            "--offline",
            "--manifest-path",
            manifest.to_str().expect("utf-8 manifest path"),
            "--test",
            "pidversion",
            "--",
            "--color",
            "never",
            "--test-threads",
            "1",
        ])
        .env("CARGO_TARGET_DIR", target)
        .env("CLANG_PATH", "/usr/bin/clang")
        .output()
        .expect("run vendored native pidversion controls");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stage = if stdout.contains("test pidversion_") {
        "native controls"
    } else {
        "fixture setup/build"
    };
    assert!(
        output.status.success(),
        "vendored pidversion {stage} failed with {}:\nstdout:\n{}\nstderr:\n{}\nhost:\n{}\nVM probe:\n{}",
        output.status,
        stdout,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(
            &Command::new("/usr/bin/sw_vers")
                .output()
                .expect("macOS version")
                .stdout
        ),
        String::from_utf8_lossy(
            &Command::new("/usr/sbin/sysctl")
                .args(["kern.hv_vmm_present", "hw.model"])
                .output()
                .expect("macOS VM probe")
                .stdout
        )
    );
    for control in [
        "pidversion_accepts_an_intact_execution",
        "pidversion_rejects_the_same_pid_after_exec",
        "pidversion_rejects_invalid_pid_bounds",
    ] {
        assert!(
            stdout.contains(&format!("test {control} ... ok")),
            "native pidversion control did not pass: {control}\n{stdout}"
        );
    }
    let fixture_lock = fs::read_to_string(fixture.join("Cargo.lock")).expect("native fixture lock");
    for package in fixture_lock.split("[[package]]").skip(1) {
        let identity = package
            .split("\ndependencies =")
            .next()
            .expect("native fixture package identity");
        assert!(
            workspace_lock.contains(identity.trim()),
            "native fixture dependency drifted from workspace lock:\n{identity}"
        );
    }
}
