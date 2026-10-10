//! `duded --version` prints the daemon's version and exits cleanly, without starting a daemon.

#[test]
#[allow(
    clippy::disallowed_types,
    reason = "the test runs the built daemon binary; host_seam.rs separately guards src/"
)]
fn version_prints_the_workspace_version_and_succeeds() {
    for flag in ["--version", "-V"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_duded"))
            .arg(flag)
            .output()
            .expect("duded runs");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("duded {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert!(output.stderr.is_empty(), "{output:?}");
    }
}
