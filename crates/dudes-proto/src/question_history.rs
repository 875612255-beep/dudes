//! The finite answered-question read allowed on the authenticated browser stream.

use dudes_types::AgentId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum retained answered questions across current agents.
pub const MAX_QUESTION_HISTORY: usize = 64;
/// Maximum retained answered questions for one current agent.
pub const MAX_AGENT_QUESTION_HISTORY: usize = 8;
/// Maximum UTF-8 bytes of the complete serialized history array, including JSON escaping.
pub const MAX_QUESTION_HISTORY_BYTES: usize = 32_768;

/// A previously open question and the fact of its recorded answer. Answer bodies stay private.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnsweredQuestionView {
    /// The current agent whose question was answered.
    pub agent_id: AgentId,
    /// Canonical positive decimal position of the question record.
    pub question_lsn: String,
    /// Canonical positive decimal position of the answer record.
    pub answer_lsn: String,
    /// The question text, without any answer, task or attribution fields.
    pub question: String,
}
