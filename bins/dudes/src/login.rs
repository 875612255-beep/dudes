//! An engine's own interactive sign-in, run only when the operator asks.
//!
//! The program is the absolute path in the daemon's `system.doctor` row for that engine, never a
//! flag, an environment variable or a search in this process. On Unix the login replaces this
//! process, so no Dudes process stays between the operator and the vendor's program. On Windows
//! the command is printed and nothing runs.

use std::path::{Path, PathBuf};

use dudes_proto::rpc::system::{ProviderLoginHome, ProviderProbe};
use dudes_providers::codex::login::{ManagedLogin, managed_login};
use dudes_types::{Escaped, Provider};

#[cfg(unix)]
use std::os::unix::process::CommandExt as _;

/// `claude auth login`, after the program.
pub const CLAUDE_LOGIN: &[&str] = &["auth", "login"];
/// `codex login`, after the program.
pub const CODEX_LOGIN: &[&str] = &["login"];

/// What `dudes login pi` says: Pi has no login subcommand.
pub const PI_GUIDANCE: &str = "Pi signs in inside its own session: run pi, then type /login.";

/// An engine whose CLI has a login command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

impl Engine {
    /// The engine behind `provider`, when it has a login command.
    #[must_use]
    pub fn of(provider: Provider) -> Option<Self> {
        match provider {
            Provider::Claude => Some(Self::Claude),
            Provider::Codex => Some(Self::Codex),
            _ => None,
        }
    }

    /// The arguments after the program.
    #[must_use]
    pub fn tokens(self) -> &'static [&'static str] {
        match self {
            Self::Claude => CLAUDE_LOGIN,
            Self::Codex => CODEX_LOGIN,
        }
    }

    /// The engine's name in a sentence.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    /// The provider name `dudes login` takes.
    #[must_use]
    pub fn provider(self) -> Provider {
        match self {
            Self::Claude => Provider::Claude,
            Self::Codex => Provider::Codex,
        }
    }
}

/// A login the daemon's row makes runnable: the engine and the absolute program it found.
#[derive(Debug, PartialEq, Eq)]
pub struct Login {
    engine: Engine,
    program: PathBuf,
    managed: Option<ManagedCodexLogin>,
}

#[derive(Debug, PartialEq, Eq)]
struct ManagedCodexLogin {
    home: PathBuf,
    plan: ManagedLogin,
}

impl Login {
    /// The login for `engine` from its `system.doctor` row, or why there is none.
    ///
    /// # Errors
    ///
    /// A sentence for the operator when the program or managed sign-in home is unavailable,
    /// relative, or contains bytes that cannot be printed safely.
    pub fn from_row(engine: Engine, row: Option<&ProviderProbe>) -> Result<Self, String> {
        let title = engine.title();
        let Some(row) = row else {
            return Err(format!(
                "the daemon has no {title} engine; turn it on with dudes quickstart"
            ));
        };
        let managed = match &row.login_home {
            None => None,
            Some(ProviderLoginHome::Unavailable { reason }) => {
                return Err(format!(
                    "the daemon cannot prepare {title} sign-in: {reason}"
                ));
            }
            Some(ProviderLoginHome::Ready { path }) => {
                if engine != Engine::Codex || !path.is_absolute() {
                    return Err(format!(
                        "the daemon reported an invalid {title} sign-in home; nothing was run"
                    ));
                }
                if !printable(path.as_os_str()) {
                    return Err(format!(
                        "the daemon's {title} sign-in home cannot be printed safely; nothing was run"
                    ));
                }
                let plan = managed_login(path).map_err(|error| error.to_string())?;
                if !plan.args.iter().all(|arg| printable(arg)) {
                    return Err(
                        "the daemon's login arguments cannot be printed safely; nothing was run"
                            .to_owned(),
                    );
                }
                Some(ManagedCodexLogin {
                    home: path.clone(),
                    plan,
                })
            }
        };
        if row
            .command
            .as_ref()
            .is_some_and(|program| !printable(program.as_os_str()))
        {
            return Err(format!(
                "the daemon's {title} program path cannot be printed safely; nothing was run"
            ));
        }
        match &row.command {
            Some(program) if row.ok && program.is_absolute() => Ok(Self {
                engine,
                program: program.clone(),
                managed,
            }),
            Some(program) if row.ok => Err(format!(
                "the daemon reports {title} at {}, which is not an absolute path, so nothing was run",
                program.display()
            )),
            _ => Err(format!(
                "the daemon did not find {title} on its PATH; install it, restart the daemon, \
                 then run dudes login {}",
                engine.provider()
            )),
        }
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> Engine {
        self.engine
    }

    /// The provider-approved login directory, or the legacy operator-home behavior.
    #[must_use]
    pub fn working_directory(&self) -> Option<&Path> {
        self.managed
            .as_ref()
            .map(|managed| managed.plan.cwd.as_path())
    }

    /// The command as the operator would type it.
    #[must_use]
    pub fn command(&self) -> String {
        let program = self.program.display().to_string();
        if cfg!(windows) {
            format!(
                "& {} {}",
                ps_literal(&program),
                self.engine.tokens().join(" ")
            )
        } else {
            let arguments = match &self.managed {
                Some(managed) => managed
                    .plan
                    .args
                    .iter()
                    .map(|arg| posix_word(arg.to_str().expect("validated login argument")))
                    .collect::<Vec<_>>()
                    .join(" "),
                None => self.engine.tokens().join(" "),
            };
            let command = format!("{} {arguments}", posix_word(&program));
            match &self.managed {
                Some(managed) => format!(
                    "(cd {} && env -u DUDES_TOKEN -u DUDES_SOCK CODEX_HOME={} {command})",
                    posix_word(&managed.plan.cwd.display().to_string()),
                    posix_word(&managed.home.display().to_string())
                ),
                None => command,
            }
        }
    }

    /// Replace this process with the login, in `home`. Returns only when that failed.
    #[cfg(unix)]
    #[must_use]
    pub fn exec(&self, home: &Path) -> std::io::Error {
        exec(self.engine, &self.program, home, self.managed.as_ref())
    }
}

fn printable(value: &std::ffi::OsStr) -> bool {
    value
        .to_str()
        .is_some_and(|text| Escaped(text).to_string() == text)
}

fn posix_word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/_-+=.,:@".contains(&byte))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn ps_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(unix)]
#[expect(
    clippy::disallowed_types,
    reason = "the engine's own login replaces this process, from the absolute path \
              the daemon found"
)]
fn exec(
    engine: Engine,
    program: &Path,
    home: &Path,
    managed: Option<&ManagedCodexLogin>,
) -> std::io::Error {
    if !program.is_absolute() {
        return std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the login program is not an absolute path",
        );
    }
    let tokens = match engine {
        Engine::Claude => CLAUDE_LOGIN,
        Engine::Codex => CODEX_LOGIN,
    };
    let mut command = std::process::Command::new(program);
    if let Some(managed) = managed {
        command.args(&managed.plan.args);
        command.env("CODEX_HOME", &managed.home);
        command.current_dir(&managed.plan.cwd);
    } else {
        command.args(tokens);
        command.current_dir(home);
    }
    command.env_remove("DUDES_TOKEN");
    command.env_remove("DUDES_SOCK");
    command.exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute on the host platform, as the daemon's search reports a found engine.
    const CODEX: &str = if cfg!(windows) {
        r"C:\engines\codex.exe"
    } else {
        "/opt/bin/codex"
    };
    const CLAUDE: &str = if cfg!(windows) {
        r"C:\engines\claude.exe"
    } else {
        "/opt/bin/claude"
    };

    fn row(command: Option<&str>, ok: bool) -> ProviderProbe {
        ProviderProbe {
            login_home: None,
            provider: Provider::Codex,
            command: command.map(PathBuf::from),
            version: None,
            ok,
            detail: None,
            sign_in: None,
        }
    }

    #[test]
    fn copyable_commands_quote_spaces_and_metacharacters() {
        let (program, expected) = if cfg!(windows) {
            (
                r"C:\Bob's tools;$home\codex.exe",
                r"& 'C:\Bob''s tools;$home\codex.exe' login",
            )
        } else {
            (
                "/opt/Bob's tools;$(false)/codex",
                "'/opt/Bob'\"'\"'s tools;$(false)/codex' login",
            )
        };
        let login = Login::from_row(Engine::Codex, Some(&row(Some(program), true))).unwrap();
        assert_eq!(login.command(), expected);
    }

    #[test]
    fn ps_literals_keep_metacharacters_literal() {
        assert_eq!(
            ps_literal(r"C:\Bob's tools;$home\codex.exe"),
            r"'C:\Bob''s tools;$home\codex.exe'"
        );
    }

    #[test]
    fn login_paths_with_terminal_controls_are_refused_without_echoing_them() {
        for control in ['\n', '\u{1b}', '\u{202e}', '\u{2028}'] {
            let path = format!("{CODEX}{control}");
            let error = Login::from_row(Engine::Codex, Some(&row(Some(&path), true))).unwrap_err();
            assert!(error.contains("cannot be printed safely"));
            assert!(!error.contains(control));
        }
    }

    /// The tokens are the adapters' own: the login beside the status command each adapter runs.
    #[test]
    fn the_login_tokens_match_the_adapters() {
        assert_eq!(CLAUDE_LOGIN, dudes_providers::claude::managed::LOGIN);
        assert_eq!(CODEX_LOGIN, dudes_providers::codex::managed::LOGIN);
        assert_eq!(Engine::Claude.tokens(), CLAUDE_LOGIN);
        assert_eq!(Engine::Codex.tokens(), CODEX_LOGIN);
        assert_eq!(Engine::of(Provider::Pi), None);
    }

    /// Only an absolute path the daemon found makes a login; a bare name would make this process
    /// search `PATH`, so it runs nothing.
    #[test]
    fn only_an_absolute_found_program_makes_a_login() {
        let login = Login::from_row(Engine::Codex, Some(&row(Some(CODEX), true)))
            .expect("an absolute path the daemon found");
        let command = |path: &str, args: &str| {
            if cfg!(windows) {
                format!("& '{path}' {args}")
            } else {
                format!("{path} {args}")
            }
        };
        assert_eq!(login.command(), command(CODEX, "login"));
        assert_eq!(
            Login::from_row(Engine::Claude, Some(&row(Some(CLAUDE), true)))
                .expect("found")
                .command(),
            command(CLAUDE, "auth login")
        );
        assert_eq!(
            Login::from_row(Engine::Codex, Some(&row(Some("codex"), true))),
            Err(
                "the daemon reports Codex at codex, which is not an absolute path, so nothing \
                 was run"
                    .to_owned()
            )
        );
        assert_eq!(
            Login::from_row(Engine::Codex, Some(&row(Some(CODEX), false))),
            Err(
                "the daemon did not find Codex on its PATH; install it, restart the daemon, then \
                 run dudes login codex"
                    .to_owned()
            )
        );
        assert_eq!(
            Login::from_row(Engine::Codex, Some(&row(None, false))),
            Login::from_row(Engine::Codex, Some(&row(Some(CODEX), false)))
        );
        assert_eq!(
            Login::from_row(Engine::Codex, None),
            Err("the daemon has no Codex engine; turn it on with dudes quickstart".to_owned())
        );
    }

    /// The spawn item refuses a relative program itself, before any `PATH` search could run.
    #[cfg(unix)]
    #[test]
    fn the_exec_item_refuses_a_relative_program() {
        let error = exec(
            Engine::Codex,
            Path::new("dudes-no-such-engine"),
            Path::new("/"),
            None,
        );
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{error}");
    }
}
