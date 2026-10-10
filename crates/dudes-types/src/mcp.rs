//! The name of an MCP server the operator declares and opts an agent in to.

use alloc::borrow::Cow;
use alloc::string::String;
use core::fmt;
use core::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

/// The text form: runs of lowercase letters and digits joined by single underscores, starting
/// with a letter.
pub const MCP_SERVER_NAME_PATTERN: &str = "^[a-z][a-z0-9]*(_[a-z0-9]+)*$";

/// Most characters in a server name.
pub const MAX_MCP_SERVER_NAME_CHARS: usize = 32;

/// A name outside [`MCP_SERVER_NAME_PATTERN`] or longer than [`MAX_MCP_SERVER_NAME_CHARS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error(
    "invalid MCP server name: expected 1 to 32 lowercase letters and digits in runs joined by \
     single underscores, starting with a letter"
)]
pub struct ParseMcpServerNameError;

/// One server name. It is a segment of a Codex dotted configuration path and part of every
/// engine's tool names, so it carries no `-` (Pi folds `-` into `_`) and no `__` (Claude's tool
/// names join server and tool with `__`).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct McpServerName(String);

impl McpServerName {
    /// Parse exactly the text [`MCP_SERVER_NAME_PATTERN`] matches, at most
    /// [`MAX_MCP_SERVER_NAME_CHARS`] long.
    ///
    /// # Errors
    ///
    /// Returns [`ParseMcpServerNameError`] for any other text.
    pub fn parse(text: &str) -> Result<Self, ParseMcpServerNameError> {
        let bytes = text.as_bytes();
        let starts_with_letter = bytes.first().is_some_and(u8::is_ascii_lowercase);
        let runs_are_whole = text.split('_').all(|run| {
            !run.is_empty()
                && run
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
        if starts_with_letter && runs_are_whole && bytes.len() <= MAX_MCP_SERVER_NAME_CHARS {
            Ok(Self(String::from(text)))
        } else {
            Err(ParseMcpServerNameError)
        }
    }

    /// The name as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for McpServerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for McpServerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "McpServerName({})", self.0)
    }
}

impl FromStr for McpServerName {
    type Err = ParseMcpServerNameError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for McpServerName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for McpServerName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = McpServerName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an MCP server name")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<McpServerName, E> {
                McpServerName::parse(text).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl JsonSchema for McpServerName {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("McpServerName")
    }

    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed("dudes_types::McpServerName")
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "McpServerName: an operator-declared MCP server.",
            "pattern": MCP_SERVER_NAME_PATTERN,
            "minLength": 1,
            "maxLength": MAX_MCP_SERVER_NAME_CHARS,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_lowercase_runs_joined_by_single_underscores() {
        for good in [
            "memory",
            "a",
            "notes_2",
            "notes0",
            "a1_b2_c3",
            "abcdefghijklmnopqrstuvwxyz012345",
        ] {
            assert_eq!(McpServerName::parse(good).map(|n| n.0), Ok(good.into()));
        }
        for bad in [
            "",
            "Memory",
            "1memory",
            "_memory",
            "memory_",
            "mem__ory",
            "mem-ory",
            "mem.ory",
            "mem ory",
            "abcdefghijklmnopqrstuvwxyz0123456",
        ] {
            assert_eq!(
                McpServerName::parse(bad),
                Err(ParseMcpServerNameError),
                "{bad}"
            );
        }
    }

    #[test]
    fn the_wire_form_is_the_bare_string_and_a_bad_one_does_not_decode() {
        let name = McpServerName::parse("memory").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), r#""memory""#);
        assert_eq!(
            serde_json::from_str::<McpServerName>(r#""memory""#).unwrap(),
            name
        );
        assert!(serde_json::from_str::<McpServerName>(r#""mem__ory""#).is_err());
    }
}
