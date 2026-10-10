//! The Floor's opt-in retirement of one agent: the bounded request, its request
//! digest, the replayable retire index, and the finite outcomes the route may report.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context as _, Result, anyhow, bail};
use dudes_journal::Reader;
use dudes_proto::Record;
use dudes_proto::record::Hex32;
use dudes_types::{AgentId, Lsn};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Most bytes one retire request body may carry, checked before a byte of it is read. The
/// canonical body is 97 bytes.
pub const MAX_RETIRE_BODY_BYTES: usize = 512;

/// The fixed reason a browser retirement journals; the browser sends none.
pub(crate) const BROWSER_RETIRE_REASON: &str = "retired from the Floor";

/// The `retire` sibling a Floor snapshot carries exactly when retiring is enabled: an empty
/// object, outside `FloorProjection` and `floor.delta`, and discarded with the rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RetireView {}

/// The one value [`RetireView`] takes.
pub const RETIRE_VIEW: RetireView = RetireView {};

/// One canonical, bounded retire request. Nothing in it names a reason, a second agent or any
/// other effect.
#[derive(Clone)]
pub struct RetireRequest {
    pub(crate) request_id: Hex32,
    pub(crate) request_wire: String,
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
}

impl fmt::Debug for RetireRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetireRequest(<redacted>)")
    }
}

/// What an admitted retire request may truthfully report once the writer has run it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetireOutcome {
    /// This request's attributed `AgentRetired` is durable and indexed, on the first attempt or
    /// on a same-key retry. A durable retirement: no process exit or terminal close is promised.
    Retired {
        /// The request identity as the page sent it.
        request_id: String,
        /// The agent it retired.
        agent: AgentId,
        /// That record's position.
        retire_lsn: Lsn,
    },
    /// No effect: the first reason, in a fixed order, that holds.
    Refused {
        /// The request identity as the page sent it.
        request_id: String,
        /// `agent_absent`, `agent_halted` or `liveness_unknown`.
        reason: &'static str,
    },
}

/// An empty HTTP refusal with no invented identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetireRefusal {
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
struct RetireJson {
    request_id: String,
    agent: String,
}

impl RetireRequest {
    /// Decode one body.
    ///
    /// One flat object with exactly two string members: an unknown member is refused by
    /// `deny_unknown_fields`, a duplicate or a missing one by the derived decoder, and a nested
    /// value by the string type of both — all before any effect.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RETIRE_BODY_BYTES {
            bail!("Floor retire request is too large");
        }
        // The derived decoder also reads a struct from a positional array; only an object is a
        // valid request body.
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            bail!("Floor retire request is not one JSON object");
        }
        let raw: RetireJson =
            serde_json::from_slice(bytes).context("invalid Floor retire request JSON")?;
        let request_id = crate::floor_actions::decode_attempt(&raw.request_id)
            .context("invalid Floor retire request id")?;
        let agent =
            AgentId::parse(&raw.agent).map_err(|_| anyhow!("invalid Floor retire agent"))?;
        Ok(Self {
            request_id,
            request_digest: request_digest(&raw.agent),
            request_wire: raw.request_id,
            agent,
        })
    }
}

/// SHA-256 over `dudes.retire.v1\0`, then `agent`, framed like every Floor request digest. The
/// request identity does not enter it.
fn request_digest(agent: &str) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(b"dudes.retire.v1\0");
    hasher.update(
        u16::try_from("agent".len())
            .expect("static field name")
            .to_be_bytes(),
    );
    hasher.update(b"agent");
    hasher.update(
        u32::try_from(agent.len())
            .expect("bounded field")
            .to_be_bytes(),
    );
    hasher.update(agent.as_bytes());
    Hex32::new(hasher.finalize().into())
}

/// One durable browser retirement, as the index keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IndexedRetire {
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) lsn: Lsn,
}

/// The replayable retire index: request identity to what the durable record said. Only journal
/// records may change it, and only after their fsync.
#[derive(Debug, Default)]
pub(crate) struct RetireIndex {
    retirements: BTreeMap<Hex32, IndexedRetire>,
}

impl RetireIndex {
    pub(crate) fn replay(reader: &Reader) -> Result<Self> {
        let mut index = Self::default();
        for item in reader
            .iter_from(Lsn::ZERO)
            .context("reading the Floor retire journal index")?
        {
            let (lsn, entry) = item.context("reading a Floor retire journal entry")?;
            index.apply(lsn, &entry.record)?;
        }
        Ok(index)
    }

    pub(crate) fn apply(&mut self, lsn: Lsn, record: &Record) -> Result<()> {
        let Record::AgentRetired {
            id,
            browser_retire: Some(attribution),
            ..
        } = record
        else {
            return Ok(());
        };
        if self.retirements.contains_key(&attribution.request_id) {
            bail!("duplicate Floor retire request in journal");
        }
        self.retirements.insert(
            attribution.request_id,
            IndexedRetire {
                request_digest: attribution.request_digest,
                agent: *id,
                lsn,
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, request_id: Hex32) -> Option<IndexedRetire> {
        self.retirements.get(&request_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUEST_ID: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const AGENT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    fn body(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    /// The digest is pinned to a vector computed outside Rust (Python's hashlib over the same
    /// framing), and the request identity does not enter it. The canonical body is 97 bytes.
    #[test]
    fn canonical_fields_have_one_domain_separated_request_digest() {
        let canonical = body(&serde_json::json!({"request_id": REQUEST_ID, "agent": AGENT}));
        assert_eq!(canonical.len(), 97);
        let request = RetireRequest::parse(&canonical).expect("canonical");
        assert_eq!(request.request_id.bytes(), [0; 32]);
        assert_eq!(request.agent, AgentId::parse(AGENT).unwrap());
        assert_eq!(
            serde_json::to_value(request.request_digest).unwrap(),
            "fa31583bc102deaf0b39023c777b71cbaa7941c4011edba51b66f2841f84006b"
        );
        let other_id = body(&serde_json::json!({
            "request_id": "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "agent": AGENT,
        }));
        assert_eq!(
            RetireRequest::parse(&other_id).unwrap().request_digest,
            request.request_digest
        );
    }

    /// Exactly the two members, flat, as strings, in one object: anything else is refused before
    /// any effect. Control: leading whitespace before the canonical object decodes.
    #[test]
    fn only_the_two_member_object_is_a_request() {
        let spaced = [
            b" \n".as_slice(),
            &body(&serde_json::json!({"request_id": REQUEST_ID, "agent": AGENT})),
        ]
        .concat();
        assert!(RetireRequest::parse(&spaced).is_ok(), "control");
        for refused in [
            body(&serde_json::json!([REQUEST_ID, AGENT])),
            body(&serde_json::json!({"request_id": REQUEST_ID})),
            body(&serde_json::json!({"request_id": REQUEST_ID, "agent": AGENT, "reason": "x"})),
            body(&serde_json::json!({"request_id": REQUEST_ID, "agent": {"id": AGENT}})),
            format!(r#"{{"request_id":"{REQUEST_ID}","agent":"{AGENT}","agent":"{AGENT}"}}"#)
                .into_bytes(),
            body(&serde_json::json!({"request_id": REQUEST_ID, "agent": AGENT.to_lowercase()})),
            vec![b' '; MAX_RETIRE_BODY_BYTES + 1],
        ] {
            assert!(
                RetireRequest::parse(&refused).is_err(),
                "{}",
                String::from_utf8_lossy(&refused)
            );
        }
    }
}
