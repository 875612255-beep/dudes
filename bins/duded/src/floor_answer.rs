//! The Floor's opt-in answer to one open question: its bounded request, its request
//! digest, the replayable answer index, and the finite outcomes the route may report.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context as _, Result, anyhow, bail};
use dudes_journal::Reader;
use dudes_proto::Record;
use dudes_proto::record::Hex32;
use dudes_types::{AgentId, Lsn};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Most UTF-8 bytes one answer carries after trimming: the inline-payload threshold, so every
/// browser answer fits whole in the next task.
pub const MAX_ANSWER_TEXT_BYTES: usize = 4_096;

/// Most bytes one answer request body may carry, checked before a byte of it is read. Four times
/// the text cap, so only a non-minimal encoding or an invalid text is ever refused by size.
pub const MAX_ANSWER_BODY_BYTES: usize = 16_384;

/// The `answer` sibling a Floor snapshot carries exactly when answering is enabled. Outside
/// `FloorProjection` and `floor.delta`, and discarded with the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AnswerView {
    max_text_bytes: usize,
}

/// The one value [`AnswerView`] takes.
pub const ANSWER_VIEW: AnswerView = AnswerView {
    max_text_bytes: MAX_ANSWER_TEXT_BYTES,
};

/// One canonical, bounded answer request. Nothing in it selects an effect beyond this answer.
#[derive(Clone)]
pub struct AnswerRequest {
    pub(crate) request_id: Hex32,
    pub(crate) request_wire: String,
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) question_lsn: Lsn,
    pub(crate) text: String,
}

impl fmt::Debug for AnswerRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AnswerRequest(<redacted>)")
    }
}

/// What an admitted answer request may truthfully report once the writer has run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerOutcome {
    /// This request's attributed answer is durable and indexed, including on a same-key retry.
    Answered {
        /// The request identity as the page sent it.
        request_id: String,
        /// The agent whose question it closed.
        agent: AgentId,
        /// The question it closed.
        question_lsn: Lsn,
    },
    /// No effect: the first reason, in a fixed order, that the pair names no open question
    /// of a live, unattached agent.
    Refused {
        /// The request identity as the page sent it.
        request_id: String,
        /// `agent_retired`, `agent_attached` or `question_closed`.
        reason: &'static str,
    },
}

/// An empty HTTP refusal with no invented identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerRefusal {
    /// The session expired or was revoked before the append permit; nothing was appended.
    Forbidden,
    /// The same request identity names a different request digest.
    Conflict,
    /// This request's own append may be durable; the same identity may be checked again later.
    Unavailable,
    /// This request appended nothing and may be retried at once with the same identity.
    UnavailableNoEffect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerJson {
    request_id: String,
    agent: String,
    question_lsn: String,
    text: String,
}

impl AnswerRequest {
    /// Decode one body.
    ///
    /// One flat object with exactly four string members: an unknown member is refused by
    /// `deny_unknown_fields`, a duplicate by the derived decoder, and a nested value by the string
    /// type of every member — all before any effect.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_ANSWER_BODY_BYTES {
            bail!("Floor answer request is too large");
        }
        // The derived decoder also reads a struct from a positional array; only an object is a
        // valid request body.
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            bail!("Floor answer request is not one JSON object");
        }
        let raw: AnswerJson =
            serde_json::from_slice(bytes).context("invalid Floor answer request JSON")?;
        let request_id = crate::floor_actions::decode_attempt(&raw.request_id)
            .context("invalid Floor answer request id")?;
        let agent =
            AgentId::parse(&raw.agent).map_err(|_| anyhow!("invalid Floor answer agent"))?;
        let question_lsn =
            decode_canonical_lsn(&raw.question_lsn).context("invalid Floor answer question_lsn")?;
        let text = raw
            .text
            .trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
            .to_owned();
        if !(1..=MAX_ANSWER_TEXT_BYTES).contains(&text.len()) {
            bail!("Floor answer text is outside its byte limit");
        }
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\t' | '\n'))
        {
            bail!("Floor answer text carries a control character");
        }
        Ok(Self {
            request_id,
            request_digest: request_digest(&raw.agent, &raw.question_lsn, &text),
            request_wire: raw.request_id,
            agent,
            question_lsn,
            text,
        })
    }
}

/// The canonical decimal of a journal position the page holds — a question's or a
/// control identity: 1–20 ASCII digits, no sign, no leading zero, at least 1 and at
/// most `u64::MAX`.
pub(crate) fn decode_canonical_lsn(value: &str) -> Result<Lsn> {
    if value.is_empty()
        || value.len() > 20
        || !value.bytes().all(|b| b.is_ascii_digit())
        || value.starts_with('0')
    {
        bail!("a Floor position is not a canonical positive decimal");
    }
    value
        .parse::<u64>()
        .map(Lsn)
        .context("a Floor position exceeds u64")
}

/// SHA-256 over `dudes.answer.v1\0`, then `agent`, `question_lsn` and the trimmed `text`, each
/// framed like every Floor request digest. The request identity does not enter it.
fn request_digest(agent: &str, question_lsn: &str, text: &str) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(b"dudes.answer.v1\0");
    for (field, value) in [
        ("agent", agent),
        ("question_lsn", question_lsn),
        ("text", text),
    ] {
        hasher.update(
            u16::try_from(field.len())
                .expect("static field name")
                .to_be_bytes(),
        );
        hasher.update(field.as_bytes());
        hasher.update(
            u32::try_from(value.len())
                .expect("bounded field")
                .to_be_bytes(),
        );
        hasher.update(value.as_bytes());
    }
    Hex32::new(hasher.finalize().into())
}

/// One durable browser answer, as the index keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IndexedAnswer {
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) question_lsn: Lsn,
}

/// The replayable answer index: request identity to what the durable answer said. Only journal
/// records may change it, and only after their fsync.
#[derive(Debug, Default)]
pub(crate) struct AnswerIndex {
    answers: BTreeMap<Hex32, IndexedAnswer>,
}

impl AnswerIndex {
    pub(crate) fn replay(reader: &Reader) -> Result<Self> {
        let mut index = Self::default();
        for item in reader
            .iter_from(Lsn::ZERO)
            .context("reading the Floor answer journal index")?
        {
            let (_, entry) = item.context("reading a Floor answer journal entry")?;
            index.apply(&entry.record)?;
        }
        Ok(index)
    }

    pub(crate) fn apply(&mut self, record: &Record) -> Result<()> {
        if let Record::HumanAnswer {
            agent,
            browser_answer: Some(attribution),
            ..
        }
        | Record::ConversationAnswerQueued {
            agent,
            authority:
                dudes_proto::ConversationAnswerAuthority::Browser {
                    browser: attribution,
                },
            ..
        } = record
        {
            if self.answers.contains_key(&attribution.request_id) {
                bail!("duplicate Floor answer request in journal");
            }
            self.answers.insert(
                attribution.request_id,
                IndexedAnswer {
                    request_digest: attribution.request_digest,
                    agent: *agent,
                    question_lsn: attribution.question_lsn,
                },
            );
        }
        Ok(())
    }

    pub(crate) fn get(&self, request_id: Hex32) -> Option<IndexedAnswer> {
        self.answers.get(&request_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const AGENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn body(question_lsn: &str, text: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "request_id": REQUEST_ID,
            "agent": AGENT,
            "question_lsn": question_lsn,
            "text": text,
        }))
        .unwrap()
    }

    /// The digest is pinned to one vector, over the trimmed text, and the request identity does
    /// not enter it.
    #[test]
    fn canonical_fields_have_one_domain_separated_request_digest() {
        let request = AnswerRequest::parse(&body("1099511628288", " main \n")).expect("canonical");
        assert_eq!(request.text, "main");
        assert_eq!(request.question_lsn, Lsn(1_099_511_628_288));
        assert_eq!(request.request_id.bytes(), [0; 32]);
        assert_eq!(
            serde_json::to_value(request.request_digest).unwrap(),
            // Recomputed outside Rust: Python's hashlib over the same framing.
            "9f3c995c90a7387c6c289665215cd2fe5dea896c1599e83c82824331050a4bc9"
        );
        let other_id = serde_json::to_vec(&serde_json::json!({
            "request_id": "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "agent": AGENT,
            "question_lsn": "1099511628288",
            "text": "main",
        }))
        .unwrap();
        assert_eq!(
            AnswerRequest::parse(&other_id).unwrap().request_digest,
            request.request_digest
        );
    }

    /// One flat object and nothing else: the derived decoder would read the same four values
    /// from a positional array, so the array is refused first.
    #[test]
    fn only_an_object_is_a_request() {
        let spaced = [b" \n".as_slice(), &body("1099511628288", "main")].concat();
        assert!(
            AnswerRequest::parse(&spaced).is_ok(),
            "answer: leading whitespace before the object"
        );
        let array = serde_json::to_vec(&serde_json::json!([
            REQUEST_ID,
            AGENT,
            "1099511628288",
            "main"
        ]))
        .unwrap();
        assert!(
            AnswerRequest::parse(&array).is_err(),
            "answer: admitted an array"
        );
    }

    #[test]
    fn question_lsn_is_a_canonical_positive_decimal() {
        for good in ["1", "18446744073709551615"] {
            assert!(decode_canonical_lsn(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "0",
            "01",
            "+1",
            "-1",
            "1.0",
            "18446744073709551616",
            "123456789012345678901",
        ] {
            assert!(decode_canonical_lsn(bad).is_err(), "{bad}");
        }
    }
}
