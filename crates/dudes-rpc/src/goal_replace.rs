use dudes_proto::rpc::goal::{GoalReplaceParams, MAX_GOAL_REPLACE_BYTES};
use dudes_proto::rpc::{ErrorCode, RpcError};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use crate::dispatch::error;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope<T> {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: IgnoredAny,
    #[serde(rename = "id")]
    _id: IgnoredAny,
    #[serde(rename = "method")]
    _method: IgnoredAny,
    params: T,
    #[serde(rename = "v", default)]
    version: Option<u16>,
    #[serde(rename = "cred", default)]
    _credential: IgnoredAny,
}

pub(crate) fn decode(line: &str) -> Result<GoalReplaceParams, RpcError> {
    if line.len() > MAX_GOAL_REPLACE_BYTES {
        return Err(error(
            ErrorCode::InvalidParams,
            "goal.replace frame exceeds 128 KiB",
        ));
    }
    if line.bytes().find(|byte| !byte.is_ascii_whitespace()) != Some(b'{') {
        return Err(error(
            ErrorCode::InvalidRequest,
            "goal.replace requires an object envelope",
        ));
    }
    let envelope = serde_json::from_str::<Envelope<Option<Value>>>(line)
        .map_err(|_| error(ErrorCode::InvalidRequest, "invalid goal.replace envelope"))?;
    if envelope
        .version
        .is_some_and(|version| version != dudes_proto::CONTRACT_VERSION)
    {
        return Err(error(
            ErrorCode::InvalidRequest,
            "unsupported goal.replace contract version",
        ));
    }
    serde_json::from_str::<Envelope<GoalReplaceParams>>(line)
        .map(|frame| frame.params)
        .map_err(|_| error(ErrorCode::InvalidParams, "invalid goal.replace params"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{PeerProvenance, Requirement};
    use crate::dispatch::Dispatcher;
    use crate::outbound::Outbound;
    use crate::testing::AdmitAll;
    use dudes_proto::rpc::{Method, Request};
    use serde_json::json;
    use std::sync::Arc;

    fn frame() -> String {
        json!({"jsonrpc":"2.0","id":1,"method":"goal.replace","params":{
            "request_id":"0".repeat(64),"agent":"01ARZ3NDEKTSV4RRFFQ69G5FAV","expected_goal":"01ARZ3NDEKTSV4RRFFQ69G5FAW","text":"ship","predicate":{"command":{"argv":["true"],"timeout_s":1}},"check":{"id":"tests","label":"Tests"}}
        }).to_string()
    }

    #[test]
    fn original_frame_validation_reaches_typed_handler_and_blocks_value_only_calls() {
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::GoalReplace, Requirement::AnyRole, |call| {
            Ok(json!({"text":call.goal_replacement.expect("original params").text}))
        });
        let outbound = Arc::new(Outbound::sink());
        let line = frame();
        assert_eq!(
            dispatcher
                .handle_line(&line, PeerProvenance::Unresolvable, &outbound)
                .result
                .unwrap(),
            json!({"text":"ship"})
        );
        let request: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(
            dispatcher
                .dispatch(&request, PeerProvenance::Unresolvable, &outbound)
                .error
                .unwrap()
                .code,
            ErrorCode::InvalidParams.code()
        );
        for mutated in [
            line.replace("\"text\":", "\"te\\u0078t\":\"other\",\"text\":"),
            line.replace("\"argv\":", "\"argv\":[],\"argv\":"),
            line.replace("\"check\":{", "\"check\":{\"extra\":0,"),
        ] {
            assert_eq!(
                dispatcher
                    .handle_line(&mutated, PeerProvenance::Unresolvable, &outbound)
                    .error
                    .unwrap()
                    .code,
                ErrorCode::InvalidParams.code()
            );
        }
        for mutated in [
            line.replacen('{', "{\"extra\":0,", 1),
            line.replacen('{', "{\"id\":2,", 1),
        ] {
            assert_eq!(
                dispatcher
                    .handle_line(&mutated, PeerProvenance::Unresolvable, &outbound)
                    .error
                    .unwrap()
                    .code,
                ErrorCode::InvalidRequest.code()
            );
        }
    }

    #[test]
    fn replacement_admits_only_missing_or_current_contract_versions() {
        let line = frame();
        assert_eq!(decode(&line).unwrap().text, "ship");
        assert_eq!(
            decode(&line.replacen('{', "{\"v\":1,", 1)).unwrap().text,
            "ship"
        );
        assert_eq!(
            decode(&line.replacen('{', "{\"v\":2,", 1))
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest.code()
        );
        let mut dispatcher = Dispatcher::new(AdmitAll);
        dispatcher.register(Method::SystemPing, Requirement::AnyRole, |_| {
            Ok(json!({"legacy":true}))
        });
        let old = json!({"jsonrpc":"2.0","id":1,"method":"system.ping","v":2});
        assert_eq!(
            dispatcher
                .handle_line(
                    &old.to_string(),
                    PeerProvenance::Unresolvable,
                    &Arc::new(Outbound::sink())
                )
                .result
                .unwrap(),
            json!({"legacy":true})
        );
    }

    #[test]
    fn raw_frame_bound_counts_whitespace_and_escape_bytes() {
        let mut line = frame().replace("ship", "\\u0073\\u0068\\u0069\\u0070");
        line.extend(std::iter::repeat_n(
            ' ',
            MAX_GOAL_REPLACE_BYTES - line.len(),
        ));
        assert_eq!(line.len(), 131_072);
        assert_eq!(decode(&line).unwrap().text, "ship");
        line.push(' ');
        assert_eq!(
            decode(&line).unwrap_err().code,
            ErrorCode::InvalidParams.code()
        );
    }
}
