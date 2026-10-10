use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::time::Duration;

use dudes_providers::launch::{EnvVar, LaunchInput, LaunchSpec};

use crate::predicate::{PredicateCaptured, PredicateRunner};

const DEADLINE: Duration = Duration::from_secs(5);
const OUTPUT_CAP: usize = 256;

/// A version probe ready to run, built on the loop thread and run on another.
#[derive(Debug)]
pub(crate) struct Probe {
    spec: LaunchSpec,
}

/// Resolve `program` and build the `--version` probe. No process runs here, so this is safe on the
/// loop thread.
pub(crate) fn prepare(program: &OsStr, env: &[EnvVar], cwd: &Path) -> io::Result<Probe> {
    let mut env = env.to_vec();
    env.retain(|var| {
        !["DUDES_TOKEN", "DUDES_SOCK", "DUDES_MAILBOX"]
            .iter()
            .any(|name| {
                if cfg!(windows) {
                    var.name()
                        .as_encoded_bytes()
                        .eq_ignore_ascii_case(name.as_bytes())
                } else {
                    var.name() == OsStr::new(name)
                }
            })
    });
    let program = crate::program::resolve(program, crate::program::child_path(&env))?;
    Ok(Probe {
        spec: LaunchSpec {
            argv: vec![program.into(), "--version".into()],
            env,
            cwd: cwd.to_path_buf(),
            stdin: LaunchInput::Null,
        },
    })
}

/// Run the probe and judge its answer. Blocks for up to the deadline, so never call it on the loop
/// thread.
pub(crate) fn run(probe: &Probe, runner: &dyn PredicateRunner) -> io::Result<()> {
    judge(&runner.run_captured(&probe.spec, DEADLINE, OUTPUT_CAP))
}

/// The refusal for a `pi` that could not be resolved at all.
pub(crate) fn unavailable() -> io::Error {
    judge(&Err(io::ErrorKind::NotFound.into())).unwrap_err()
}

fn judge(outcome: &io::Result<PredicateCaptured>) -> io::Result<()> {
    if let Ok(PredicateCaptured::Finished {
        exit: Some(0),
        stdout,
        truncated: false,
        timed_out: false,
    }) = outcome
        && dudes_providers::pi::tested_mcp_version(stdout)
    {
        return Ok(());
    }
    let report = match outcome {
        Ok(PredicateCaptured::Finished { stdout, .. }) if !stdout.is_empty() => {
            format!(
                "the installed pi reports {}",
                String::from_utf8_lossy(stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
            )
        }
        _ => "the installed pi did not answer".to_owned(),
    };
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!(
            "Pi MCP servers need a tested pi ({}); {report}",
            dudes_providers::pi::PI_MCP_TESTED_VERSIONS.join(", ")
        ),
    ))
}
