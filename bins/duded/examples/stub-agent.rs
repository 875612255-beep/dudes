//! A provider CLI that costs nothing, so the cost-reporting check can be observed rather than asserted.
//!
//! # Why this exists
//!
//! `spent_today` moves only when a turn ends with usage, and usage only arrives from a provider.
//! So the only way to a non-zero figure was to spend real vendor credit — which made the
//! acceptance run cost money, made it non-deterministic, and made it unrunnable on a clean host
//! with no vendor account, which is exactly the host the gate is meant to run on. This program
//! stands in for the CLI: it speaks the one line of the vendor's stream-json protocol that carries
//! usage, and it stops.
//!
//! # It is not a provider, and cannot be reached as one
//!
//! `Provider` still names ten real CLIs and no stub — that enum is not this example's to change, and this
//! changes nothing about it. What changes is *which program* the Claude adapter runs, which is already a
//! per-deployment value (`ClaudeConfig::command`). The daemon reads it only under the
//! `stub-provider` feature, which is off by default, never enabled by `--workspace`, and asserted
//! absent from every manifest's defaults by `tests/dev_features.rs`. This example is itself
//! `required-features = ["stub-provider"]`, so an ordinary build does not even produce it.
//!
//! # It announces its session first, because an unattributed result buys nothing
//!
//! The stream parser holds a `result` line *provisional* until the turn has announced a session,
//! and refuses anything still provisional when the turn ends
//! (`crates/dudes-providers/src/claude/stream.rs`). That is invariant 3 — evidence the daemon
//! cannot attribute is not evidence — and a stub that skipped the announcement would have its cost
//! discarded, end every turn with no usage, and quietly prove nothing.
//!
//! So the first line is the vendor's `system/init`, echoing the id the daemon minted and passed as
//! `--session-id` (or `--resume` when it resumes one).
//!
//! # The two-agent mechanism, and why it reads argv
//!
//! That check needs two agents at two *different* non-zero costs, so the stub's answer has to vary per
//! turn. The child's environment carries only `DUDES_SOCK` and `DUDES_TOKEN`
//! (set where the supervisor builds the child's environment) and no agent identity, so there is nothing
//! there to key on. The **task text is one argv element** — invariant 7, and
//! the provider adapter says so at the seam — and two agents given
//! different goals therefore receive different bytes. So the usage is a function of the task text,
//! and nothing else:
//!
//! ```text
//! tokens = the task text's length in bytes
//! usd    = tokens thousandths of a dollar, emitted as an exact six-decimal string
//! ```
//!
//! The dollar figure is computed in integer micros and formatted by hand rather than divided into
//! a float: the aggregate is carried as micros downstream, and a float here would be a rounding
//! step between two integer representations for no reason.
//!
//! Deterministic, distinct whenever the two task texts differ in length, and — the part that
//! matters — **the gate never needs to know this rule**. It reads both figures out of the journal
//! and asserts they are non-zero and different. A stub whose arithmetic the acceptance script
//! reimplemented would be a script asserting its own opinion twice.

use std::io::Write as _;

/// One line on stderr, without `eprintln!` — the workspace denies the print macros, and an example
/// in this package is inside that rule like everything else.
fn complain(message: &str) -> ! {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(message.as_bytes());
    let _ = stderr.write_all(b"\n");
    std::process::exit(2);
}

/// The model key the usage is attributed to.
///
/// A name no vendor uses, so a journal carrying it cannot be mistaken for a record of a real turn
/// by anyone reading it later — including the reader who finds this file's output in a fixture and
/// wonders which model ran.
const STUB_MODEL: &str = "stub-agent-no-model";

fn main() {
    // The session the daemon minted for this turn, under whichever flag carried it. A turn with
    // neither is one this program cannot attribute anything to, and saying so beats emitting a
    // result the daemon will refuse.
    let mut argv = std::env::args_os().collect::<Vec<_>>();
    let session = match argv
        .windows(2)
        .find(|pair| pair[0] == *"--session-id" || pair[0] == *"--resume")
    {
        Some(pair) => pair[1].to_string_lossy().into_owned(),
        None => complain("stub-agent: no --session-id or --resume in the argv"),
    };

    // The task text is the last argv element: the adapter appends it after `--` where the CLI
    // supports it, and it is always one element. Empty argv means nobody asked for a turn, which
    // is a usage error rather than a turn worth reporting.
    let Some(task) = argv.pop() else {
        complain("stub-agent: no argv; this stands in for a provider CLI and is not run by hand");
    };
    let tokens = task.as_encoded_bytes().len() as u64;
    if tokens == 0 {
        complain(
            "stub-agent: the task text is empty, so this turn would cost nothing and prove nothing",
        );
    }
    let micros = tokens.saturating_mul(1_000);
    let (whole, frac) = (micros / 1_000_000, micros % 1_000_000);

    let line = format!(
        concat!(
            r#"{{"type":"result","subtype":"success","is_error":false,"#,
            r#""session_id":"{session}","result":"stub turn","total_cost_usd":{whole}.{frac:06},"#,
            r#""modelUsage":{{"{model}":{{"inputTokens":{tokens},"outputTokens":0,"#,
            r#""cacheReadInputTokens":0,"cacheCreationInputTokens":0}}}}}}"#
        ),
        session = session,
        whole = whole,
        frac = frac,
        model = STUB_MODEL,
        tokens = tokens,
    );

    let announcement =
        format!(r#"{{"type":"system","subtype":"init","session_id":"{session}","tools":[]}}"#);

    let mut stdout = std::io::stdout().lock();
    if writeln!(stdout, "{announcement}").is_err()
        || writeln!(stdout, "{line}").is_err()
        || stdout.flush().is_err()
    {
        // The daemon closed the pipe. Nothing to report and nobody to report it to.
        std::process::exit(1);
    }
}
