//! A fixed live kind rebuilt from real journal frames.

use dudes_journal::{Fsync, JournalOptions, Projections, Reader, open, replay};
use dudes_proto::{Entry, LiveActivityKind, Record};
use dudes_types::{AgentId, Timestamp};

#[test]
fn a_fresh_replay_rebuilds_the_same_current_kind_and_time() {
    let dir = tempfile::tempdir().unwrap();
    let journal = dir.path().join("journal");
    let (mut writer, _) = open(
        &journal,
        JournalOptions {
            fsync: Fsync::None,
            ..JournalOptions::default()
        },
    )
    .unwrap();
    let agent = AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    let entries = [
        Entry {
            at: Timestamp::from_unix_millis(1_000),
            record: serde_json::from_value(serde_json::json!({
                "t":"agent_hired", "id":agent, "name":"Ada", "role":"builder",
                "department":"engineering", "provider":"claude", "cwd":"/work",
                "budget":{}
            }))
            .unwrap(),
        },
        Entry {
            at: Timestamp::from_unix_millis(2_000),
            record: Record::LiveActivityObserved {
                agent,
                kind: LiveActivityKind::UsingTool,
            },
        },
    ];
    let mut live = Projections::in_memory().unwrap();
    for entry in &entries {
        let (lsn, crc) = writer.append_witnessed(entry).unwrap();
        live.apply(lsn, crc, entry).unwrap();
    }
    drop(writer);
    let mut rebuilt = Projections::in_memory().unwrap();
    assert_eq!(
        replay(&Reader::new(&journal), &mut rebuilt)
            .unwrap()
            .applied,
        2
    );
    assert_eq!(rebuilt.agent(agent).unwrap(), live.agent(agent).unwrap());
    let row = serde_json::to_value(rebuilt.agent(agent).unwrap().unwrap()).unwrap();
    assert_eq!(
        row["live_activity"],
        serde_json::json!({"kind":"using_tool", "at":2_000})
    );
    assert_eq!(
        replay(&Reader::new(&journal), &mut rebuilt)
            .unwrap()
            .applied,
        0
    );
}
