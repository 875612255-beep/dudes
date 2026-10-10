//! Managed directories exist owner-only before exec, even when an adapter writes no files.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use dudes_host::RealHost;
use dudes_providers::adapter::ManagedInstall;
use dudes_providers::launch::LaunchSpec;
use dudes_supervisor::host::ProcessHost;

struct Running(RealHost);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path)
        .expect("managed directory exists")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn a_file_free_install_creates_and_narrows_every_managed_directory() {
    let root = tempfile::tempdir().expect("state directory");
    let agent = root.path().join("agents/example");
    let sessions = agent.join("pi-sessions");
    let install = ManagedInstall {
        directories: vec![agent.clone(), sessions.clone()],
        files: Vec::new(),
    };
    let spec = LaunchSpec {
        argv: vec!["/bin/sleep".into(), "600".into()],
        env: Vec::new(),
        cwd: root.path().to_path_buf(),
        stdin: dudes_providers::launch::LaunchInput::Null,
    };
    let mut running = Running(RealHost::with_grace(Duration::from_millis(20)));
    common::spawned(running.0.spawn(&spec, &install), "file-free launch");
    for dir in [&agent, &sessions] {
        assert_eq!(mode(dir), 0o700);
        fs::set_permissions(dir, fs::Permissions::from_mode(0o777)).expect("widen old directory");
        assert_eq!(mode(dir), 0o777);
    }
    common::spawned(running.0.spawn(&spec, &install), "repeat file-free launch");
    for dir in [&agent, &sessions] {
        assert_eq!(mode(dir), 0o700, "{}", dir.display());
    }
}
