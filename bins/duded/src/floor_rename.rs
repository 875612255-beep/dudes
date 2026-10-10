//! The bounded browser rename and its journal-derived retry truth.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context as _, Result, anyhow, bail};
use dudes_journal::Reader;
use dudes_proto::Record;
use dudes_proto::record::Hex32;
use dudes_types::{AgentId, DisplayName, Lsn};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// Maximum encoded body, including JSON escaping of a 128-byte name.
pub const MAX_RENAME_BODY_BYTES: usize = 2_048;

/// Canonical rename fields admitted before the writer receives them.
#[derive(Clone)]
pub struct RenameRequest {
    pub(crate) request_id: Hex32,
    pub(crate) request_wire: String,
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) name: String,
}

impl fmt::Debug for RenameRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RenameRequest(<redacted>)")
    }
}

/// What the durable rename path can report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenameOutcome {
    /// This request's original rename is durable and indexed.
    Changed {
        /// Browser request identity.
        request_id: String,
        /// Structural identity, unchanged.
        agent: AgentId,
        /// Display name recorded by this request, even after a later rename.
        name: String,
    },
    /// No record was appended.
    Refused {
        /// Browser request identity.
        request_id: String,
        /// The missing or retired agent is `agent_absent`.
        reason: &'static str,
    },
}

/// An empty HTTP refusal with no invented outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameRefusal {
    /// The admitting session no longer authorizes append.
    Forbidden,
    /// This request identity already names different canonical fields.
    Conflict,
    /// Durability is uncertain. Keep this exact attempt.
    Unavailable,
    /// Nothing was appended by this request.
    UnavailableNoEffect,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameJson {
    request_id: String,
    agent: String,
    name: String,
}

impl RenameRequest {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_RENAME_BODY_BYTES {
            bail!("Floor rename request is too large");
        }
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            bail!("Floor rename request is not one JSON object");
        }
        let raw: RenameJson = serde_json::from_slice(bytes).context("invalid Floor rename JSON")?;
        let request_id = crate::floor_actions::decode_attempt(&raw.request_id)
            .context("invalid Floor rename request id")?;
        let agent =
            AgentId::parse(&raw.agent).map_err(|_| anyhow!("invalid Floor rename agent"))?;
        let name = DisplayName::parse(&raw.name)
            .context("invalid Floor rename name")?
            .into_string();
        let mut hasher = Sha256::new();
        hasher.update(b"dudes.rename.v1\0");
        for (field, value) in [("agent", raw.agent.as_str()), ("name", name.as_str())] {
            hasher.update(
                u16::try_from(field.len())
                    .expect("static field")
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
        Ok(Self {
            request_id,
            request_wire: raw.request_id,
            request_digest: Hex32::new(hasher.finalize().into()),
            agent,
            name,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedRename {
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) name: String,
}

#[derive(Debug, Default)]
pub(crate) struct DurableRenameIndex {
    changes: BTreeMap<Hex32, IndexedRename>,
}

impl DurableRenameIndex {
    pub(crate) fn replay(reader: &Reader) -> Result<Self> {
        let mut index = Self::default();
        for item in reader
            .iter_from(Lsn::ZERO)
            .context("reading Floor rename index")?
        {
            let (_, entry) = item.context("reading Floor rename journal entry")?;
            index.apply(&entry.record)?;
        }
        Ok(index)
    }

    pub(crate) fn apply(&mut self, record: &Record) -> Result<()> {
        let Record::AgentRenamed {
            id,
            name,
            browser_rename: Some(meta),
        } = record
        else {
            return Ok(());
        };
        if self.changes.contains_key(&meta.request_id) {
            bail!("duplicate Floor rename request in journal");
        }
        self.changes.insert(
            meta.request_id,
            IndexedRename {
                request_digest: meta.request_digest,
                agent: *id,
                name: name.clone(),
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, id: Hex32) -> Option<&IndexedRename> {
        self.changes.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(name: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "request_id": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "agent": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "name": name,
        }))
        .unwrap()
    }
    #[test]
    fn strict_fields_and_canonical_name() {
        let parsed = RenameRequest::parse(&body(" \tKim\r\n")).unwrap();
        assert_eq!(parsed.name, "Kim");
        assert_eq!(
            parsed.request_digest,
            RenameRequest::parse(&body("Kim")).unwrap().request_digest
        );
        assert!(RenameRequest::parse(&body(&"é".repeat(64))).is_ok());
        assert!(RenameRequest::parse(&body(&"é".repeat(65))).is_err());
        assert!(RenameRequest::parse(&body(" \r\n")).is_err());
        for bytes in [
            br#"["AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","01ARZ3NDEKTSV4RRFFQ69G5FAV","Kim"]"#.to_vec(),
            br#"{"request_id":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","agent":"01ARZ3NDEKTSV4RRFFQ69G5FAV","name":"Kim","name":"Leo"}"#.to_vec(),
            br#"{"request_id":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","agent":"01ARZ3NDEKTSV4RRFFQ69G5FAV","name":"Kim","token":"secret"}"#.to_vec(),
        ] {
            assert!(RenameRequest::parse(&bytes).is_err());
        }
    }

    #[test]
    fn a_control_character_in_the_name_is_refused_and_bidi_is_admitted() {
        assert!(RenameRequest::parse(&body("Kim\u{1b}]52;c;aGk=\u{7}")).is_err());
        assert!(RenameRequest::parse(&body("Kim\nLeo")).is_err());
        assert_eq!(
            RenameRequest::parse(&body("Kim\u{202e}")).unwrap().name,
            "Kim\u{202e}"
        );
    }
}
