//! The `[providers.pi]` table.

use serde::Deserialize;

/// The `[providers.pi]` table. An empty table is every default.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PiConfig {
    /// The program, resolved by PATH lookup in the daemon, never by a shell. A `String` for the
    /// reason [`CodexConfig::command`](crate::codex::config::CodexConfig::command) is one.
    pub command: String,
}

impl Default for PiConfig {
    fn default() -> Self {
        Self {
            command: "pi".to_owned(),
        }
    }
}
