//! `memory.*` params and results. An agent appends only to its own memory (agent token); the
//! operator names the target it appends to or reads.

use dudes_types::{AgentId, Lsn};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::hook::HookToken;

/// `memory.append` params. An agent caller omits `agent` and derives itself from its token; an
/// operator caller must name the target.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemoryAppendParams {
    /// Target for an operator call. An agent call must omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// The text to append.
    pub text: String,
    /// The agent credential, presented per call; absent from an operator peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

/// `memory.append` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAppendResult {
    /// Where the `MemoryAppend` record landed.
    pub lsn: Lsn,
}

/// `memory.read` params.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MemoryReadParams {
    /// Whose memory. Required from an operator peer; ignored for a proven agent caller, which
    /// always reads its own memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// Only appends after this lsn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_lsn: Option<Lsn>,
    /// The agent credential, presented per call; absent from an operator peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

/// `memory.read` result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemoryReadResult {
    /// The text.
    pub text: String,
    /// The lsn of the last append included.
    pub lsn: Lsn,
}

/// `memory.search` params. Matching is literal, case-sensitive UTF-8 substring matching.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MemorySearchParams {
    /// A query of 1 through 1,024 UTF-8 bytes, without U+0000.
    pub query: String,
    /// Required for operators. Agents may omit it or name themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentId>,
    /// Search candidates strictly after this journal position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_lsn: Option<Lsn>,
    /// Maximum hits, 1 through 100; absent means 10. Byte limits can return fewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Per-call agent credential, absent from an operator peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<HookToken>,
}

/// Validated matching inputs, carried without credentials to the loop and projection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemorySearchOptions {
    query: String,
    since_lsn: Option<Lsn>,
    limit: u8,
}

/// Why a search input cannot be admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemorySearchInputError {
    /// Query length is outside 1 through 1,024 UTF-8 bytes.
    QueryLength,
    /// Query contains U+0000.
    Nul,
    /// Hit limit is outside 1 through 100.
    Limit,
}

impl std::fmt::Display for MemorySearchInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::QueryLength => "query must contain 1 through 1,024 UTF-8 bytes",
            Self::Nul => "query must not contain U+0000",
            Self::Limit => "limit must be 1 through 100",
        })
    }
}

impl std::error::Error for MemorySearchInputError {}

impl MemorySearchOptions {
    /// Validate matching inputs once before handing them to a search port.
    ///
    /// # Errors
    /// Rejects an empty, overlong or NUL-bearing query, or a hit limit outside 1 through 100.
    pub fn new(
        query: String,
        since_lsn: Option<Lsn>,
        limit: Option<u64>,
    ) -> Result<Self, MemorySearchInputError> {
        if !(1..=1_024).contains(&query.len()) {
            return Err(MemorySearchInputError::QueryLength);
        }
        if query.contains('\0') {
            return Err(MemorySearchInputError::Nul);
        }
        let limit = limit.unwrap_or(10);
        if !(1..=100).contains(&limit) {
            return Err(MemorySearchInputError::Limit);
        }
        Ok(Self {
            query,
            since_lsn,
            limit: u8::try_from(limit).map_err(|_| MemorySearchInputError::Limit)?,
        })
    }

    /// Exact query, including all whitespace.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Exclusive candidate cursor.
    #[must_use]
    pub const fn since_lsn(&self) -> Option<Lsn> {
        self.since_lsn
    }

    /// Maximum returned hits.
    #[must_use]
    pub fn limit(&self) -> usize {
        usize::from(self.limit)
    }
}

/// One matching original note or current rendered summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum MemorySearchHit {
    /// A bounded excerpt of one valid original append.
    Raw {
        /// Original append position.
        lsn: Lsn,
        /// First match's byte offset in the complete original text.
        match_offset: usize,
        /// Exact UTF-8 excerpt, at most 1,024 bytes, containing the whole first match.
        text: String,
        /// Excerpt's starting byte offset in the original text.
        text_offset: usize,
        /// Complete original text's byte length.
        source_bytes: usize,
        /// Whether the excerpt omits original bytes.
        truncated: bool,
        /// Exclusive raw-read cursor that starts with this append.
        read_since_lsn: Lsn,
    },
    /// Complete current validated summary, separately labelled from original notes.
    Summary {
        /// Condense record position, not a raw-read cursor.
        lsn: Lsn,
        /// First match's byte offset in the rendered summary.
        match_offset: usize,
        /// Whole rendered summary, at most 8,192 bytes.
        text: String,
        /// Zero: summary text is returned whole.
        text_offset: usize,
        /// Complete rendered summary's byte length.
        source_bytes: usize,
        /// False: summary text is returned whole.
        truncated: bool,
        /// Cursor starting the original covered range; null means read from the start.
        read_since_lsn: Option<Lsn>,
        /// First original position represented by this summary.
        covered_from_lsn: Lsn,
        /// Last original position represented by this summary.
        covered_to_lsn: Lsn,
    },
}

/// One bounded live search page. The canonical JSON result is at most 65,536 bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchResult {
    /// At most the requested number of matching candidates, in increasing journal order.
    pub hits: Vec<MemorySearchHit>,
    /// Last consumed candidate when more remain; null means exhausted at this call's state.
    pub next_since_lsn: Option<Lsn>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::hook::HookToken;

    const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn memory_append_token_round_trips_and_debug_redacts() {
        let params = MemoryAppendParams {
            agent: Some(AgentId::NIL),
            text: "learned something".to_owned(),
            token: Some(HookToken::new(TOKEN).unwrap()),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["token"], TOKEN);
        let round_tripped: MemoryAppendParams = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, params);

        let debug = format!("{params:?}");
        assert!(debug.contains("HookToken(<redacted>)"), "{debug}");
        assert!(!debug.contains(TOKEN), "{debug}");

        let operator: MemoryAppendParams =
            serde_json::from_value(serde_json::json!({"text": "operator memory"})).unwrap();
        assert_eq!(operator.agent, None);
        assert_eq!(operator.token, None);
        assert!(
            serde_json::to_value(operator)
                .unwrap()
                .get("token")
                .is_none()
        );
    }

    #[test]
    fn memory_read_token_round_trips_and_debug_redacts() {
        let params = MemoryReadParams {
            agent: Some(AgentId::NIL),
            since_lsn: Some(Lsn(7)),
            token: Some(HookToken::new(TOKEN).unwrap()),
        };

        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["token"], TOKEN);
        let round_tripped: MemoryReadParams = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, params);

        let debug = format!("{params:?}");
        assert!(debug.contains("HookToken(<redacted>)"), "{debug}");
        assert!(!debug.contains(TOKEN), "{debug}");

        let operator: MemoryReadParams =
            serde_json::from_value(serde_json::json!({"agent": AgentId::NIL})).unwrap();
        assert_eq!(operator.token, None);
        assert!(
            serde_json::to_value(operator)
                .unwrap()
                .get("token")
                .is_none()
        );
    }

    #[test]
    fn search_options_preserve_literal_bytes_and_validate_boundaries() {
        let options = MemorySearchOptions::new(" \t🦀 ".into(), Some(Lsn(u64::MAX)), None).unwrap();
        assert_eq!(options.query(), " \t🦀 ");
        assert_eq!(options.since_lsn(), Some(Lsn(u64::MAX)));
        assert_eq!(options.limit(), 10);
        assert_eq!(
            MemorySearchOptions::new("🦀".repeat(256), None, Some(100))
                .unwrap()
                .query()
                .len(),
            1024
        );
        assert!(matches!(
            MemorySearchOptions::new(String::new(), None, None),
            Err(MemorySearchInputError::QueryLength)
        ));
        assert!(matches!(
            MemorySearchOptions::new("x".repeat(1025), None, None),
            Err(MemorySearchInputError::QueryLength)
        ));
        assert!(matches!(
            MemorySearchOptions::new("x\0y".into(), None, None),
            Err(MemorySearchInputError::Nul)
        ));
        for limit in [0, 101, u64::MAX] {
            assert!(matches!(
                MemorySearchOptions::new("x".into(), None, Some(limit)),
                Err(MemorySearchInputError::Limit)
            ));
        }
    }

    #[test]
    fn search_params_require_query_deny_unknown_fields_and_redact_token() {
        let params: MemorySearchParams =
            serde_json::from_value(serde_json::json!({"query":"literal", "token":TOKEN})).unwrap();
        assert!(!format!("{params:?}").contains(TOKEN));
        assert_eq!(params.query, "literal");
        assert!(params.limit.is_none());
        assert!(params.agent.is_none());
        assert!(serde_json::from_value::<MemorySearchParams>(serde_json::json!({})).is_err());
        assert!(
            serde_json::from_value::<MemorySearchParams>(
                serde_json::json!({"query":"x", "typo":1})
            )
            .is_err()
        );
    }

    #[test]
    fn summary_read_cursor_null_and_raw_read_cursor_zero_stay_distinct() {
        let hits = [
            MemorySearchHit::Summary {
                lsn: Lsn(3),
                match_offset: 0,
                text: "x".into(),
                text_offset: 0,
                source_bytes: 1,
                truncated: false,
                read_since_lsn: None,
                covered_from_lsn: Lsn(0),
                covered_to_lsn: Lsn(2),
            },
            MemorySearchHit::Raw {
                lsn: Lsn(1),
                match_offset: 0,
                text: "x".into(),
                text_offset: 0,
                source_bytes: 1,
                truncated: false,
                read_since_lsn: Lsn(0),
            },
        ];
        let encoded = serde_json::to_value(&hits).unwrap();
        assert_eq!(encoded[0]["read_since_lsn"], serde_json::Value::Null);
        assert_eq!(encoded[1]["read_since_lsn"], 0);
        assert_eq!(
            serde_json::from_value::<Vec<MemorySearchHit>>(encoded).unwrap(),
            hits
        );
    }
}
