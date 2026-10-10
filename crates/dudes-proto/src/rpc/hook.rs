//! `hook.event` params: what `dudes-hook` sends over the process-bearing local transport.

use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::secret;

/// The daemon's explicit hook-admission refusal. The shim distinguishes this from delivery and
/// authorization failures so only overloaded control hooks block the vendor step.
pub const OVERLOADED: &str = "hook event was not queued because hook capacity is full";

/// `hook.event` params.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HookEventParams {
    /// The per-agent hook token, minted before spawn and handed to the child by environment,
    /// never by argv. The daemon stores only its hash; the token is the agent's identity on
    /// this call; independently resolved peer ancestry qualifies it (invariant 8).
    pub token: HookToken,
    /// The vendor's JSON, verbatim.
    pub raw: Value,
}

/// Length of a hook token on the wire: 32 random bytes as unpadded base64url.
pub const HOOK_TOKEN_LEN: usize = 43;

/// The hook token text form: exactly [`HOOK_TOKEN_LEN`] characters of the base64url alphabet.
pub const HOOK_TOKEN_PATTERN: &str = "^[A-Za-z0-9_-]{43}$";

/// A per-agent hook token: the capability that authenticates `hook.event`.
///
/// A secret, so it behaves unlike other strings: `Debug` prints `HookToken(<redacted>)`,
/// equality is constant-time, there is no `Display` and no `Clone`, and the text is reached
/// only through [`HookToken::expose`]. The wire form is validated on the way in: anything but
/// [`HOOK_TOKEN_LEN`] base64url characters is refused before it reaches a handler.
///
/// All three of those properties come from the private `secret` module, which
/// [`OperatorCredential`](super::cred::OperatorCredential) shares: two secrets audited as one
/// shape is one audit.
#[derive(Eq)]
pub struct HookToken(String);

impl HookToken {
    /// Wrap token text, refusing anything that is not [`HOOK_TOKEN_PATTERN`].
    pub fn new(text: impl Into<String>) -> Result<Self, InvalidHookToken> {
        let text = text.into();
        if secret::well_formed(&text, HOOK_TOKEN_LEN) {
            Ok(Self(text))
        } else {
            Err(InvalidHookToken)
        }
    }

    /// The token text. Named so a call site that hands the secret on is visible in review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HookToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        secret::redacted("HookToken", f)
    }
}

impl PartialEq for HookToken {
    /// Constant-time over the bytes, so a comparison against a presented token does not leak
    /// how many leading characters matched.
    fn eq(&self, other: &Self) -> bool {
        secret::eq(&self.0, &other.0)
    }
}

impl Serialize for HookToken {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HookToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HookToken {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "HookToken".into()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        "dudes_proto::rpc::hook::HookToken".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "A per-agent hook token: 32 random bytes as unpadded base64url. A secret; never logged.",
            "pattern": HOOK_TOKEN_PATTERN,
            "minLength": HOOK_TOKEN_LEN,
            "maxLength": HOOK_TOKEN_LEN,
        })
    }
}

/// Text that is not a well-formed hook token. Carries nothing: the offending text is a
/// candidate secret and does not belong in an error message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvalidHookToken;

impl fmt::Display for InvalidHookToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid hook token: expected {HOOK_TOKEN_LEN} base64url characters"
        )
    }
}

impl std::error::Error for InvalidHookToken {}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    /// The token never appears in `Debug` output — not on its own, not through the params
    /// that carry it — so a `tracing` line or a panic message cannot leak it.
    #[test]
    fn debug_redacts_the_token() {
        let token = HookToken::new(TOKEN).unwrap();
        assert_eq!(format!("{token:?}"), "HookToken(<redacted>)");
        let params = HookEventParams {
            token,
            raw: serde_json::json!({"hook_event_name": "Stop"}),
        };
        let debug = format!("{params:?}");
        assert!(!debug.contains(TOKEN), "{debug}");
        assert!(debug.contains("HookToken(<redacted>)"), "{debug}");
        assert_eq!(params.token.expose(), TOKEN);
    }

    /// The wire form is exactly 43 base64url characters, in the parser and in the schema;
    /// padding, other alphabets and other lengths are refused before a handler sees them.
    #[test]
    fn wire_form_is_43_base64url_characters() {
        assert!(HookToken::new(TOKEN).is_ok());
        assert!(HookToken::new("abcDEF012-_abcDEF012-_abcDEF012-_abcDEF012-").is_ok());
        for bad in [
            "",
            &TOKEN[..42],
            &format!("{TOKEN}A"),
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA+/",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA ",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\u{e9}",
        ] {
            assert_eq!(HookToken::new(bad), Err(InvalidHookToken), "{bad:?}");
        }
        let json = serde_json::to_string(&HookToken::new(TOKEN).unwrap()).unwrap();
        assert_eq!(json, format!("\"{TOKEN}\""));
        assert_eq!(
            serde_json::from_str::<HookToken>(&json).unwrap(),
            HookToken::new(TOKEN).unwrap()
        );
        assert!(serde_json::from_str::<HookToken>("\"short\"").is_err());
        assert!(serde_json::from_str::<HookEventParams>(r#"{"token":"short","raw":{}}"#).is_err());
        let schema = schemars::schema_for!(HookToken);
        let value = schema.as_value();
        assert_eq!(value["type"], "string");
        assert_eq!(value["pattern"], HOOK_TOKEN_PATTERN);
        assert_eq!(value["minLength"], 43);
        assert_eq!(value["maxLength"], 43);
    }

    /// Equality compares the whole token; two tokens that differ in the last character are
    /// not equal and the error type names no text.
    #[test]
    fn equality_and_error_carry_no_secret() {
        let a = HookToken::new(TOKEN).unwrap();
        let b = HookToken::new(format!("{}B", &TOKEN[..42])).unwrap();
        assert_ne!(a, b);
        assert_eq!(a, HookToken::new(TOKEN).unwrap());
        assert_eq!(
            InvalidHookToken.to_string(),
            "invalid hook token: expected 43 base64url characters"
        );
    }
}
