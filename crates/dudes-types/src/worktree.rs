//! The identity of a daemon-created git worktree.

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt::{self, Write as _};
use core::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// The text form: 32 lowercase hexadecimal characters.
pub const WORKTREE_ID_PATTERN: &str = "^[0-9a-f]{32}$";

/// Exactly `2 * N` lowercase hexadecimal characters as `N` bytes.
fn parse_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let bytes = text.as_bytes();
    if bytes.len() != 2 * N {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = (digit(bytes[2 * index])? << 4) | digit(bytes[2 * index + 1])?;
    }
    Some(out)
}

/// A worktree text that is not 32 lowercase hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid worktree id: expected 32 lowercase hexadecimal characters")]
pub struct ParseWorktreeIdError;

/// A git worktree one agent owns, minted by the daemon when it leases the tree.
///
/// Sixteen bytes, written as 32 lowercase hexadecimal characters. It does not convert into the
/// other identities, and this crate mints nothing: the daemon draws the bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorktreeId([u8; 16]);

impl WorktreeId {
    /// Wrap the bytes the daemon drew.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 bytes (the redb key form).
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Parse exactly the text [`WORKTREE_ID_PATTERN`] matches.
    ///
    /// # Errors
    ///
    /// Returns [`ParseWorktreeIdError`] for any other text, uppercase hexadecimal included.
    pub fn parse(text: &str) -> Result<Self, ParseWorktreeIdError> {
        parse_hex(text).map(Self).ok_or(ParseWorktreeIdError)
    }

    /// The first 12 hexadecimal characters, which name the lease's branch.
    #[must_use]
    pub fn short(self) -> String {
        let mut text = String::with_capacity(12);
        for byte in &self.0[..6] {
            let _ = write!(text, "{byte:02x}");
        }
        text
    }
}

impl fmt::Display for WorktreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for WorktreeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WorktreeId({self})")
    }
}

impl FromStr for WorktreeId {
    type Err = ParseWorktreeIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for WorktreeId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for WorktreeId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = WorktreeId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 32-character lowercase hexadecimal worktree id")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<WorktreeId, E> {
                WorktreeId::parse(text).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl JsonSchema for WorktreeId {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("WorktreeId")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("dudes_types::WorktreeId")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "WorktreeId: a daemon-created git worktree (32 lowercase hexadecimal characters).",
            "pattern": WORKTREE_ID_PATTERN,
            "minLength": 32,
            "maxLength": 32,
        })
    }
}

/// A git commit as the lease records it: 40 lowercase hexadecimal characters.
///
/// The daemon builds one only from a `git rev-parse` answer it has parsed, so a lease never names
/// a branch or a symbolic ref that could move.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GitCommit([u8; 20]);

/// A commit text that is not 40 lowercase hexadecimal characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("invalid git commit: expected 40 lowercase hexadecimal characters")]
pub struct ParseGitCommitError;

impl GitCommit {
    /// The text form's pattern.
    pub const PATTERN: &'static str = "^[0-9a-f]{40}$";

    /// Parse exactly [`Self::PATTERN`].
    ///
    /// # Errors
    ///
    /// Returns [`ParseGitCommitError`] for any other text.
    pub fn parse(text: &str) -> Result<Self, ParseGitCommitError> {
        parse_hex(text).map(Self).ok_or(ParseGitCommitError)
    }
}

impl fmt::Display for GitCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for GitCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GitCommit({self})")
    }
}

impl FromStr for GitCommit {
    type Err = ParseGitCommitError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for GitCommit {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for GitCommit {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = GitCommit;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 40-character lowercase hexadecimal git commit")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<GitCommit, E> {
                GitCommit::parse(text).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl JsonSchema for GitCommit {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("GitCommit")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("dudes_types::GitCommit")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "GitCommit: a commit object name (40 lowercase hexadecimal characters).",
            "pattern": GitCommit::PATTERN,
            "minLength": 40,
            "maxLength": 40,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_only_lowercase_hex_parses() {
        let id = WorktreeId::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0, 0, 0, 0, 0, 0, 0, 0xff,
        ]);
        assert_eq!(alloc::format!("{id}"), "0123456789abcdef00000000000000ff");
        assert_eq!(WorktreeId::parse(&alloc::format!("{id}")), Ok(id));
        assert_eq!(id.short(), "0123456789ab");
        for bad in [
            "0123456789ABCDEF00000000000000FF",
            "0123456789abcdef00000000000000f",
            "0123456789abcdef00000000000000fff",
            "0123456789abcdef00000000000000fg",
            "",
        ] {
            assert_eq!(WorktreeId::parse(bad), Err(ParseWorktreeIdError), "{bad}");
        }
    }

    #[test]
    fn a_commit_is_forty_lowercase_hex_characters() {
        let text = "0123456789abcdef0123456789abcdef01234567";
        let commit = GitCommit::parse(text).unwrap();
        assert_eq!(alloc::format!("{commit}"), text);
        assert_eq!(serde_json::to_value(commit).unwrap(), text);
        for bad in ["", "HEAD", &text[1..], &text.to_uppercase(), "main"] {
            assert!(GitCommit::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn serde_is_the_text_form() {
        let id = WorktreeId::from_bytes([7; 16]);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"07070707070707070707070707070707\"");
        assert_eq!(serde_json::from_str::<WorktreeId>(&json).unwrap(), id);
        assert!(serde_json::from_str::<WorktreeId>("\"07\"").is_err());
    }
}
