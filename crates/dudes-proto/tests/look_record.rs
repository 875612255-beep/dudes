//! The authored appearance edit's journal tag and nullable wire fields.

use dudes_proto::{Record, RecordKind};
use dudes_types::{AccentColour, AgentId, CastCharacter};

#[test]
fn look_record_round_trips_explicit_automatic() {
    let id = AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    let record = Record::AgentLookChanged {
        id,
        role: "engineer".to_owned(),
        character: None,
        accent: Some(AccentColour::Sky),
        browser_look: None,
    };
    let encoded = serde_json::to_value(&record).unwrap();
    assert_eq!(encoded["t"], "agent_look_changed");
    assert_eq!(encoded["character"], serde_json::Value::Null);
    assert_eq!(encoded["accent"], "sky");
    assert_eq!(RecordKind::from_tag(45), Some(RecordKind::AgentLookChanged));
    assert_eq!(serde_json::from_value::<Record>(encoded).unwrap(), record);

    let authored = Record::AgentLookChanged {
        id,
        role: "designer".to_owned(),
        character: Some(CastCharacter::H),
        accent: None,
        browser_look: None,
    };
    assert_eq!(serde_json::to_value(authored).unwrap()["character"], "h");
}
