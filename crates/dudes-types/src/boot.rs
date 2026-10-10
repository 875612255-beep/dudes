//! The boot identity a daemon run stamps spawns with.

use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// This run's boot identity, as the host that read it reports it, and the *reason*
/// when there is none.
///
/// **Three values, because two of them mean opposite things and the third is their difference.**
/// `AdoptionKey`'s `boot` is `Option<String>` and its `None` carries exactly one meaning — the
/// writer asserts its start value is wall-clock-anchored — and never "the writer is boot-relative
/// but could not read its boot identity". A host reporting itself has the same two
/// facts to keep apart, and one more reason to: [`Unreadable`](Self::Unreadable) is the state in
/// which every recorded key that carries a boot identity answers `Liveness::Unknown` against this
/// host, so every open turn of that run stops making progress. Collapsed into "this host has
/// none", that is a failure that is nearly unreachable and invisible; kept
/// apart, `system.doctor` can say it out loud before it bites.
///
/// Declared here rather than in the host or the supervisor because it is on the wire —
/// `system.doctor` answers with it once per run — and a contract type may not reach into either
/// ([`Liveness`](crate::Liveness) lives here too for the same reason).
///
/// **Opaque.** A boot identity is compared for equality and never parsed: a kernel that changes
/// the format of its value still works. The `String` in [`Unreadable`](Self::Unreadable) is the
/// opposite — a message for an operator, never compared.
///
/// **No `Default`, deliberately.** Every variant is a claim about a host, so there is no value a
/// deserialiser could supply on a producer's behalf without inventing one: a payload that names no
/// boot identity is a daemon that did not report one, which is a fact about the *reply* and is
/// carried by the absence of the field rather than by a variant of this type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostBoot {
    /// This platform's start values are captured at process creation and mean the same thing
    /// across a reboot, so the keys it writes carry no boot identity. macOS and Windows.
    NotApplicable,
    /// Read once, at daemon start, and stamped on every spawn of this run. Linux.
    Read(String),
    /// A boot-relative platform whose read failed this run, and what the platform said about it.
    ///
    /// Nothing this host spawns is recorded (the spawn is refused rather than writing a key whose
    /// `None` would lie), and every recorded key that *does* carry a boot identity answers
    /// `Liveness::Unknown` here — no evidence, rather than a wrong `Gone`.
    Unreadable(String),
}

impl HostBoot {
    /// What `dudes_supervisor::host::liveness` takes as `host_boot`: the identity if this host has
    /// one, `None` otherwise — and `None` for both of the other two variants, which is exactly why
    /// they are kept apart *here* rather than there.
    #[must_use]
    pub fn as_host_boot(&self) -> Option<&str> {
        match self {
            Self::Read(boot) => Some(boot.as_str()),
            Self::NotApplicable | Self::Unreadable(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::borrow::ToOwned;

    use super::*;

    /// Fixed vocabulary on the wire, asserted literally in both directions. Pinned because an
    /// operator surface renders these words and a rename would be a silent contract break.
    #[test]
    fn serialises_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&HostBoot::NotApplicable).unwrap(),
            "\"not_applicable\""
        );
        assert_eq!(
            serde_json::to_string(&HostBoot::Read("f0e1d2c3".to_owned())).unwrap(),
            "{\"read\":\"f0e1d2c3\"}"
        );
        assert_eq!(
            serde_json::to_string(&HostBoot::Unreadable("permission denied".to_owned())).unwrap(),
            "{\"unreadable\":\"permission denied\"}"
        );
        assert_eq!(
            serde_json::from_str::<HostBoot>("{\"unreadable\":\"permission denied\"}").unwrap(),
            HostBoot::Unreadable("permission denied".to_owned())
        );
    }

    /// The two variants that produce no identity are not equal to each other, and neither is
    /// `Unreadable` to an `Unreadable` with a different reason.
    ///
    /// This is the assertion a collapse to `Option<String>` fails: under that shape both of the
    /// first two are `None` and compare equal, which is precisely the fact the adoption key refuses to
    /// let one value carry.
    #[test]
    fn a_platform_with_no_boot_identity_is_not_a_failed_read() {
        assert_ne!(
            HostBoot::NotApplicable,
            HostBoot::Unreadable("permission denied".to_owned())
        );
        assert_ne!(
            HostBoot::Unreadable("permission denied".to_owned()),
            HostBoot::Unreadable("no such file".to_owned())
        );
        // The control: two of the same variant with the same payload do compare equal, so the
        // assertions above are about the variants and not about a type that answers `ne` to
        // everything.
        assert_eq!(HostBoot::NotApplicable, HostBoot::NotApplicable);
    }

    /// Only a read produces an identity for the comparison. The other two answer `None` — the same
    /// answer, from two states a caller of this method is deliberately not asked to tell apart.
    #[test]
    fn only_a_read_answers_the_comparison() {
        assert_eq!(
            HostBoot::Read("f0e1d2c3".to_owned()).as_host_boot(),
            Some("f0e1d2c3")
        );
        assert_eq!(HostBoot::NotApplicable.as_host_boot(), None);
        assert_eq!(
            HostBoot::Unreadable("permission denied".to_owned()).as_host_boot(),
            None
        );
    }
}
