use std::process::Command;

use anyhow::{Context, Result, ensure};

use crate::workspace::Workspace;

pub(crate) fn run(workspace: &Workspace) -> Result<()> {
    ensure!(
        !cfg!(windows),
        "smoke requires the macOS or Linux installer"
    );
    let executable = std::env::current_exe().context("finding xtask")?;
    let status = Command::new("node")
        .arg("xtask/src/smoke.mjs")
        .arg(executable)
        .arg(env!("CARGO_PKG_VERSION"))
        .current_dir(workspace.root())
        .status()
        .context("starting the smoke controller")?;
    ensure!(status.success(), "smoke failed ({status})");
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::Command;

    #[test]
    fn smoke_controller_tests() {
        let status = Command::new("node")
            .args(["--test", "xtask/src/smoke.test.mjs"])
            .current_dir(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap(),
            )
            .status()
            .expect("starting smoke controller tests");
        assert!(status.success(), "smoke controller tests failed ({status})");
    }
}
