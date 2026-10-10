//! A bounded browser appearance edit and its journal-derived retry truth.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context as _, Result, anyhow, bail};
use dudes_journal::Reader;
use dudes_proto::Record;
use dudes_proto::record::Hex32;
use dudes_types::{AccentColour, AgentId, CastCharacter, Lsn};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

/// Maximum encoded body, including JSON escaping of a 512-byte role.
pub const MAX_LOOK_BODY_BYTES: usize = 4_096;

/// Canonical fields admitted before the writer receives them.
#[derive(Clone)]
pub struct LookRequest {
    pub(crate) request_id: Hex32,
    pub(crate) request_wire: String,
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) expected_lsn: Lsn,
    pub(crate) role: String,
    pub(crate) character: Option<CastCharacter>,
    pub(crate) accent: Option<AccentColour>,
}

impl fmt::Debug for LookRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LookRequest(<redacted>)")
    }
}

/// The durable edit path's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookOutcome {
    /// This request's original edit is durable and indexed.
    Changed {
        /// Browser request identity.
        request_id: String,
        /// Structural agent identity.
        agent: AgentId,
        /// Recorded display role.
        role: String,
        /// Recorded authored character, or automatic selection.
        character: Option<CastCharacter>,
        /// Recorded accent, or automatic selection.
        accent: Option<AccentColour>,
    },
    /// No record was appended.
    Refused {
        /// Browser request identity.
        request_id: String,
        /// `agent_absent` or `stale_look`.
        reason: &'static str,
    },
}

/// An empty HTTP refusal with no invented outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookRefusal {
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
struct LookJson {
    request_id: String,
    agent: String,
    expected_lsn: String,
    role: String,
    character: Option<CastCharacter>,
    accent: Option<AccentColour>,
}

impl LookRequest {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_LOOK_BODY_BYTES {
            bail!("Floor look request is too large");
        }
        if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            bail!("Floor look request is not one JSON object");
        }
        let object: serde_json::Value =
            serde_json::from_slice(bytes).context("invalid Floor look JSON")?;
        let Some(fields) = object.as_object() else {
            bail!("Floor look request is not one JSON object");
        };
        if !fields.contains_key("character") || !fields.contains_key("accent") {
            bail!("Floor look request omits appearance fields");
        }
        let raw: LookJson = serde_json::from_slice(bytes).context("invalid Floor look fields")?;
        let request_id = crate::floor_actions::decode_attempt(&raw.request_id)
            .context("invalid Floor look request id")?;
        let agent = AgentId::parse(&raw.agent).map_err(|_| anyhow!("invalid Floor look agent"))?;
        let expected_lsn = if raw.expected_lsn.is_empty()
            || (raw.expected_lsn.len() > 1 && raw.expected_lsn.starts_with('0'))
            || !raw.expected_lsn.bytes().all(|byte| byte.is_ascii_digit())
        {
            bail!("invalid Floor look revision");
        } else {
            Lsn(raw
                .expected_lsn
                .parse()
                .context("invalid Floor look revision")?)
        };
        let role = dudes_types::DisplayRole::parse(&raw.role)
            .context("invalid Floor look role")?
            .into_string();
        let mut hasher = Sha256::new();
        hasher.update(b"dudes.look.v1\0");
        for (field, value) in [
            ("agent", agent.to_string()),
            ("expected_lsn", expected_lsn.0.to_string()),
            ("role", role.clone()),
            (
                "character",
                raw.character.map_or("", CastCharacter::as_str).to_owned(),
            ),
            (
                "accent",
                raw.accent.map_or("", AccentColour::as_str).to_owned(),
            ),
        ] {
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
            expected_lsn,
            role,
            character: raw.character,
            accent: raw.accent,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IndexedLook {
    pub(crate) request_digest: Hex32,
    pub(crate) agent: AgentId,
    pub(crate) role: String,
    pub(crate) character: Option<CastCharacter>,
    pub(crate) accent: Option<AccentColour>,
}

#[derive(Debug, Default)]
pub(crate) struct DurableLookIndex {
    changes: BTreeMap<Hex32, IndexedLook>,
}

impl DurableLookIndex {
    pub(crate) fn replay(reader: &Reader) -> Result<Self> {
        let mut index = Self::default();
        for item in reader
            .iter_from(Lsn::ZERO)
            .context("reading Floor look index")?
        {
            let (_, entry) = item.context("reading Floor look journal entry")?;
            index.apply(&entry.record)?;
        }
        Ok(index)
    }

    pub(crate) fn apply(&mut self, record: &Record) -> Result<()> {
        let Record::AgentLookChanged {
            id,
            role,
            character,
            accent,
            browser_look: Some(meta),
        } = record
        else {
            return Ok(());
        };
        if self.changes.contains_key(&meta.request_id) {
            bail!("duplicate Floor look request in journal");
        }
        self.changes.insert(
            meta.request_id,
            IndexedLook {
                request_digest: meta.request_digest,
                agent: *id,
                role: role.clone(),
                character: *character,
                accent: *accent,
            },
        );
        Ok(())
    }

    pub(crate) fn get(&self, id: Hex32) -> Option<&IndexedLook> {
        self.changes.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(role: &str) -> Result<LookRequest> {
        LookRequest::parse(
            &serde_json::to_vec(&serde_json::json!({
                "request_id": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                "agent": "01ARZ3NDEKTSV4RRFFQ69G5FAV", "expected_lsn": "0",
                "role": role, "character": null, "accent": null,
            }))
            .unwrap(),
        )
    }

    #[test]
    fn a_control_character_in_the_role_is_refused_and_bidi_is_admitted() {
        for role in [
            "engi\u{1b}]52;c;aGk=\u{7}neer",
            "engi\nneer",
            "engi\u{9b}1mneer",
        ] {
            assert!(parse(role).is_err(), "{role:?}");
        }
        assert_eq!(
            parse(" engi\u{202e}neer\n").unwrap().role,
            "engi\u{202e}neer"
        );
    }
}
