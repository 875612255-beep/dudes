//! The operator credential: what a calling operator presents, on the [`Request`](super::Request)
//! envelope.
//!
//! It is the other half of invariant 8's *presented* side, beside the per-agent
//! [`HookToken`](super::hook::HookToken). The two are deliberately the same shape — same length,
//! same alphabet, same redaction, same constant-time equality, sharing one implementation of each
//! in the private `secret` module — because two secrets audited as one shape is one audit.
//!
//! **Presenting is necessary and not sufficient.** What a presented credential *resolves to* is
//! qualified by what the daemon can independently resolve about the peer, and that is one table —
//! the peer-classification table that `dudes_rpc::auth::classify` implements, deliberately not paraphrased here. Nothing in this crate
//! resolves anything.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::secret;

/// Length of an operator credential on the wire: 32 random bytes as unpadded base64url.
pub const OPERATOR_CREDENTIAL_LEN: usize = 43;

/// The operator credential text form: exactly [`OPERATOR_CREDENTIAL_LEN`] characters of the
/// base64url alphabet.
pub const OPERATOR_CREDENTIAL_PATTERN: &str = "^[A-Za-z0-9_-]{43}$";

/// The capability that identifies a calling **operator**: 32 random bytes the daemon mints once
/// per daemon run.
///
/// A secret, so it behaves unlike other strings: `Debug` prints `OperatorCredential(<redacted>)`,
/// equality is constant-time, there is no `Display` and no `Clone`, and the text is reached only
/// through [`OperatorCredential::expose`]. The wire form is validated on the way in: anything but
/// [`OPERATOR_CREDENTIAL_LEN`] base64url characters is refused before it reaches a handler.
///
/// # What it is not
///
/// It is **not a boundary**. The file it is read from is readable by the same uid, so an agent
/// child that goes looking will find it; what it removes is the *ambient* grant, so that reaching
/// the operator method set requires an act rather than an omission. And a
/// credential the daemon recognises still resolves through peer provenance before it means
/// anything, whose four columns are not all the same answer.
///
/// # Why there is no `Clone`
///
/// The same reason [`HookToken`](super::hook::HookToken) has none: a secret that can be duplicated
/// by inference is a secret whose copies nobody counted. Handing it on is
/// [`OperatorCredential::expose`], which is greppable, or building a second value from the text,
/// which is visible at the call site. [`Request`](super::Request) therefore has no `Clone` either.
#[derive(Eq)]
pub struct OperatorCredential(String);

impl OperatorCredential {
    /// Wrap credential text, refusing anything that is not [`OPERATOR_CREDENTIAL_PATTERN`].
    ///
    /// # Errors
    ///
    /// [`InvalidOperatorCredential`] when the text is not well formed.
    pub fn new(text: impl Into<String>) -> Result<Self, InvalidOperatorCredential> {
        let text = text.into();
        if secret::well_formed(&text, OPERATOR_CREDENTIAL_LEN) {
            Ok(Self(text))
        } else {
            Err(InvalidOperatorCredential)
        }
    }

    /// The credential text. Named so a call site that hands the secret on is visible in review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OperatorCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        secret::redacted("OperatorCredential", f)
    }
}

impl PartialEq for OperatorCredential {
    /// Constant-time over the bytes, so a comparison against a presented credential does not leak
    /// how many leading characters matched.
    fn eq(&self, other: &Self) -> bool {
        secret::eq(&self.0, &other.0)
    }
}

impl Serialize for OperatorCredential {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OperatorCredential {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for OperatorCredential {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OperatorCredential".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "dudes_proto::rpc::cred::OperatorCredential".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "The operator credential: 32 random bytes as unpadded base64url, minted once per daemon run. A secret; never logged.",
            "pattern": OPERATOR_CREDENTIAL_PATTERN,
            "minLength": OPERATOR_CREDENTIAL_LEN,
            "maxLength": OPERATOR_CREDENTIAL_LEN,
        })
    }
}

/// Text that is not a well-formed operator credential. Carries nothing: the offending text is a
/// candidate secret and does not belong in an error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvalidOperatorCredential;

impl fmt::Display for InvalidOperatorCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid operator credential: expected {OPERATOR_CREDENTIAL_LEN} base64url characters"
        )
    }
}

impl std::error::Error for InvalidOperatorCredential {}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> String {
        "c".repeat(OPERATOR_CREDENTIAL_LEN)
    }

    /// Well-formed text is accepted and reaches back out unchanged; the length and the alphabet are
    /// both enforced.
    #[test]
    fn only_well_formed_text_becomes_a_credential() {
        assert_eq!(
            OperatorCredential::new(text()).unwrap().expose(),
            text(),
            "positive control: a well-formed credential round-trips"
        );
        for bad in [
            "",
            "short",
            &"c".repeat(42),
            &"c".repeat(44),
            &"+".repeat(43),
        ] {
            assert_eq!(
                OperatorCredential::new(bad),
                Err(InvalidOperatorCredential),
                "`{bad}` must not become a credential"
            );
        }
    }

    /// The `Debug` prints no part of the secret. Asserted against a distinctive value so a
    /// substring of it could actually be found if the redaction were dropped.
    #[test]
    fn debug_is_redacted() {
        let distinctive = format!("{}{}", "zq7", "c".repeat(OPERATOR_CREDENTIAL_LEN - 3));
        let cred = OperatorCredential::new(distinctive.clone()).unwrap();
        let shown = format!("{cred:?}");
        assert_eq!(shown, "OperatorCredential(<redacted>)");
        assert!(
            !shown.contains("zq7"),
            "the credential text must not reach a log"
        );
        assert!(
            format!("{:?}", Some(&cred)).contains("<redacted>"),
            "and not through a wrapper either"
        );
    }

    /// Equality answers both ways round: a credential equals itself and differs from another, so
    /// the constant-time comparison has not been implemented as "always false".
    #[test]
    fn equality_answers_both_ways() {
        let a = OperatorCredential::new(text()).unwrap();
        let same = OperatorCredential::new(text()).unwrap();
        let other = OperatorCredential::new("d".repeat(OPERATOR_CREDENTIAL_LEN)).unwrap();
        assert_eq!(a, same, "positive control");
        assert_ne!(a, other);
    }

    /// The wire form is a bare string, and a malformed one is refused at deserialisation rather
    /// than reaching a handler.
    #[test]
    fn the_wire_form_is_a_validated_string() {
        let json = serde_json::to_string(&OperatorCredential::new(text()).unwrap()).unwrap();
        assert_eq!(json, format!("\"{}\"", text()));
        let back: OperatorCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(back.expose(), text());
        assert!(
            serde_json::from_str::<OperatorCredential>("\"nope\"").is_err(),
            "a malformed credential is refused on the way in"
        );
    }

    /// The error text names the requirement and repeats no candidate secret.
    #[test]
    fn the_error_carries_no_text() {
        let message = InvalidOperatorCredential.to_string();
        assert!(message.contains("43"));
        assert!(!message.contains('"'));
    }
}
