//! Login and status commands for a daemon-admitted canonical provider home.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Fixed authentication operation and the directory whose project layers are excluded.
#[derive(Debug, PartialEq, Eq)]
pub struct ManagedLogin {
    /// Canonical home admitted by the running daemon.
    pub cwd: PathBuf,
    /// Arguments after the daemon-resolved Codex program.
    pub args: Vec<OsString>,
}

/// The authoritative home cannot form a supported managed login.
#[derive(Debug, thiserror::Error)]
#[error("Codex streaming login requires a daemon-admitted absolute home on macOS or Linux")]
pub struct InvalidLoginHome;

/// Provider-owned interactive login in the daemon's admitted home.
///
/// # Errors
/// The path is relative or contains parent traversal, or the platform is unsupported.
pub fn managed_login(home: &Path) -> Result<ManagedLogin, InvalidLoginHome> {
    if !cfg!(any(target_os = "macos", target_os = "linux"))
        || !home.is_absolute()
        || home
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(InvalidLoginHome);
    }
    let entries = home
        .ancestors()
        .map(|path| {
            let path = path.to_str().ok_or(InvalidLoginHome)?;
            let key = serde_json::to_string(path).map_err(|_| InvalidLoginHome)?;
            Ok(format!("{key} = {{ trust_level = \"untrusted\" }}"))
        })
        .collect::<Result<Vec<_>, InvalidLoginHome>>()?;
    let mut args = ["--disable", "hooks", "--disable", "plugins", "-c"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    args.push(format!("projects={{ {} }}", entries.join(", ")).into());
    args.push("login".into());
    Ok(ManagedLogin {
        cwd: home.to_path_buf(),
        args,
    })
}

/// Read only the provider's sign-in status using the same login isolation plan.
///
/// # Errors
/// As [`managed_login`].
pub fn managed_login_status(home: &Path) -> Result<ManagedLogin, InvalidLoginHome> {
    let mut plan = managed_login(home)?;
    plan.args.push("status".into());
    Ok(plan)
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn login_plan_covers_every_home_ancestor_and_keeps_metacharacters_literal() {
        let home = Path::new("/data/a b;$(not-a-command)/codex");
        let plan = managed_login(home).unwrap();
        assert_eq!(plan.cwd, home);
        assert_eq!(plan.args.last().unwrap(), "login");
        let overrides = plan.args[5].to_str().unwrap();
        for path in home.ancestors() {
            assert!(overrides.contains(&format!(
                "{} = {{ trust_level = \"untrusted\" }}",
                serde_json::to_string(path.to_str().unwrap()).unwrap()
            )));
        }
        assert_eq!(
            &managed_login_status(home).unwrap().args[6..],
            [OsString::from("login"), OsString::from("status")]
        );
        assert!(managed_login(Path::new("relative")).is_err());
        assert!(managed_login(Path::new("/data/../other")).is_err());
    }
}
