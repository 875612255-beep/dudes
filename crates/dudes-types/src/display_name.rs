//! Admission of new agent display names and roles. Historical journal strings remain replayable.

use alloc::string::{String, ToString};

/// Most UTF-8 bytes in an admitted display name.
pub const MAX_DISPLAY_NAME_BYTES: usize = 128;

/// Most UTF-8 bytes in an admitted display role.
pub const MAX_DISPLAY_ROLE_BYTES: usize = 512;

/// `value` without surrounding ASCII space, tab, carriage return and line feed, when that holds 1 to
/// `max` UTF-8 bytes and no control character, which a terminal printing it would obey.
fn admitted(value: &str, max: usize) -> Option<&str> {
    let text = value.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'));
    ((1..=max).contains(&text.len()) && !text.chars().any(char::is_control)).then_some(text)
}

/// A canonical display name admitted for a new hire or rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayName(String);

/// A name outside the admitted byte bounds, or one holding a control character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "display name must contain 1 to 128 UTF-8 bytes after trimming ASCII space, and no control character"
)]
pub struct InvalidDisplayName;

impl DisplayName {
    /// Trim ASCII space, tab, carriage return and line feed, and admit 1 to 128 UTF-8 bytes with no
    /// control character.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidDisplayName`] for an empty or overlong name, or one that still holds a
    /// control character after trimming.
    pub fn parse(value: &str) -> Result<Self, InvalidDisplayName> {
        admitted(value, MAX_DISPLAY_NAME_BYTES)
            .map(|name| Self(name.to_string()))
            .ok_or(InvalidDisplayName)
    }

    /// Consume the admitted name.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// A canonical role admitted for a browser hire or a look edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayRole(String);

/// A role outside the admitted byte bounds, or one holding a control character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "role must contain 1 to 512 UTF-8 bytes after trimming ASCII space, and no control character"
)]
pub struct InvalidDisplayRole;

impl DisplayRole {
    /// Trim ASCII space, tab, carriage return and line feed, and admit 1 to 512 UTF-8 bytes with no
    /// control character.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidDisplayRole`] for an empty or overlong role, or one that still holds a
    /// control character after trimming.
    pub fn parse(value: &str) -> Result<Self, InvalidDisplayRole> {
        admitted(value, MAX_DISPLAY_ROLE_BYTES)
            .map(|role| Self(role.to_string()))
            .ok_or(InvalidDisplayRole)
    }

    /// Consume the admitted role.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_trims_only_the_four_ascii_scalars_and_counts_utf8_bytes() {
        assert_eq!(
            DisplayName::parse(" \tKim\r\n").unwrap().into_string(),
            "Kim"
        );
        assert_eq!(
            DisplayName::parse("\u{a0}Kim\u{a0}").unwrap().into_string(),
            "\u{a0}Kim\u{a0}"
        );
        assert!(DisplayName::parse(" \t\r\n").is_err());
        assert!(DisplayName::parse(&"é".repeat(64)).is_ok());
        assert!(DisplayName::parse(&"é".repeat(65)).is_err());
        assert!(DisplayName::parse(&"x".repeat(129)).is_err());
    }

    #[test]
    fn admission_refuses_each_control_character_left_after_trimming() {
        for name in [
            "Kim\u{1b}]52;c;aGk=\u{7}",
            "Kim\u{1b}[2J",
            "Kim\nLeo",
            "Kim\u{7f}",
            "Kim\u{9b}1m",
            "\u{0}",
        ] {
            assert_eq!(
                DisplayName::parse(name),
                Err(InvalidDisplayName),
                "{name:?}"
            );
        }
        assert_eq!(
            DisplayName::parse("\u{202e}Kim\u{2066}")
                .unwrap()
                .into_string(),
            "\u{202e}Kim\u{2066}"
        );
    }

    #[test]
    fn role_admission_trims_counts_512_bytes_and_refuses_control_characters() {
        assert_eq!(
            DisplayRole::parse(" \tengi\u{202e}neer\r\n")
                .unwrap()
                .into_string(),
            "engi\u{202e}neer"
        );
        assert!(DisplayRole::parse(&"é".repeat(256)).is_ok());
        assert_eq!(
            DisplayRole::parse(&"é".repeat(257)),
            Err(InvalidDisplayRole)
        );
        for role in [
            " \t\r\n",
            "engi\u{1b}[2Jneer",
            "engi\nneer",
            "engi\u{9b}1mneer",
        ] {
            assert_eq!(
                DisplayRole::parse(role),
                Err(InvalidDisplayRole),
                "{role:?}"
            );
        }
    }
}
