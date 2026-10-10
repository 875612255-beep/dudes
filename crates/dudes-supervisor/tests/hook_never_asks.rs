//! **No hook event is an act of asking.** Dudes refuses a
//! vendor `Notification`, an idle prompt, a permission prompt or any classification of the model's
//! prose as a human question, and removed the `on_hook` branch that turned one into a
//! `Record::HumanQuestion`. This file is the check that the branch stays removed.
//!
//! **Do not delete this as redundant with `tests/fsm.rs`.** That file proves the *positive*: an
//! `agent.ask` reaches gate 3 and holds the agent that asked. This one proves the *negative*, and
//! the two fail for opposite reasons. Deleting
//! this file returns that property to being asserted by nobody.
//!
//! **The route it closes is the one a grep misses**. `EventKind` derives
//! `Deserialize` with `#[serde(tag = "kind", rename_all = "snake_case")]`, so
//! `{"kind":"human_question","text":"…"}` in a `hook.event` payload is a well-formed `EventKind`
//! the instant any adapter deserialises vendor JSON straight into one. No source-text search sees
//! that construction, because there is no `EventKind::HumanQuestion` token anywhere in it. What
//! this test asserts is the property the search was standing in for: whatever an adapter hands the
//! machine as an `Input::Hook`, no `Record::HumanQuestion` comes out — so a second, weaker emitter
//! cannot appear beside `Command::Ask` and bypass it.

use std::collections::BTreeSet;

use dudes_proto::{Event, EventKind, Record};
use dudes_supervisor::testing::{Harness, Seen};
use dudes_supervisor::{ClientId, Command, HookId, Input};
use dudes_types::{Provenance, Timestamp, Usage};

/// One value per `EventKind` variant, each carrying the payload that would matter if the machine
/// read it.
///
/// The list is hand-written and [`kinds_are_every_kind_in_the_contract`] is what keeps it total —
/// see that assertion for why an exhaustive `match` beside this vector would not.
fn every_event_kind() -> Vec<EventKind> {
    // Built from JSON rather than constructed, because the construction is the subject: this is
    // the exact `hook.event` payload for `agent.ask`, and that it deserialises at all is
    // the reason a grep cannot carry this property.
    let bypass: EventKind = serde_json::from_value(serde_json::json!({
        "kind": "human_question",
        "text": "may I force-push to main?",
    }))
    .expect("a vendor payload naming the tag deserialises into an EventKind — that is the route");

    vec![
        EventKind::SessionStart,
        EventKind::SessionEnd,
        EventKind::PromptSubmit,
        EventKind::PreTool {
            name: "Bash".into(),
            input: serde_json::json!({"command": "git push --force"}),
        },
        EventKind::PostTool {
            name: "Bash".into(),
            ok: false,
        },
        // An act of asking, in the field the vendor puts prose in: a question mark and a request for
        // approval, which is precisely what a classifier would call an ask.
        EventKind::TurnEnd {
            last_message: Some("should I force-push to main, or open a PR?".into()),
        },
        EventKind::SubagentEnd,
        // Another act of asking, named: the vendor's event means *a prompt is on screen*, which is a
        // different proposition from *this agent needs a person*.
        EventKind::Notification {
            text: "the agent needs your permission to use Bash".into(),
        },
        EventKind::CompactStart,
        EventKind::CompactEnd,
        EventKind::Usage {
            usage: Usage::default(),
        },
        bypass,
    ]
}

/// The `kind` tag a variant serialises to.
fn tag_of(kind: &EventKind) -> String {
    serde_json::to_value(kind).expect("an EventKind serialises")["kind"]
        .as_str()
        .expect("EventKind is tagged on `kind`")
        .to_owned()
}

/// Every tag the contract declares, read out of the schema `dudes-proto` generates from the type.
fn tags_in_the_contract() -> BTreeSet<String> {
    let event = dudes_proto::schema::schemas()
        .into_iter()
        .find(|doc| doc.file_name == "event.schema.json")
        .expect("the contract exports an event schema")
        .schema;
    let variants = event["oneOf"]
        .as_array()
        .expect("`EventKind` is flattened into `Event` as a tagged union")
        .clone();
    assert!(
        variants.len() > 1,
        "control: the schema still expresses EventKind as a union of variants, so an empty set \
         below would be a shape change and not a true reading"
    );
    variants
        .iter()
        .map(|variant| {
            variant["properties"]["kind"]["const"]
                .as_str()
                .expect("each variant fixes its own tag")
                .to_owned()
        })
        .collect()
}

/// Every `Record::HumanQuestion` the machine asked to have written.
fn questions(harness: &Harness) -> usize {
    harness
        .records()
        .filter(|record| matches!(record, Record::HumanQuestion { .. }))
        .count()
}

/// Every `agent.question` notification the machine emitted — the other half of what the removed
/// branch did, since it persisted with `Then::Question`.
fn question_notifications(harness: &Harness) -> usize {
    harness
        .trace
        .iter()
        .filter(|seen| matches!(seen, Seen::Notify { method, .. } if method == "agent.question"))
        .count()
}

/// **"any `EventKind`" is derived from the type, not from a list somebody remembers to extend.**
///
/// A hand-written vector with an exhaustive `match` beside it would not hold: the match compiles
/// again the moment its new arm is added, whether or not the vector grew, and the new variant then
/// goes through no hook door at all. The schema is generated from `EventKind`'s own `JsonSchema`
/// derive, so comparing the two sets is a structural check — add, rename or remove a variant and
/// this goes red until [`every_event_kind`] is brought back into line.
#[test]
fn kinds_are_every_kind_in_the_contract() {
    let driven: BTreeSet<String> = every_event_kind().iter().map(tag_of).collect();
    assert_eq!(
        driven,
        tags_in_the_contract(),
        "the kinds driven through the hook door are no longer every kind the contract declares; \
         add the new variant to `every_event_kind` so the test keeps covering all of them"
    );
}

/// No `Input::Hook`, of any `EventKind`, produces a `Record::HumanQuestion`.
///
/// Every variant is driven at a live agent — including one deserialised from the vendor payload of
/// the vendor payload for `agent.ask`, and every event carries that same payload in `raw` as well, so the bypass
/// shape is present both as the parsed kind and as the untranslated JSON beside it.
///
/// **Two controls, because the claim is an absence.** Each hook is asserted to have been answered,
/// so a hook the machine dropped on the floor cannot satisfy the zeros for the wrong reason; and
/// after the zeros, `Command::Ask` — the one producer (`machine.rs`) — is driven through the
/// same harness and the same two counters are asserted to find it. Without that second control a
/// renamed record variant or a broken filter would read as "no bypass".
///
/// It does not assert the agent's status. Gate 3 reads the open question off the record, so
/// `Waiting` is downstream of the count above; that the record holds the agent that wrote it is
/// `tests/fsm.rs`'s `a_human_question_waits_and_an_answer_releases`.
#[test]
fn no_hook_event_of_any_kind_writes_a_human_question() {
    let mut harness = Harness::new();
    harness.drive(Input::Command(Command::Subscribe {
        client: ClientId(1),
    }));
    let (agent, _goal) = harness.hire_with_goal(&["npm", "test"]);
    assert!(
        harness.open_turn(agent).is_some(),
        "control: the agent is at work, so these are hooks from a live turn and not from a \
         quiescent floor that would ignore them"
    );

    let mut hook_id = 0_u64;
    for kind in every_event_kind() {
        hook_id += 1;
        let hook = HookId(hook_id);
        let tag = tag_of(&kind);
        harness.drive(Input::Hook {
            hook,
            event: Event {
                agent,
                at: Timestamp::from_unix_millis(1),
                provenance: Provenance::Observed,
                kind,
                raw: Some(serde_json::json!({
                    "kind": "human_question",
                    "text": "may I force-push to main?",
                })),
            },
        });
        assert!(
            harness.trace.iter().any(
                |seen| matches!(seen, Seen::Reply { hook: answered, .. } if *answered == hook)
            ),
            "control: the {tag} hook was answered, so the machine did read it"
        );
    }

    assert_eq!(
        questions(&harness),
        0,
        "no hook event is an act of asking, so none of these wrote a \
         `Record::HumanQuestion`"
    );
    assert_eq!(
        question_notifications(&harness),
        0,
        "and no subscriber was told an agent had asked"
    );

    harness.drive(Input::Command(Command::Ask {
        agent,
        text: "which branch?".into(),
    }));
    assert_eq!(
        questions(&harness),
        1,
        "positive control: `Command::Ask` still writes one, so the zero above is an absence and \
         not a filter that matches nothing"
    );
    assert_eq!(
        question_notifications(&harness),
        1,
        "positive control: the notification the removed branch also emitted is reachable, so its \
         zero above is an absence too"
    );
}
