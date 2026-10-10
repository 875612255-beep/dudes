//! The `[providers.codex]` table.

use serde::Deserialize;

/// The `[providers.codex]` table. An empty table is every default.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CodexConfig {
    /// The program, resolved by PATH lookup in the daemon, never by a shell. A `String` rather than
    /// an `OsString`, because serde reads an `OsString` only from its platform-tagged byte form.
    pub command: String,
    /// Use the app-server transport with a separate managed provider home.
    pub streaming: bool,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            command: "codex".to_owned(),
            streaming: false,
        }
    }
}
