//! Incremental provider text through the daemon's turn-reader interface.

use dudes_proto::{ExitStatus, TurnOutcome};
use dudes_providers::{claude, pi, stream::TurnReader};
use serde_json::{Value, json};

const SESSION: &str = "stream-session";

fn read(reader: &mut dyn TurnReader, event: &Value) -> String {
    let blocks = reader.line(&event.to_string()).text;
    for block in &blocks {
        assert_eq!(block.session.as_str(), SESSION);
    }
    blocks.into_iter().map(|block| block.text).collect()
}

fn claude_event(event: Value) -> Value {
    {
        let mut wrapper =
            json!({"type":"stream_event", "session_id":SESSION, "parent_tool_use_id":null});
        wrapper["event"] = event;
        wrapper
    }
}

fn claude_start(id: &str) -> Value {
    claude_event(
        json!({"type":"message_start", "message":{"id":id,"role":"assistant","content":[]}}),
    )
}

fn claude_text_start(index: u64) -> Value {
    claude_event(
        json!({"type":"content_block_start", "index":index,"content_block":{"type":"text","text":""}}),
    )
}

fn claude_delta(index: u64, text: &str) -> Value {
    claude_event(
        json!({"type":"content_block_delta", "index":index,"delta":{"type":"text_delta","text":text}}),
    )
}

fn claude_end(id: &str, text: &str) -> Value {
    json!({"type":"assistant","session_id":SESSION,"message":{"id":id,"role":"assistant","content":[{"type":"text","text":text}]}})
}

fn claude_reader() -> claude::stream::TurnStream {
    let mut reader = claude::stream::TurnStream::new();
    read(
        &mut reader,
        &json!({"type":"system","subtype":"init","session_id":SESSION}),
    );
    reader
}

#[test]
fn claude_streams_before_completion_and_does_not_repeat_the_snapshot() {
    let mut reader = claude_reader();
    read(&mut reader, &claude_start("first"));
    read(&mut reader, &claude_text_start(0));
    assert_eq!(read(&mut reader, &claude_delta(0, "Hello ")), "Hello ");
    assert_eq!(read(&mut reader, &claude_delta(0, "世界")), "世界");
    read(
        &mut reader,
        &claude_event(json!({"type":"content_block_stop","index":0})),
    );
    assert_eq!(read(&mut reader, &claude_end("first", "Hello 世界!")), "!");
    assert_eq!(read(&mut reader, &claude_end("first", "Hello 世界!")), "");
    read(&mut reader, &claude_event(json!({"type":"message_stop"})));
    assert_eq!(read(&mut reader, &claude_delta(0, "late")), "");
    read(&mut reader, &claude_start("second"));
    read(&mut reader, &claude_text_start(0));
    assert_eq!(read(&mut reader, &claude_delta(0, "Hello ")), "\nHello ");
    assert_eq!(
        read(&mut reader, &claude_end("second", "Hello again")),
        "again"
    );
    read(
        &mut reader,
        &json!({"type":"result","session_id":SESSION,"subtype":"success","result":"final reply"}),
    );
    let end = reader.finish(ExitStatus {
        code: Some(0),
        signal: None,
    });
    assert_eq!(end.outcome, TurnOutcome::Completed);
    assert_eq!(end.last_message.as_deref(), Some("final reply"));
}

#[test]
fn claude_excludes_unattributed_nested_synthetic_and_nontext_fragments() {
    let mut reader = claude_reader();
    read(&mut reader, &claude_start("first"));
    read(&mut reader, &claude_text_start(0));
    for (field, value) in [
        ("session_id", json!("other")),
        ("session_id", Value::Null),
        ("parent_tool_use_id", json!("tool")),
        ("isSynthetic", json!(true)),
    ] {
        let mut event = claude_delta(0, "hidden");
        event[field] = value;
        assert_eq!(read(&mut reader, &event), "");
    }
    read(
        &mut reader,
        &claude_event(
            json!({"type":"content_block_start","index":1,"content_block":{"type":"thinking"}}),
        ),
    );
    assert_eq!(read(&mut reader, &claude_delta(1, "hidden")), "");
    assert_eq!(
        read(
            &mut reader,
            &claude_event(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hidden"}})
            )
        ),
        ""
    );
    assert_eq!(read(&mut reader, &claude_delta(0, "visible")), "visible");
    assert_eq!(read(&mut reader, &claude_end("first", "visible")), "");
    let mut unbound = claude::stream::TurnStream::new();
    read(&mut unbound, &claude_start("first"));
    read(&mut unbound, &claude_text_start(0));
    assert_eq!(read(&mut unbound, &claude_delta(0, "hidden")), "");
}

fn pi_update(event: Value) -> Value {
    {
        let mut wrapper = json!({"type":"message_update"});
        wrapper["assistantMessageEvent"] = event;
        wrapper
    }
}

fn pi_start() -> Value {
    json!({"type":"message_start","message":{"role":"assistant","content":[],"stopReason":"pending"}})
}

fn pi_delta(index: u64, text: &str) -> Value {
    pi_update(json!({"type":"text_delta","contentIndex":index,"delta":text}))
}

fn pi_end(text: &str) -> Value {
    json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":text}],"stopReason":"stop","usage":{"input":5,"output":2}}})
}

fn pi_reader() -> pi::stream::TurnStream {
    let mut reader = pi::stream::TurnStream::new();
    read(&mut reader, &json!({"type":"session","id":SESSION}));
    reader
}

#[test]
fn pi_streams_before_completion_and_reconciles_each_block_once() {
    let mut reader = pi_reader();
    read(&mut reader, &pi_start());
    read(
        &mut reader,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "Hello ")), "Hello ");
    assert_eq!(read(&mut reader, &pi_delta(0, "世界")), "世界");
    assert_eq!(
        read(
            &mut reader,
            &pi_update(json!({"type":"text_end","contentIndex":0,"content":"Hello 世界!"}))
        ),
        "!"
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "late")), "");
    assert_eq!(read(&mut reader, &pi_end("Hello 世界!")), "");
    read(&mut reader, &json!({"type":"agent_settled"}));
    let end = Box::new(reader).end(ExitStatus {
        code: Some(0),
        signal: None,
    });
    assert_eq!(end.outcome, TurnOutcome::Completed);
    assert_eq!(end.last_message.as_deref(), Some("Hello 世界!"));
    assert_eq!(
        end.usage
            .unwrap()
            .charge(dudes_types::Usage::ZERO)
            .input
            .count(),
        5
    );
}

#[test]
fn pi_resets_on_message_boundaries_and_excludes_nontext_and_compaction() {
    let mut reader = pi_reader();
    assert_eq!(read(&mut reader, &pi_delta(0, "orphan")), "");
    for cycle in 0..3 {
        read(&mut reader, &pi_start());
        read(
            &mut reader,
            &pi_update(json!({"type":"thinking_start","contentIndex":1})),
        );
        read(
            &mut reader,
            &pi_update(json!({"type":"toolcall_start","contentIndex":2})),
        );
        assert_eq!(read(&mut reader, &pi_delta(1, "thinking")), "");
        assert_eq!(read(&mut reader, &pi_delta(2, "tool")), "");
        read(
            &mut reader,
            &pi_update(json!({"type":"text_start","contentIndex":0})),
        );
        assert_eq!(
            read(&mut reader, &pi_delta(0, "visible")),
            if cycle == 0 { "visible" } else { "\nvisible" }
        );
        assert_eq!(read(&mut reader, &pi_end("visible")), "");
        assert_eq!(read(&mut reader, &pi_delta(0, "after-end")), "");
        read(&mut reader, &json!({"type":"agent_end","willRetry":true}));
        read(
            &mut reader,
            &json!({"type":"auto_retry_start","attempt":cycle+1}),
        );
        read(&mut reader, &json!({"type":"compaction_start"}));
        assert_eq!(read(&mut reader, &pi_delta(0, "summary")), "");
        read(&mut reader, &json!({"type":"compaction_end","result":{}}));
    }
}

#[test]
fn malformed_deltas_do_not_displace_completed_messages() {
    let mut claude = claude_reader();
    read(&mut claude, &claude_start("first"));
    read(&mut claude, &claude_text_start(0));
    assert_eq!(
        read(
            &mut claude,
            &claude_event(
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":42}})
            )
        ),
        ""
    );
    assert_eq!(read(&mut claude, &claude_end("first", "final")), "final");
    let mut pi = pi_reader();
    read(&mut pi, &pi_start());
    read(
        &mut pi,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(
        read(
            &mut pi,
            &pi_update(json!({"type":"text_delta","contentIndex":0,"delta":42}))
        ),
        ""
    );
    assert_eq!(read(&mut pi, &pi_end("final")), "final");
}

#[test]
fn claude_reconciles_separately_completed_blocks_of_the_same_message() {
    let mut reader = claude_reader();
    read(&mut reader, &claude_start("first"));
    read(
        &mut reader,
        &claude_event(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}),
        ),
    );
    read(
        &mut reader,
        &claude_event(json!({"type":"content_block_stop","index":0})),
    );
    for (index, text, expected) in [
        (1, "first block", "first block"),
        (2, "second block", "\nsecond block"),
    ] {
        read(&mut reader, &claude_text_start(index));
        assert_eq!(read(&mut reader, &claude_delta(index, text)), expected);
        assert_eq!(read(&mut reader, &claude_end("first", text)), "");
        read(
            &mut reader,
            &claude_event(json!({"type":"content_block_stop","index":index})),
        );
        assert_eq!(read(&mut reader, &claude_end("first", text)), "");
    }
    assert_eq!(
        read(
            &mut reader,
            &json!({"type":"assistant","session_id":SESSION,"message":{"id":"first","role":"assistant","content":[{"type":"thinking"},{"type":"text","text":"first block"},{"type":"text","text":"second block"}]}})
        ),
        ""
    );
    assert_eq!(
        read(&mut reader, &claude_end("without-partials", "fallback one")),
        "\nfallback one"
    );
    assert_eq!(
        read(&mut reader, &claude_end("without-partials", "fallback two")),
        "\nfallback two"
    );
}

#[test]
fn claude_retry_and_compaction_require_a_new_message_start() {
    let mut reader = claude_reader();
    for (subtype, expected) in [("api_retry", "before"), ("compact_boundary", "\nbefore")] {
        read(&mut reader, &claude_start("first"));
        read(&mut reader, &claude_text_start(0));
        assert_eq!(read(&mut reader, &claude_delta(0, "before")), expected);
        read(
            &mut reader,
            &json!({"type":"system","subtype":subtype,"session_id":SESSION}),
        );
        assert_eq!(read(&mut reader, &claude_delta(0, "orphan")), "");
        read(&mut reader, &claude_start("next"));
        read(&mut reader, &claude_text_start(0));
        assert_eq!(read(&mut reader, &claude_delta(0, "after")), "\nafter");
        assert_eq!(read(&mut reader, &claude_end("next", "after")), "");
    }
}

#[test]
fn pi_compaction_and_user_messages_do_not_start_streaming_text() {
    let mut reader = pi_reader();
    read(
        &mut reader,
        &json!({"type":"message_start","message":{"role":"user","content":[]}}),
    );
    read(
        &mut reader,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "user")), "");
    read(&mut reader, &pi_start());
    read(
        &mut reader,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "visible")), "visible");
    read(&mut reader, &json!({"type":"compaction_start"}));
    assert_eq!(read(&mut reader, &pi_delta(0, "summary")), "");
    read(&mut reader, &pi_start());
    read(
        &mut reader,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "summary")), "");
    let mut unbound = pi::stream::TurnStream::new();
    read(&mut unbound, &pi_start());
    read(
        &mut unbound,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut unbound, &pi_delta(0, "unbound")), "");
}

#[test]
fn pi_captured_stream_publishes_the_delta_before_message_end() {
    let mut reader = pi::stream::TurnStream::new();
    let mut output = String::new();
    let mut saw_delta = false;
    for line in include_str!("fixtures/pi/l1-success.jsonl").lines() {
        let event: Value = serde_json::from_str(line).unwrap();
        let text: String = reader
            .line(line)
            .text
            .into_iter()
            .map(|block| block.text)
            .collect();
        if event
            .pointer("/assistantMessageEvent/type")
            .and_then(Value::as_str)
            == Some("text_delta")
        {
            assert_eq!(text, "OK");
            saw_delta = true;
        }
        if event.get("type").and_then(Value::as_str) == Some("message_end") {
            assert_eq!(text, "");
        }
        output.push_str(&text);
    }
    assert!(saw_delta);
    assert_eq!(output, "OK");
}

#[test]
fn corrected_completed_content_is_authoritative_and_closes_live_text() {
    let mut reader = pi_reader();
    read(&mut reader, &pi_start());
    read(
        &mut reader,
        &pi_update(json!({"type":"text_start","contentIndex":0})),
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "draft")), "draft");
    assert_eq!(
        read(
            &mut reader,
            &pi_update(json!({"type":"text_end","contentIndex":0,"content":"corrected"}))
        ),
        ""
    );
    assert_eq!(read(&mut reader, &pi_delta(0, "late")), "");
    read(&mut reader, &pi_end("corrected"));
    read(&mut reader, &json!({"type":"agent_settled"}));
    let end = Box::new(reader).end(ExitStatus {
        code: Some(0),
        signal: None,
    });
    assert_eq!(end.final_reply.unwrap().text, "corrected");
    assert_eq!(end.last_message.as_deref(), Some("corrected"));
}
