//! Regression tests for explicit nulls in streamed vendor payloads.

use dudes_proto::{ExitStatus, TurnOutcome};
use dudes_providers::claude::stream::TurnStream;
use dudes_providers::stream::ScopedUsage;
use dudes_types::Usd;

const SESSION: &str = "3f9a1c2e-0000-4000-8000-000000000001";
const EXIT_0: ExitStatus = ExitStatus {
    code: Some(0),
    signal: None,
};

/// Invariant 4: a null vendor token counter cannot erase the result line and turn observed spend
/// into an unpriced `NoResult`. The counter defaults, but the other counters and dollar total
/// must still reach accounting.
#[test]
fn a_nulled_scalar_does_not_cost_the_turn_its_spend() {
    let mut turn = TurnStream::new();
    turn.push(&format!(
        r#"{{"type":"system","subtype":"init","session_id":"{SESSION}","tools":[]}}"#
    ));
    turn.push(&format!(
        concat!(
            r#"{{"type":"result","subtype":"success","is_error":false,"session_id":"{}","#,
            r#""total_cost_usd":0.25,"modelUsage":{{"claude-opus-4":{{"#,
            r#""inputTokens":null,"outputTokens":22,"cacheReadInputTokens":33,"#,
            r#""cacheCreationInputTokens":44}}}}}}"#
        ),
        SESSION
    ));

    assert!(
        turn.has_result(),
        "the nulled scalar must not erase the line"
    );
    let end = turn.finish(EXIT_0);
    let Some(ScopedUsage::Session { total: usage, .. }) = end.usage else {
        panic!("the readable result line must still account its spend");
    };

    assert_eq!(end.outcome, TurnOutcome::Completed);
    assert_eq!(end.unreadable, 0);
    assert_eq!(usage.input.count(), 0);
    assert_eq!(usage.output.count(), 22);
    assert_eq!(usage.cache_read.count(), 33);
    assert_eq!(usage.cache_creation.count(), 44);
    assert_eq!(usage.usd, Some(Usd::from_micros(250_000)));
}
