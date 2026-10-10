//! The authenticated team roster read.

use std::sync::Arc;

use dudes_proto::rpc::{Empty, ErrorCode, Method, RpcError};
use serde_json::Value;

use super::{TeamRoster, encode};
use crate::auth::PeerRole;
use crate::dispatch::{Call, error};

/// Read the roster using only the dispatcher-resolved caller.
pub fn roster<T: TeamRoster>(
    team: Arc<T>,
) -> impl Fn(Call<'_>) -> Result<Value, RpcError> + Send + Sync + 'static {
    move |call| {
        let caller = match call.role {
            PeerRole::Agent { agent } => Some(*agent),
            PeerRole::Operator => None,
            PeerRole::Unproven => {
                return Err(error(ErrorCode::Refused, "team.roster: unproven caller"));
            }
        };
        let mut value = call
            .params
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if caller.is_some()
            && let Some(object) = value.as_object_mut()
        {
            object.remove("token");
        }
        serde_json::from_value::<Empty>(value)
            .map_err(|err| error(ErrorCode::InvalidParams, format!("team.roster: {err}")))?;
        encode(Method::TeamRoster, &team.roster(caller)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeMintedTokens;
    use crate::{Dispatcher, Outbound, PeerClassifier, PeerProvenance};
    use dudes_proto::rpc::cred::OperatorCredential;
    use dudes_proto::rpc::{JSONRPC, Request, RequestId};
    use dudes_types::AgentId;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Read(Mutex<Vec<Option<AgentId>>>);
    impl TeamRoster for Read {
        fn roster(
            &self,
            caller: Option<AgentId>,
        ) -> Result<Vec<dudes_proto::rpc::team::RosterEntry>, RpcError> {
            self.0.lock().unwrap().push(caller);
            Ok(vec![])
        }
    }

    #[test]
    fn roster_authenticates_presentation_then_strictly_decodes_empty_business_params() {
        let token = "t".repeat(43);
        let credential = || OperatorCredential::new("C".repeat(43)).unwrap();
        let mut dispatcher = Dispatcher::new(PeerClassifier::new(
            credential(),
            Arc::new(FakeMintedTokens::minted(&[(&token, AgentId::NIL)])),
        ));
        let read = Arc::new(Read::default());
        super::super::register_team_roster(&mut dispatcher, &read);
        let outbound = Arc::new(Outbound::sink());
        let call = |params, cred, provenance| {
            dispatcher.dispatch(
                &Request {
                    jsonrpc: JSONRPC.into(),
                    id: RequestId::Number(1),
                    method: Method::TeamRoster.name().into(),
                    params: Some(params),
                    v: None,
                    cred,
                },
                provenance,
                &outbound,
            )
        };
        assert_eq!(
            call(
                serde_json::json!({"token":token}),
                None,
                PeerProvenance::Agent(AgentId::NIL)
            )
            .result,
            Some(serde_json::json!([]))
        );
        assert_eq!(
            call(
                serde_json::json!({"token":"malformed"}),
                Some(credential()),
                PeerProvenance::NoRecordedAgent
            )
            .error
            .unwrap()
            .code,
            ErrorCode::InvalidParams.code()
        );
        assert_eq!(
            call(
                serde_json::json!({"token":"x".repeat(43),"extra":true}),
                None,
                PeerProvenance::Agent(AgentId::NIL)
            )
            .error
            .unwrap()
            .code,
            ErrorCode::Refused.code()
        );
        assert_eq!(
            call(
                serde_json::json!({}),
                Some(credential()),
                PeerProvenance::NoRecordedAgent
            )
            .result,
            Some(serde_json::json!([]))
        );
        for params in [
            serde_json::json!({"token":token,"agent":AgentId::NIL}),
            serde_json::json!({"token":token,"caller":AgentId::NIL}),
            serde_json::json!({"token":token,"extra":true}),
            serde_json::json!([token]),
        ] {
            let (cred, provenance) = if params.is_array() {
                (Some(credential()), PeerProvenance::NoRecordedAgent)
            } else {
                (None, PeerProvenance::Agent(AgentId::NIL))
            };
            assert_eq!(
                call(params, cred, provenance).error.unwrap().code,
                ErrorCode::InvalidParams.code()
            );
        }
        for params in [
            serde_json::json!({"token":"forged","extra":true}),
            serde_json::json!({"extra":true}),
        ] {
            assert_eq!(
                call(params, None, PeerProvenance::Agent(AgentId::NIL))
                    .error
                    .unwrap()
                    .code,
                ErrorCode::Unauthorized.code()
            );
        }
        assert_eq!(*read.0.lock().unwrap(), [Some(AgentId::NIL), None]);
    }
}
