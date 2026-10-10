//! The coding-agent CLIs the daemon can drive.

use core::fmt;
use core::str::FromStr;

use alloc::string::String;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A supported provider. The adapter for each lives in `dudes-providers`; this enum is only the
/// name that travels in `AgentHired`, `agent.hire` and the floor.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    /// Claude Code.
    Claude,
    /// Codex CLI.
    Codex,
    /// GitHub Copilot CLI.
    Copilot,
    /// Antigravity.
    Antigravity,
    /// Grok CLI.
    Grok,
    /// Kimi CLI.
    Kimi,
    /// Qwen Code.
    Qwen,
    /// The `opencode` CLI.
    #[serde(rename = "opencode")]
    OpenCode,
    /// Pi.
    Pi,
    /// Crush.
    Crush,
}

/// Whether a managed turn carries its provider's isolation set.
///
/// The selection crosses operator configuration, `agent.hire` and `AgentHired`, so it is shared
/// vocabulary rather than an adapter-owned type. What each mode means for a particular CLI
/// remains the adapter's contract.
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
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum IsolationMode {
    /// Carry the adapter's default isolation set.
    #[default]
    Default,
    /// Inherit the operator's CLI configuration for this agent.
    Inherit,
    /// Use the provider's bare mode, where supported and admitted by the adapter.
    Bare,
}

/// The model a managed turn was launched under, as the adapter chose it **before** the spawn.
///
/// The selector is the provider's own spelling, kept byte-for-byte: Dudes resolves no alias, keeps
/// no model catalogue, and never claims this is the model that ran. What actually consumed tokens
/// is observed at the result boundary and is a different fact with a different cardinality —
/// `TurnEnded.model_usage`, which may name several models where this names at most one.
///
/// **The type exists so that the absent and the empty cases stay distinguishable.** Wrapped in an
/// `Option` on the record, three states survive replay where a bare `Option<String>` would collapse
/// two of them: the field absent is a journal written before model selection was recorded and recording no choice at
/// all; `ModelSelection { selector: None }` is a turn that deliberately took the provider's own
/// default; `ModelSelection { selector: Some(..) }` is a turn that named one. A re-issue replays
/// the second and third exactly, and only the first may fall back to current configuration —
/// which is why flattening the two `Option`s is the mutation the record tests refuse.
///
/// It crosses operator configuration, the adapter seam and the journal, so it is shared vocabulary
/// rather than an adapter-owned type. It carries a `String`, so `Clone + Eq + Hash` rather than
/// `Copy`, per this crate's own rule.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub struct ModelSelection {
    /// The provider's raw selector, or `None` for the provider's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

impl ModelSelection {
    /// The provider's own default: a choice, deliberately made, that names no model.
    #[must_use]
    pub const fn provider_default() -> Self {
        Self { selector: None }
    }

    /// This provider selector, exactly as configured.
    #[must_use]
    pub fn named(selector: impl Into<String>) -> Self {
        Self {
            selector: Some(selector.into()),
        }
    }

    /// The selector, when one was named.
    #[must_use]
    pub fn selector(&self) -> Option<&str> {
        self.selector.as_deref()
    }
}

impl Provider {
    /// Every provider, in declaration order.
    pub const ALL: [Self; 10] = [
        Self::Claude,
        Self::Codex,
        Self::Copilot,
        Self::Antigravity,
        Self::Grok,
        Self::Kimi,
        Self::Qwen,
        Self::OpenCode,
        Self::Pi,
        Self::Crush,
    ];

    /// The wire name, which is also the config key (`[providers.<name>]`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::Antigravity => "antigravity",
            Self::Grok => "grok",
            Self::Kimi => "kimi",
            Self::Qwen => "qwen",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
            Self::Crush => "crush",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Text that names no provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("unknown provider")]
pub struct ParseProviderError;

impl FromStr for Provider {
    type Err = ParseProviderError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|p| p.as_str() == text)
            .ok_or(ParseProviderError)
    }
}

#[cfg(test)]
mod tests {
    use alloc::format;

    use super::*;

    /// One name per provider, identical on the wire (serde), in config (`as_str`) and on the
    /// command line (`FromStr`); `opencode` is one word.
    #[test]
    fn names_agree_everywhere() {
        for provider in Provider::ALL {
            let json = serde_json::to_string(&provider).unwrap();
            assert_eq!(json, format!("\"{}\"", provider.as_str()));
            assert_eq!(serde_json::from_str::<Provider>(&json).unwrap(), provider);
            assert_eq!(provider.as_str().parse::<Provider>().unwrap(), provider);
        }
        assert_eq!(Provider::OpenCode.as_str(), "opencode");
        assert_eq!("gpt".parse::<Provider>(), Err(ParseProviderError));
    }

    /// the isolation selection crosses configuration, RPC and journal
    /// boundaries, so its three names belong to the shared contract rather than an adapter.
    #[test]
    fn isolation_modes_are_shared_snake_case_vocabulary() {
        assert_eq!(IsolationMode::default(), IsolationMode::Default);
        assert_eq!(
            serde_json::to_string(&IsolationMode::Default).unwrap(),
            "\"default\""
        );
        assert_eq!(
            serde_json::from_str::<IsolationMode>("\"inherit\"").unwrap(),
            IsolationMode::Inherit
        );
        assert_eq!(
            serde_json::from_str::<IsolationMode>("\"bare\"").unwrap(),
            IsolationMode::Bare
        );
    }
}
