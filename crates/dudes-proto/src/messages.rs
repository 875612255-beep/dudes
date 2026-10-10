//! The bounded agent-to-agent metadata permitted on the Floor stream.

use dudes_types::{AgentId, MailId, ThreadId, Timestamp};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) const POSITIVE_ORDER_PATTERN: &str = "^([1-9][0-9]{0,18}|1[0-7][0-9]{18}|18[0-3][0-9]{17}|184[0-3][0-9]{16}|1844[0-5][0-9]{15}|18446[0-6][0-9]{14}|184467[0-3][0-9]{13}|1844674[0-3][0-9]{12}|184467440[0-6][0-9]{10}|1844674407[0-2][0-9]{9}|18446744073[0-6][0-9]{8}|1844674407370[0-8][0-9]{6}|18446744073709[0-4][0-9]{5}|184467440737095[0-4][0-9]{4}|18446744073709550[0-9]{3}|18446744073709551[0-5][0-9]{2}|1844674407370955160[0-9]|1844674407370955161[0-4]|18446744073709551615)$";

/// Maximum rows in the complete Messages window.
pub const MAX_MESSAGES: usize = 128;
/// Maximum bytes of the complete serialized Messages array, including JSON escaping.
pub const MAX_MESSAGES_BYTES: usize = 65_536;

/// Maximum rows in the complete live thread graph.
pub const MAX_THREADS: usize = 128;
/// Maximum bytes of the complete serialized thread array.
pub const MAX_THREADS_BYTES: usize = 65_536;

/// Fixed membership of a thread with at least one visible message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadView {
    /// The journaled conversation identity.
    pub id: ThreadId,
    /// The opener's distinct members in canonical id order.
    #[schemars(length(min = 2, max = 8))]
    pub members: Vec<AgentId>,
}

/// A content-free delivery label with no notification count or bounce reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageState {
    /// The daemon accepted the mail durably.
    Durable,
    /// The recipient was notified.
    Notified,
    /// The recipient acknowledged the mail.
    Acked,
    /// The mail could not be delivered.
    Bounced,
}

/// Original journal metadata and the current fixed delivery label for agent mail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageView {
    /// The sender's mail identity.
    pub id: MailId,
    /// The structural sending agent.
    pub from: AgentId,
    /// The addressed agent.
    pub to: AgentId,
    /// The original Mail entry's journal acceptance time.
    pub at: Timestamp,
    /// The original Mail entry's creation order as canonical decimal text.
    #[schemars(regex(pattern = POSITIVE_ORDER_PATTERN))]
    pub lsn: String,
    /// The most recently recorded delivery state.
    pub state: MessageState,
    /// The journaled thread, absent for legacy messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<ThreadId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_mail_pair_fits_the_existing_compound_reserve() {
        let row = MessageView {
            id: MailId::from_parts(u64::MAX, u128::MAX),
            from: AgentId::from_parts(u64::MAX, u128::MAX),
            to: AgentId::from_parts(u64::MAX, u128::MAX - 1),
            at: Timestamp::from_unix_millis(i64::MIN),
            lsn: u64::MAX.to_string(),
            state: MessageState::Notified,
            thread: Some(ThreadId::from_parts(u64::MAX, u128::MAX)),
        };
        let thread = ThreadView {
            id: row.thread.unwrap(),
            members: (0..8)
                .rev()
                .map(|n| AgentId::from_parts(u64::MAX, u128::MAX - n))
                .collect(),
        };
        assert_eq!(serde_json::to_vec(&row).unwrap().len(), 217);
        assert_eq!(serde_json::to_vec(&thread).unwrap().len(), 279);
        let messages = vec![row; MAX_MESSAGES];
        let threads = vec![thread; MAX_THREADS];
        let message_bytes = serde_json::to_vec(&messages).unwrap().len();
        let thread_bytes = serde_json::to_vec(&threads).unwrap().len();
        assert_eq!((message_bytes, thread_bytes), (27_905, 35_841));
        assert_eq!(message_bytes + thread_bytes, 63_746);
        assert!(message_bytes + thread_bytes <= MAX_MESSAGES_BYTES);
        let pair = serde_json::json!({"messages":messages, "messages_lsn":u64::MAX.to_string(),
            "threads":threads, "threads_lsn":u64::MAX.to_string()});
        let legacy_pair = serde_json::json!({"messages":[], "messages_lsn":u64::MAX.to_string()});
        let empty_pair = serde_json::json!({"messages":[], "messages_lsn":u64::MAX.to_string(),
            "threads":[], "threads_lsn":u64::MAX.to_string()});
        assert_eq!(
            serde_json::to_vec(&empty_pair).unwrap().len()
                - serde_json::to_vec(&legacy_pair).unwrap().len(),
            50
        );
        assert_eq!(
            serde_json::to_vec(&pair).unwrap().len()
                - serde_json::to_vec(&empty_pair).unwrap().len(),
            63_742
        );
    }
}
