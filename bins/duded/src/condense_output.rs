use dudes_providers::claude::outcome::{TurnOutcome, turn_outcome};
use dudes_providers::claude::{StreamLine, parse_line};

pub(super) fn claude_summary(stdout: &[u8]) -> Option<String> {
    let stdout = std::str::from_utf8(stdout).ok()?;
    let envelope: serde_json::Value = serde_json::from_str(stdout).ok()?;
    if envelope.get("type")?.as_str()? != "result"
        || envelope.get("subtype")?.as_str()? != "success"
        || envelope.get("is_error")?.as_bool()?
    {
        return None;
    }
    let StreamLine::Result(result) = parse_line(stdout).ok()?.body else {
        return None;
    };
    if !matches!(
        turn_outcome(
            Some(&result),
            dudes_proto::ExitStatus {
                code: Some(0),
                signal: None,
            }
        ),
        TurnOutcome::Completed { .. }
    ) {
        return None;
    }
    let payload: serde_json::Value = serde_json::from_str(result.result.as_deref()?).ok()?;
    let text = payload.get("condensed")?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::claude_summary;

    #[test]
    fn only_a_successful_provider_result_can_supply_condense_text() {
        let inner = r#"{"condensed":"a bounded summary"}"#;
        let valid = serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":inner}).to_string();
        assert_eq!(
            claude_summary(valid.as_bytes()),
            Some("a bounded summary".to_owned())
        );
        for invalid in [
            inner.to_owned(),
            serde_json::json!({"result":inner}).to_string(),
            serde_json::json!({"type":"result","subtype":"error_during_execution","is_error":true,"result":inner}).to_string(),
            serde_json::json!({"type":"result","subtype":"success","is_error":true,"result":inner}).to_string(),
            serde_json::json!({"type":"result","subtype":"success","is_error":false,"terminal_reason":"api_error","result":inner}).to_string(),
            serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":format!("```json\n{inner}\n```")}).to_string(),
        ] {
            assert_eq!(claude_summary(invalid.as_bytes()), None, "provider framing and completion are required");
        }
    }

    #[test]
    fn invalid_bytes_missing_markers_and_multiple_frames_are_refused() {
        let inner = r#"{"condensed":"summary"}"#;
        let valid = serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":inner}).to_string();
        for invalid in [
            vec![0xff],
            Vec::new(),
            serde_json::json!({"type":"result","subtype":"success","result":inner}).to_string().into_bytes(),
            serde_json::json!({"type":"result","subtype":"success","is_error":null,"result":inner}).to_string().into_bytes(),
            serde_json::json!({"type":"assistant","subtype":"success","is_error":false,"result":inner}).to_string().into_bytes(),
            format!("{valid}\n{valid}").into_bytes(),
            format!("{valid} trailing diagnostics").into_bytes(),
            serde_json::json!([{ "type":"result","subtype":"success","is_error":false,"result":inner }]).to_string().into_bytes(),
        ] {
            assert_eq!(claude_summary(&invalid), None);
        }
    }

    #[test]
    fn terminal_text_is_not_the_managed_outcome_preview() {
        let summary = "x".repeat(1000);
        let output = serde_json::json!({"type":"result","subtype":"success","is_error":false,
            "result":serde_json::json!({"condensed":summary}).to_string()})
        .to_string();
        assert_eq!(claude_summary(output.as_bytes()), Some(summary));
    }
}
