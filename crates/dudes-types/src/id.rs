//! Identities: ULID newtypes for the five long-lived things, plus `Seq`, `Lsn` and `SessionId`.
//!
//! The newtypes share one representation (a ULID: 48-bit millisecond timestamp, 80 random bits;
//! 26 characters of Crockford base32 on the wire) and deliberately do not convert into each other:
//! an `AgentId` is never a `TurnId`. Nothing here mints an id — minting needs a clock and a random
//! source, which belong to the daemon's `Clock` and `Rng` traits. This crate only carries ids.

use core::fmt;
use core::str::FromStr;

use alloc::borrow::Cow;
use alloc::string::String;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The ULID text form: 26 characters of Crockford base32, first character `0`–`7`.
///
/// This is the only text the parser accepts. Crockford base32 has aliases (lowercase, `I`/`L`
/// for `1`, `O` for `0`) and a 130-bit text space for a 128-bit value (`ZZZZ…` decodes to the
/// same value as `7ZZZ…`); accepting any of them would give one id two names, so the parser
/// matches this pattern exactly and the schema says the same thing.
pub const ULID_PATTERN: &str = "^[0-7][0-9A-HJKMNP-TV-Z]{25}$";

/// A ULID string could not be parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum ParseIdError {
    /// The text is not 26 characters long.
    #[error("invalid id: expected 26 base32 characters")]
    InvalidLength,
    /// The text contains a character outside Crockford base32.
    #[error("invalid id: not a Crockford base32 character")]
    InvalidChar,
}

impl From<ulid::DecodeError> for ParseIdError {
    fn from(err: ulid::DecodeError) -> Self {
        match err {
            ulid::DecodeError::InvalidLength => Self::InvalidLength,
            ulid::DecodeError::InvalidChar => Self::InvalidChar,
        }
    }
}

/// True for the 32 characters of [`ULID_PATTERN`]'s class: digits and uppercase letters without
/// `I`, `L`, `O`, `U`.
const fn is_canonical_base32(byte: u8) -> bool {
    matches!(
        byte,
        b'0'..=b'9' | b'A'..=b'H' | b'J' | b'K' | b'M' | b'N' | b'P'..=b'T' | b'V'..=b'Z'
    )
}

/// Parse exactly the text [`ULID_PATTERN`] matches. The `ulid` crate's own decoder is lenient
/// (lowercase, alias letters, and a first character above `7` that silently overflows); this
/// checks the canonical form first so that the parser and the schema agree.
const fn parse_canonical(text: &str) -> Result<Ulid, ParseIdError> {
    let bytes = text.as_bytes();
    if bytes.len() != ulid::ULID_LEN {
        return Err(ParseIdError::InvalidLength);
    }
    if bytes[0] > b'7' {
        return Err(ParseIdError::InvalidChar);
    }
    let mut i = 0;
    while i < bytes.len() {
        if !is_canonical_base32(bytes[i]) {
            return Err(ParseIdError::InvalidChar);
        }
        i += 1;
    }
    match Ulid::from_string(text) {
        Ok(ulid) => Ok(ulid),
        Err(ulid::DecodeError::InvalidLength) => Err(ParseIdError::InvalidLength),
        Err(ulid::DecodeError::InvalidChar) => Err(ParseIdError::InvalidChar),
    }
}

macro_rules! ulid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Ulid);

        impl $name {
            /// The all-zero id. Never minted; useful as a sentinel in tests.
            pub const NIL: Self = Self(Ulid::nil());

            /// Wrap an existing ULID.
            #[must_use]
            pub const fn from_ulid(ulid: Ulid) -> Self {
                Self(ulid)
            }

            /// Build from the two ULID parts: a millisecond Unix timestamp and 80 random bits
            /// (the top 48 bits of `random` are discarded).
            #[must_use]
            pub const fn from_parts(timestamp_ms: u64, random: u128) -> Self {
                Self(Ulid::from_parts(timestamp_ms, random))
            }

            /// Parse the 26-character text form. Accepts exactly what [`ULID_PATTERN`] matches:
            /// lowercase, alias letters and a first character above `7` are `InvalidChar`, any
            /// other length is `InvalidLength`.
            pub const fn parse(text: &str) -> Result<Self, ParseIdError> {
                match parse_canonical(text) {
                    Ok(ulid) => Ok(Self(ulid)),
                    Err(err) => Err(err),
                }
            }

            /// The underlying ULID.
            #[must_use]
            pub const fn as_ulid(self) -> Ulid {
                self.0
            }

            /// The millisecond Unix timestamp the id was minted at.
            #[must_use]
            pub const fn timestamp_ms(self) -> u64 {
                self.0.timestamp_ms()
            }

            /// True for [`Self::NIL`].
            #[must_use]
            pub const fn is_nil(self) -> bool {
                self.0.is_nil()
            }

            /// The 16 big-endian bytes (the redb key form).
            #[must_use]
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0.to_bytes()
            }

            /// Rebuild from [`Self::to_bytes`].
            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Ulid::from_bytes(bytes))
            }
        }

        impl From<Ulid> for $name {
            fn from(ulid: Ulid) -> Self {
                Self(ulid)
            }
        }

        impl From<$name> for Ulid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }

        impl FromStr for $name {
            type Err = ParseIdError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                Self::parse(text)
            }
        }

        // Hand-written rather than `ulid/serde`: that feature does not build without `std`
        // (ulid 3.0.0 `serde.rs` names `String` unimported), and the text form needs no
        // allocation anyway.
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut buf = [0u8; ulid::ULID_LEN];
                serializer.serialize_str(self.0.array_to_str(&mut buf))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Visitor;

                impl serde::de::Visitor<'_> for Visitor {
                    type Value = $name;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("a 26-character ULID string")
                    }

                    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<$name, E> {
                        $name::parse(text).map_err(E::custom)
                    }
                }

                deserializer.deserialize_str(Visitor)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                Cow::Borrowed(stringify!($name))
            }

            fn schema_id() -> Cow<'static, str> {
                Cow::Borrowed(concat!("dudes_types::", stringify!($name)))
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "type": "string",
                    "description": concat!(
                        stringify!($name),
                        ": a ULID (26 characters of Crockford base32)."
                    ),
                    "pattern": ULID_PATTERN,
                    "minLength": 26,
                    "maxLength": 26,
                })
            }
        }
    };
}

ulid_id! {
    /// A hired agent. Minted by `agent.hire`, retired by `agent.retire`; lives for days to
    /// months and spans every turn, every attach and every goal it is given.
    AgentId
}

ulid_id! {
    /// One headless invocation of a provider CLI. Minted when `decide()` returns `Continue`,
    /// persisted as `TurnIssued` before the process is spawned, closed by `TurnEnded`.
    TurnId
}

ulid_id! {
    /// A goal: text plus a predicate the daemon can run. Spans many turns and, across hand-offs,
    /// may be pursued by more than one agent in sequence. The goal is the unit of *done*.
    GoalId
}

ulid_id! {
    /// A board card.
    CardId
}

ulid_id! {
    /// A mail item in an agent's outbox or inbox.
    MailId
}

ulid_id! {
    /// A conversation whose membership is fixed by its first mail item.
    ThreadId
}

/// A per-card sequence number. Every board transition carries the `expected_seq` it was decided
/// against; the journal rejects a transition whose `expected_seq` is stale before appending it.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Seq(pub u64);

impl Seq {
    /// The sequence number of a card that has never moved.
    pub const ZERO: Self = Self(0);

    /// The sequence number after one more transition.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A log sequence number: the position of a record in the journal,
/// `(segment_no << 40) | byte_offset_of_the_frame`. Monotone within a journal; assigned by the
/// writer, never by the caller.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct Lsn(pub u64);

impl Lsn {
    /// Width of the byte-offset field in bits.
    pub const OFFSET_BITS: u32 = 40;
    const OFFSET_MASK: u64 = (1 << Self::OFFSET_BITS) - 1;

    /// The position before any record.
    pub const ZERO: Self = Self(0);

    /// Compose an lsn from a segment number and a byte offset inside that segment.
    ///
    /// Returns `None` when the offset does not fit in 40 bits or the segment in 24.
    #[must_use]
    pub const fn from_parts(segment: u32, offset: u64) -> Option<Self> {
        if offset > Self::OFFSET_MASK || segment >= (1 << (64 - Self::OFFSET_BITS)) {
            return None;
        }
        Some(Self(((segment as u64) << Self::OFFSET_BITS) | offset))
    }

    /// The segment number.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)] // 24 bits by construction
    pub const fn segment(self) -> u32 {
        (self.0 >> Self::OFFSET_BITS) as u32
    }

    /// The byte offset of the frame inside its segment.
    #[must_use]
    pub const fn offset(self) -> u64 {
        self.0 & Self::OFFSET_MASK
    }
}

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.segment(), self.offset())
    }
}

/// A vendor session handle (`claude --resume <id>`, `codex exec resume <id>`, …).
///
/// Not a Dudes identity: it is an attribute of an agent, recorded on `TurnIssued` /
/// `SessionLearned` / `TurnEnded`, may be rotated over the agent's life, and never outlives or
/// identifies the agent. Opaque text; the vendor decides its shape.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Wrap vendor text.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The text as the vendor gave it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SessionId {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl From<&str> for SessionId {
    fn from(text: &str) -> Self {
        Self(String::from(text))
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    const TEXT: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

    /// Identity is structural: an id round-trips through its text form unchanged, so the journal,
    /// the RPC surface and the floor all name the same agent by the same 26 characters.
    #[test]
    fn ulid_newtype_round_trips_through_text_and_json() {
        let id = AgentId::parse(TEXT).unwrap();
        assert_eq!(id.to_string(), TEXT);
        assert_eq!(TEXT.parse::<AgentId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, alloc::format!("\"{TEXT}\""));
        assert_eq!(serde_json::from_str::<AgentId>(&json).unwrap(), id);
        assert_eq!(alloc::format!("{id:?}"), alloc::format!("AgentId({TEXT})"));
        assert_eq!(AgentId::from_bytes(id.to_bytes()), id);
    }

    /// Vocabulary: an `AgentId` is never a `TurnId`. The types do not convert; the test pins that
    /// the text form alone is not enough to confuse them at the type level.
    #[test]
    fn ids_are_distinct_types_with_the_same_wire_form() {
        let agent = AgentId::parse(TEXT).unwrap();
        let turn = TurnId::parse(TEXT).unwrap();
        assert_eq!(agent.as_ulid(), turn.as_ulid());
        assert_eq!(
            serde_json::to_string(&agent).unwrap(),
            serde_json::to_string(&turn).unwrap()
        );
    }

    #[test]
    fn thread_id_round_trips_as_a_canonical_ulid() {
        let id = ThreadId::parse(TEXT).unwrap();
        assert_eq!(id.to_string(), TEXT);
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            alloc::format!("\"{TEXT}\"")
        );
        assert_eq!(
            serde_json::from_str::<ThreadId>(&alloc::format!("\"{TEXT}\"")).unwrap(),
            id
        );
    }

    /// Parse errors are named variants, never a panic.
    #[test]
    fn parse_errors_are_named() {
        assert_eq!(AgentId::parse("short"), Err(ParseIdError::InvalidLength));
        assert_eq!(
            AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FAU"),
            Err(ParseIdError::InvalidChar)
        );
        assert!(serde_json::from_str::<AgentId>("\"nope\"").is_err());
    }

    /// The parser and the schema accept the same strings, and nothing else: for every edge
    /// case, `parse` succeeds exactly when the text matches [`ULID_PATTERN`]. Aliases the
    /// lenient base32 decoder would accept (lowercase, overflow, `I`/`L`/`O`/`U`) are refused
    /// so that one id has one name.
    #[test]
    fn parser_accepts_exactly_the_schema_pattern() {
        let pattern = regex::Regex::new(ULID_PATTERN).unwrap();
        let cases = [
            TEXT,
            "00000000000000000000000000",  // nil
            "7ZZZZZZZZZZZZZZZZZZZZZZZZZ",  // the largest value
            "8ZZZZZZZZZZZZZZZZZZZZZZZZZ",  // first character above 7: overflow
            "ZZZZZZZZZZZZZZZZZZZZZZZZZZ",  // decodes to the same bits as 7ZZZ… in lenient base32
            "01arz3ndektsv4rrffq69g5fav",  // lowercase
            "01ARZ3NDEKTSV4RRFFQ69G5FAv",  // one lowercase character
            "01ARZ3NDEKTSV4RRFFQ69G5FAI",  // I, an alias of 1
            "01ARZ3NDEKTSV4RRFFQ69G5FAL",  // L, an alias of 1
            "01ARZ3NDEKTSV4RRFFQ69G5FAO",  // O, an alias of 0
            "01ARZ3NDEKTSV4RRFFQ69G5FAU",  // U, not in the alphabet
            "01ARZ3NDEKTSV4RRFFQ69G5FA",   // 25 characters
            "01ARZ3NDEKTSV4RRFFQ69G5FAVV", // 27 characters
            "",
            "01ARZ3NDEKTSV4RRFFQ69G5FA-",
            " 01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "01ARZ3NDEKTSV4RRFFQ69G5FAV\n",
            "01ARZ3NDEKTSV4RRFFQ69G5FA\u{e9}", // 26 characters, 27 bytes
        ];
        for case in cases {
            assert_eq!(
                AgentId::parse(case).is_ok(),
                pattern.is_match(case),
                "parser and pattern disagree on {case:?}"
            );
            if let Ok(id) = AgentId::parse(case) {
                assert_eq!(id.to_string(), case, "an accepted id renders as itself");
            }
        }
        assert_eq!(
            AgentId::parse("ZZZZZZZZZZZZZZZZZZZZZZZZZZ"),
            Err(ParseIdError::InvalidChar)
        );
        assert_eq!(
            AgentId::parse("01arz3ndektsv4rrffq69g5fav"),
            Err(ParseIdError::InvalidChar)
        );
        assert_eq!(
            AgentId::parse("01ARZ3NDEKTSV4RRFFQ69G5FA"),
            Err(ParseIdError::InvalidLength)
        );
        assert!(serde_json::from_str::<AgentId>("\"01arz3ndektsv4rrffq69g5fav\"").is_err());
    }

    /// Ids are time-ordered: one minted later sorts after one minted earlier, as values and as
    /// text, so a journal or a directory listing sorted by id is sorted by time.
    #[test]
    fn later_minted_sorts_after_earlier() {
        let earlier = AgentId::from_parts(1_787_394_600_000, u128::MAX);
        let later = AgentId::from_parts(1_787_394_600_001, 0);
        assert!(earlier < later);
        assert!(earlier.to_string() < later.to_string());
        assert!(earlier.to_bytes() < later.to_bytes());
        assert_eq!(later.timestamp_ms(), 1_787_394_600_001);
    }

    /// Journal format: `lsn = (segment_no << 40) | byte_offset`.
    #[test]
    fn lsn_packs_segment_and_offset() {
        let lsn = Lsn::from_parts(3, 0x1234_5678).unwrap();
        assert_eq!(lsn.segment(), 3);
        assert_eq!(lsn.offset(), 0x1234_5678);
        assert_eq!(lsn.0, (3 << 40) | 0x1234_5678);
        assert_eq!(Lsn::from_parts(0, 1 << 40), None);
        assert_eq!(Lsn::from_parts(1 << 24, 0), None);
        assert!(Lsn::from_parts(1, 0).unwrap() > Lsn::from_parts(0, u64::MAX >> 24).unwrap());
        assert_eq!(lsn.to_string(), "3:305419896");
    }

    /// The schema for an id is a constrained string, so generated clients type ids as strings
    /// with the ULID pattern rather than as opaque objects.
    #[test]
    fn id_schema_is_a_patterned_string() {
        let schema = schemars::schema_for!(GoalId);
        let value = schema.as_value();
        assert_eq!(value["type"], "string");
        assert_eq!(value["pattern"], ULID_PATTERN);
        assert_eq!(value["title"], "GoalId");
    }
}
