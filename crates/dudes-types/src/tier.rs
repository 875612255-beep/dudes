//! Provider tiers: what attached-mode governance a provider can offer.

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How much the daemon can govern a provider while a human is attached.
///
/// The tier changes attached-mode governance only; managed mode is the same on every tier and
/// is the only mode the loop needs.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Tier 1: hook-governed. A stop-boundary hook can force continuation and a pre-tool hook
    /// can deny; the daemon answers the vendor's hooks with deny / steer / halt.
    HookGoverned,
    /// Tier 2: program-governed. An in-process plugin or API does the same imperatively.
    ProgramGoverned,
    /// Tier 3: observed-only. Managed mode works; attached mode gets pause only.
    ObservedOnly,
}

impl Tier {
    /// The tier number as the documents write it: 1, 2 or 3.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::HookGoverned => 1,
            Self::ProgramGoverned => 2,
            Self::ObservedOnly => 3,
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tier {}", self.number())
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    /// Vocabulary: exactly three tiers, numbered 1–3 in declaration order, named on the wire as
    /// `hook_governed` / `program_governed` / `observed_only`.
    #[test]
    fn tiers_are_numbered_and_named() {
        assert_eq!(Tier::HookGoverned.number(), 1);
        assert_eq!(Tier::ProgramGoverned.number(), 2);
        assert_eq!(Tier::ObservedOnly.number(), 3);
        assert_eq!(Tier::ObservedOnly.to_string(), "Tier 3");
        assert_eq!(
            serde_json::to_string(&Tier::HookGoverned).unwrap(),
            "\"hook_governed\""
        );
        assert!(Tier::HookGoverned < Tier::ObservedOnly);
    }
}
