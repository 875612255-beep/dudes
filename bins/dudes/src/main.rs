//! `dudes` — the Dudes control CLI.
//!
//! # This file is the only one that names a real stream or a real environment
//!
//! `bins/duded/src/main.rs` keeps the same shape for the same reason: everything ambient is read
//! here, once, and handed on as values. [`dudes::run`] takes its argv, its environment and both of
//! its streams as arguments, so the whole binary is drivable from a test with no process-wide
//! mutation — which matters here because a test that set `DUDES_TOKEN` on the process would leak it
//! into every other test's subprocess.
//!
//! # No subscriber, and nothing on stderr but this binary's own words
//!
//! `tracing` is not linked at all. A CLI's diagnostics are its output, and a subscriber would put a
//! second, unrendered channel beside it — which is exactly the channel a secret leaks through. The
//! `DUDES_TOKEN` test asserts over this process's real stdout and stderr, and a subscriber
//! would have given it a third stream to be wrong about.

use std::ffi::OsString;
use std::io::{BufRead, IsTerminal as _, Write as _};
use std::process::ExitCode;

use dudes::Environment;

fn main() -> ExitCode {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let environment = Environment::from_env();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let stderr = std::io::stderr();
    // A terminal on both sides is an operator who can answer; anything else is a script, which
    // is asked nothing.
    let terminal = stdin.is_terminal() && stdout.is_terminal();
    let mut input = stdin.lock();
    let mut out = stdout.lock();
    let mut err = stderr.lock();

    let code = dudes::run(
        &argv,
        &environment,
        terminal.then_some(&mut input as &mut dyn BufRead),
        &mut out,
        &mut err,
    );

    // Flushed before the exit code is returned, and both of them: `ExitCode` leaves the process
    // through `Termination`, which does not run `Drop` for these locks, so a buffered last line
    // would be lost exactly when a caller was reading it. A flush failure is a broken pipe — the
    // reader is gone, there is nowhere to report it, and the exit code still stands.
    let _ = out.flush();
    let _ = err.flush();
    ExitCode::from(code)
}
