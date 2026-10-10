//! Where a fact came from: something the daemon saw, or something it guessed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provenance of an event or a predicate result.
///
/// Ordered: `Observed > Inferred`. `Done` requires an `Observed` predicate result; an `Inferred`
/// event may raise a breaker rung but may never be the sole cause of `Stop` or `Done`
/// (invariant 3).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// The daemon did not see it: attached-mode PTY silence, a child that vanished between
    /// daemon restarts, an agent's self-report. Evidence, never a verdict.
    Inferred,
    /// The daemon saw it: a process exit code, a command it ran, a hash it computed, a board
    /// state it holds, a verifier verdict it received.
    Observed,
}

impl Provenance {
    /// True for [`Provenance::Observed`].
    #[must_use]
    pub const fn is_observed(self) -> bool {
        matches!(self, Self::Observed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invariant 3 (Done is Observed) rests on the order `Observed > Inferred`; the gate code
    /// compares, so the order is pinned here.
    #[test]
    fn observed_outranks_inferred() {
        assert!(Provenance::Observed > Provenance::Inferred);
        assert!(Provenance::Observed.is_observed());
        assert!(!Provenance::Inferred.is_observed());
    }

    /// Fixed vocabulary on the wire: `observed` / `inferred`, nothing else.
    #[test]
    fn serialises_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Provenance::Observed).unwrap(),
            "\"observed\""
        );
        assert_eq!(
            serde_json::from_str::<Provenance>("\"inferred\"").unwrap(),
            Provenance::Inferred
        );
    }
}
