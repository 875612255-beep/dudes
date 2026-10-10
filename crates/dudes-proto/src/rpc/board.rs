//! `board.*` params and results. Every move carries `expected_seq`; a stale one is
//! `-32002 Conflict{current_seq}`.

use dudes_types::{AgentId, CardId, CardStatus, Seq};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::hook::HookToken;

/// `board.create` params.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardCreateParams {
    /// Title.
    pub title: String,
    /// Department whose agents may claim it.
    pub department: String,
    /// Cards that must be `Done` first.
    #[serde(default)]
    pub deps: Vec<CardId>,
}

/// `board.create` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CardCreateResult {
    /// The new card.
    pub card: CardId,
}

/// `board.claim` params.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardClaimParams {
    /// The card.
    pub card: CardId,
    /// The claimer. Sender-supplied only from an operator peer; on a call made with an agent
    /// token the server overwrites it with the token's owner (invariant 8), as `memory.read`
    /// does with its `agent`.
    pub agent: AgentId,
    /// The card's `seq` as last seen.
    pub expected_seq: Seq,
    /// The agent credential, presented per call; absent from an operator peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

/// `board.transition` params.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CardTransitionParams {
    /// The card.
    pub card: CardId,
    /// Where to.
    pub to: CardStatus,
    /// The card's `seq` as last seen.
    pub expected_seq: Seq,
    /// The agent credential, presented per call; absent from an operator peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

/// `board.list` params: every filter optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CardListParams {
    /// Only this department.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    /// Only this status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<CardStatus>,
    /// Only cards held by this agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<AgentId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::hook::HookToken;

    const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn card_claim_token_round_trips_and_debug_redacts() {
        let params = CardClaimParams {
            card: CardId::NIL,
            agent: AgentId::NIL,
            expected_seq: Seq(3),
            token: Some(HookToken::new(TOKEN).unwrap()),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["token"], TOKEN);
        let round_tripped: CardClaimParams = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, params);

        let debug = format!("{params:?}");
        assert!(debug.contains("HookToken(<redacted>)"), "{debug}");
        assert!(!debug.contains(TOKEN), "{debug}");

        let operator: CardClaimParams = serde_json::from_value(serde_json::json!({
            "card": CardId::NIL,
            "agent": AgentId::NIL,
            "expected_seq": 3
        }))
        .unwrap();
        assert_eq!(operator.token, None);
        assert!(
            serde_json::to_value(operator)
                .unwrap()
                .get("token")
                .is_none()
        );
    }

    #[test]
    fn card_transition_token_round_trips_and_debug_redacts() {
        let params = CardTransitionParams {
            card: CardId::NIL,
            to: CardStatus::Doing,
            expected_seq: Seq(3),
            token: Some(HookToken::new(TOKEN).unwrap()),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["token"], TOKEN);
        let round_tripped: CardTransitionParams = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, params);

        let debug = format!("{params:?}");
        assert!(debug.contains("HookToken(<redacted>)"), "{debug}");
        assert!(!debug.contains(TOKEN), "{debug}");

        let operator: CardTransitionParams = serde_json::from_value(serde_json::json!({
            "card": CardId::NIL,
            "to": "doing",
            "expected_seq": 3
        }))
        .unwrap();
        assert_eq!(operator.token, None);
        assert!(
            serde_json::to_value(operator)
                .unwrap()
                .get("token")
                .is_none()
        );
    }
}
