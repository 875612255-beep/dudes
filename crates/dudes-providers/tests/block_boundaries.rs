//! Logical assistant blocks remain separate when consumers concatenate incremental output.

use dudes_proto::{ExitStatus, TurnOutcome};
use dudes_providers::stream::FinalReplyCompleteness;
use dudes_providers::{claude, codex, pi, stream::TurnReader};
use serde_json::{Value, json};

fn append(reader: &mut dyn TurnReader, output: &mut String, event: &Value) {
    for block in reader.line(&event.to_string()).text {
        output.push_str(&block.text);
    }
}

fn claude_event(reader: &mut dyn TurnReader, output: &mut String, event: Value) {
    let mut wrapper =
        json!({"type":"stream_event","session_id":"session","parent_tool_use_id":null});
    wrapper["event"] = event;
    append(reader, output, &wrapper);
}

fn pi_event(reader: &mut dyn TurnReader, output: &mut String, event: Value) {
    let mut wrapper = json!({"type":"message_update"});
    wrapper["assistantMessageEvent"] = event;
    append(reader, output, &wrapper);
}

#[test]
fn claude_streamed_blocks_keep_boundaries_without_splitting_fragments() {
    let mut reader = claude::stream::TurnStream::new();
    let mut output = String::new();
    append(
        &mut reader,
        &mut output,
        &json!({"type":"system","subtype":"init","session_id":"session"}),
    );
    for id in ["first", "second"] {
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"message_start","message":{"id":id,"role":"assistant"}}),
        );
        for (index, kind) in [(0, "text"), (1, "thinking"), (2, "tool_use")] {
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_start","index":index,"content_block":{"type":kind,"text":""}}),
            );
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_stop","index":index}),
            );
        }
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}),
        );
        for fragment in ["```rust\n", "fn main() {}\n", "```"] {
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_delta","index":3,"delta":{"type":"text_delta","text":fragment}}),
            );
        }
        append(
            &mut reader,
            &mut output,
            &json!({"type":"assistant","session_id":"session","message":{"id":id,"role":"assistant","content":[{"type":"text","text":"```rust\nfn main() {}\n```"}]}}),
        );
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_stop","index":3}),
        );
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_start","index":4,"content_block":{"type":"text","text":""}}),
        );
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_delta","index":4,"delta":{"type":"text_delta","text":"do"}}),
        );
        append(
            &mut reader,
            &mut output,
            &json!({"type":"assistant","session_id":"session","message":{"id":id,"role":"assistant","content":[{"type":"text","text":"done"}]}}),
        );
        claude_event(&mut reader, &mut output, json!({"type":"message_stop"}));
    }
    assert_eq!(
        output,
        "```rust\nfn main() {}\n```\ndone\n```rust\nfn main() {}\n```\ndone"
    );
}

#[test]
fn pi_streamed_blocks_keep_boundaries_without_splitting_fragments() {
    let mut reader = pi::stream::TurnStream::new();
    let mut output = String::new();
    append(
        &mut reader,
        &mut output,
        &json!({"type":"session","id":"session"}),
    );
    for _ in 0..2 {
        append(
            &mut reader,
            &mut output,
            &json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_start","contentIndex":0}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_end","contentIndex":0,"content":""}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"toolcall_start","contentIndex":1}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_start","contentIndex":2}),
        );
        for fragment in ["first ", "block"] {
            pi_event(
                &mut reader,
                &mut output,
                json!({"type":"text_delta","contentIndex":2,"delta":fragment}),
            );
        }
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_end","contentIndex":2,"content":"first block"}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_start","contentIndex":3}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_delta","contentIndex":3,"delta":"second "}),
        );
        append(
            &mut reader,
            &mut output,
            &json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":""},{"type":"toolCall"},{"type":"text","text":"first block"},{"type":"text","text":"second block"}],"stopReason":"stop"}}),
        );
    }
    assert_eq!(
        output,
        "first block\nsecond block\nfirst block\nsecond block"
    );
}

#[test]
fn whole_block_fallback_keeps_boundaries_and_skips_empty_and_tool_blocks() {
    let mut readers: Vec<Box<dyn TurnReader>> = vec![
        Box::new(claude::stream::TurnStream::new()),
        Box::new(pi::stream::TurnStream::new()),
    ];
    for (index, reader) in readers.iter_mut().enumerate() {
        let mut output = String::new();
        let header = if index == 0 {
            json!({"type":"system","subtype":"init","session_id":"session"})
        } else {
            json!({"type":"session","id":"session"})
        };
        append(reader.as_mut(), &mut output, &header);
        for id in ["first", "second"] {
            let kind = if index == 0 {
                "assistant"
            } else {
                "message_end"
            };
            append(
                reader.as_mut(),
                &mut output,
                &json!({"type":kind,"session_id":"session","message":{"id":id,"role":"assistant","content":[{"type":"text","text":""},{"type":"text","text":"first block"},{"type":"tool_use"},{"type":"text","text":"second block"}]}}),
            );
        }
        assert_eq!(
            output,
            "first block\nsecond block\nfirst block\nsecond block"
        );
    }
}

#[test]
fn captured_claude_messages_keep_code_fences_and_words_separate() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/claude/r1-hooks.stdout"),
            "```\nStatus | agent=example | project=demo | branch=unknown\n```\ndone",
        ),
        (
            include_str!("fixtures/claude/r8-stopblock.stdout"),
            "```\nStatus | agent=example | project=demo | branch=unknown\n```\n\npong\nping",
        ),
    ] {
        let mut reader = claude::stream::TurnStream::new();
        let output: String = fixture
            .lines()
            .flat_map(|line| reader.line(line).text)
            .map(|block| block.text)
            .collect();
        assert_eq!(output, expected);
    }
}

#[test]
fn codex_completed_item_boundaries_use_item_identity() {
    let mut reader = codex::stream::TurnStream::new();
    let mut output = String::new();
    append(
        &mut reader,
        &mut output,
        &json!({"type":"thread.started","thread_id":"f480db8c-a42b-79a2-86a5-c9b3073a5c65"}),
    );
    for (id, kind, text) in [
        ("item_0", "agent_message", ""),
        ("item_1", "agent_message", "```rust\nfn main() {}\n```"),
        ("item_1", "agent_message", "```rust\nfn main() {}\n```"),
        ("item_2", "command_execution", "hidden"),
        ("item_3", "agent_message", "do"),
        ("item_3", "agent_message", "done"),
        ("item_3", "agent_message", "done"),
        ("item_4", "agent_message", "again"),
    ] {
        append(
            &mut reader,
            &mut output,
            &json!({"type":"item.completed","item":{"id":id,"type":kind,"text":text}}),
        );
    }
    assert_eq!(output, "```rust\nfn main() {}\n```\ndone\nagain");
}

#[test]
fn captured_codex_completed_items_keep_their_boundary() {
    let mut reader = codex::stream::TurnStream::new();
    let output: String = include_str!("fixtures/codex/c2-command.stdout")
        .lines()
        .flat_map(|line| reader.line(line).text)
        .map(|block| block.text)
        .collect();
    assert_eq!(output, "I’ll run `echo ok`.\n\nok");
}

#[test]
fn claude_block_limit_preserves_final_reply_and_resets_for_the_next_message() {
    for reset in [false, true] {
        let mut reader = claude::stream::TurnStream::new();
        let mut output = String::new();
        append(
            &mut reader,
            &mut output,
            &json!({"type":"system","subtype":"init","session_id":"session"}),
        );
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"message_start","message":{"id":"first","role":"assistant"}}),
        );
        for index in 0..1000 {
            let kind = if index % 2 == 0 { "thinking" } else { "text" };
            let sparse = usize::MAX - index * 2;
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_start","index":sparse,"content_block":{"type":kind,"text":""}}),
            );
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_stop","index":sparse}),
            );
        }
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        );
        claude_event(
            &mut reader,
            &mut output,
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"retained"}}),
        );
        append(
            &mut reader,
            &mut output,
            &json!({"type":"assistant","session_id":"session","message":{"id":"first","role":"assistant","content":[{"type":"text","text":"retained"}]}}),
        );
        assert_eq!(output, "");
        let expected = if reset {
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"message_start","message":{"id":"second","role":"assistant"}}),
            );
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            );
            claude_event(
                &mut reader,
                &mut output,
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"resumed"}}),
            );
            append(
                &mut reader,
                &mut output,
                &json!({"type":"assistant","session_id":"session","message":{"id":"second","role":"assistant","content":[{"type":"text","text":"resumed"}]}}),
            );
            assert_eq!(output, "resumed");
            "resumed"
        } else {
            "retained"
        };
        append(
            &mut reader,
            &mut output,
            &json!({"type":"result","subtype":"success","session_id":"session","result":expected}),
        );
        let end = Box::new(reader).end(ExitStatus {
            code: Some(0),
            signal: None,
        });
        assert_eq!(end.outcome, TurnOutcome::Completed);
        let reply = end.final_reply.unwrap();
        assert_eq!(reply.text, expected);
        assert_eq!(reply.completeness, FinalReplyCompleteness::Complete);
    }
}

#[test]
fn pi_block_limit_preserves_final_reply_and_resets_for_the_next_message() {
    for reset in [false, true] {
        let mut reader = pi::stream::TurnStream::new();
        let mut output = String::new();
        append(
            &mut reader,
            &mut output,
            &json!({"type":"session","id":"session"}),
        );
        append(
            &mut reader,
            &mut output,
            &json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
        );
        for index in 0..1000 {
            let kind = if index % 2 == 0 {
                "thinking_start"
            } else {
                "text_start"
            };
            let sparse = usize::MAX - index * 2;
            pi_event(
                &mut reader,
                &mut output,
                json!({"type":kind,"contentIndex":sparse}),
            );
        }
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_start","contentIndex":0}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_delta","contentIndex":0,"delta":"retained"}),
        );
        pi_event(
            &mut reader,
            &mut output,
            json!({"type":"text_end","contentIndex":0,"content":"retained"}),
        );
        append(
            &mut reader,
            &mut output,
            &json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"retained"}],"stopReason":"stop"}}),
        );
        assert_eq!(output, "");
        let expected = if reset {
            append(
                &mut reader,
                &mut output,
                &json!({"type":"message_start","message":{"role":"assistant","content":[]}}),
            );
            pi_event(
                &mut reader,
                &mut output,
                json!({"type":"text_start","contentIndex":0}),
            );
            pi_event(
                &mut reader,
                &mut output,
                json!({"type":"text_delta","contentIndex":0,"delta":"resumed"}),
            );
            append(
                &mut reader,
                &mut output,
                &json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"resumed"}],"stopReason":"stop"}}),
            );
            assert_eq!(output, "resumed");
            "resumed"
        } else {
            "retained"
        };
        append(&mut reader, &mut output, &json!({"type":"agent_settled"}));
        let end = Box::new(reader).end(ExitStatus {
            code: Some(0),
            signal: None,
        });
        assert_eq!(end.outcome, TurnOutcome::Completed);
        let reply = end.final_reply.unwrap();
        assert_eq!(reply.text, expected);
        assert_eq!(reply.completeness, FinalReplyCompleteness::Complete);
    }
}

#[test]
fn provider_limit_reports_only_nonempty_text_including_interleaved_excess_blocks() {
    let mut readers: Vec<Box<dyn TurnReader>> = vec![
        Box::new(claude::stream::TurnStream::new()),
        Box::new(pi::stream::TurnStream::new()),
    ];
    for (provider, reader) in readers.iter_mut().enumerate() {
        let header = if provider == 0 {
            json!({"type":"system","subtype":"init","session_id":"session"})
        } else {
            json!({"type":"session","id":"session"})
        };
        reader.line(&header.to_string());
        let start = if provider == 0 {
            json!({"type":"stream_event","session_id":"session","event":{"type":"message_start","message":{"id":"message","role":"assistant"}}})
        } else {
            json!({"type":"message_start","message":{"role":"assistant","content":[]}})
        };
        reader.line(&start.to_string());
        for index in 0..140 {
            let event = if provider == 0 {
                json!({"type":"stream_event","session_id":"session","event":{"type":"content_block_start","index":index,"content_block":{"type":"thinking"}}})
            } else {
                json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_start","contentIndex":index}})
            };
            assert_eq!(reader.line(&event.to_string()).omitted_text, None);
        }
        for index in [usize::MAX - 2, usize::MAX] {
            let event = if provider == 0 {
                json!({"type":"stream_event","session_id":"session","event":{"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}})
            } else {
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":index}})
            };
            assert_eq!(reader.line(&event.to_string()).omitted_text, None);
        }
        for text in ["", "omitted"] {
            let index = usize::MAX - 2;
            let event = if provider == 0 {
                json!({"type":"stream_event","session_id":"session","event":{"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":text}}})
            } else {
                json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":index,"delta":text}})
            };
            if provider == 0 {
                for (field, value) in [
                    ("session_id", json!("wrong")),
                    ("parent_tool_use_id", json!("tool")),
                    ("isSynthetic", json!(true)),
                ] {
                    let mut excluded = event.clone();
                    excluded[field] = value;
                    assert_eq!(reader.line(&excluded.to_string()).omitted_text, None);
                }
            }
            let read = reader.line(&event.to_string());
            assert!(read.text.is_empty());
            assert_eq!(
                read.omitted_text
                    .as_ref()
                    .map(dudes_types::SessionId::as_str),
                if text.is_empty() {
                    None
                } else {
                    Some("session")
                }
            );
        }
        let hidden = if provider == 0 {
            json!({"type":"stream_event","session_id":"session","event":{"type":"content_block_delta","index":usize::MAX,"delta":{"type":"thinking_delta","thinking":"hidden"}}})
        } else {
            json!({"type":"message_update","assistantMessageEvent":{"type":"thinking_delta","contentIndex":usize::MAX,"delta":"hidden"}})
        };
        assert_eq!(reader.line(&hidden.to_string()).omitted_text, None);
        let final_message = if provider == 0 {
            json!({"type":"assistant","session_id":"session","message":{"id":"message","role":"assistant","content":[{"type":"text","text":"complete text"}]}})
        } else {
            let mut content = vec![json!({"type":"thinking"}); 128];
            content.push(json!({"type":"text","text":"complete text"}));
            json!({"type":"message_end","message":{"role":"assistant","content":content,"stopReason":"stop"}})
        };
        assert_eq!(
            reader
                .line(&final_message.to_string())
                .omitted_text
                .as_ref()
                .map(dudes_types::SessionId::as_str),
            Some("session")
        );
    }
}
