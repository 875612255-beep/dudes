//! What the daemon knows about a recorded child right now.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The host's answer to a probe of a recorded child's *adoption key*.
///
/// `Gone` requires positive evidence; anything that is not `Alive` and is not positive evidence of
/// absence is `Unknown`, never `Gone`. **How a key is compared against a probe is one table,
/// the liveness table of the supervisor**, cited here and deliberately not restated: it is applied in exactly one function,
/// `dudes_supervisor::host::liveness`, and only the supervisor decides what an answer means.
///
/// It is declared in this crate rather than in the supervisor because it is on the wire —
/// `system.doctor` answers with it per open turn — and a contract type may not
/// reach into the supervisor. `dudes_supervisor::host` re-exports it, so the supervisor's own name
/// for it is unchanged. What the platform layer *read* is a different thing and does not move: a
/// `ProbeAnswer` is what a host saw, this is what the rule decided, and only the decision is on the
/// wire.
///
/// `Unknown` is a property of a recorded child process and never of a caller. The role a socket
/// peer resolves to is named `Unproven` where this one would have been ambiguous — a different
/// word on a different type, so one word never carries two meanings. The classification table in
/// `dudes_rpc::auth` decides what resolves to that role; it is not restated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    /// A process with that pid exists and its start value equals the recorded one.
    Alive,
    /// Positive evidence of absence: no such pid, a different start value (the pid was reused), or
    /// two *present* boot identities that differ (the machine rebooted).
    Gone,
    /// The host could not answer, or the key and this host disagree about whether a boot identity
    /// exists at all. Nothing is written; the turn stays open and is re-asked.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixed vocabulary on the wire: `alive` / `gone` / `unknown`, nothing else. Pinned because an
    /// operator surface renders these words and a rename would be a silent contract break.
    #[test]
    fn serialises_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Liveness::Alive).unwrap(),
            "\"alive\""
        );
        assert_eq!(serde_json::to_string(&Liveness::Gone).unwrap(), "\"gone\"");
        assert_eq!(
            serde_json::to_string(&Liveness::Unknown).unwrap(),
            "\"unknown\""
        );
        assert_eq!(
            serde_json::from_str::<Liveness>("\"unknown\"").unwrap(),
            Liveness::Unknown
        );
    }
}
