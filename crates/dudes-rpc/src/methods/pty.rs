//! The decoders of `pty.write` and of browser terminal input: the one place base64 keystrokes
//! become bytes.
//!
//! Nothing else in this crate decodes a payload into something a host writes, and that is the
//! point of keeping it in one function. The bytes it produces are the bytes the router digests
//! into `PtyWriteRequested.payload` and the bytes the daemon hands the host — one derivation, so
//! the digest names what was written rather than a re-encoding of it.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dudes_proto::rpc::pty::PtyWriteParams;
use dudes_proto::rpc::{ErrorCode, RpcError};
use dudes_proto::{ConditionedTarget, PayloadDigest};
use serde_json::Value;

use crate::auth::PeerRole;
use crate::conditioned::DecodedEffect;
use crate::dispatch::error;

/// Decode `pty.write` before the central conditioned disposition is selected.
///
/// **The engine is `STANDARD` and its strictness is load-bearing, not a default nobody chose.**
/// `base64`'s `STANDARD` requires canonical padding and rejects non-zero trailing bits, so exactly
/// one spelling of any byte string is admitted. A permissive engine would let two different
/// `data` strings produce one `PayloadDigest`, and the journal would then carry a digest a reader
/// cannot reproduce from the frame that was sent — which is the whole reason
/// `PtyWriteParams::data` states the canonical encoding.
///
/// Neither identity nor provenance appears here. Invariant 8 requires those to come from the
/// accepted connection, and `PtyWriteParams` has no field that could carry one:
/// `deny_unknown_fields` is what turns an attempt to add one into `-32602` rather than a silently
/// dropped key.
pub(crate) fn conditioned_write(
    value: Option<&Value>,
    _role: &PeerRole,
) -> Result<DecodedEffect<PtyWriteParams>, RpcError> {
    let request: PtyWriteParams = serde_json::from_value(
        value
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new())),
    )
    .map_err(|err| error(ErrorCode::InvalidParams, format!("pty.write: {err}")))?;
    let payload = STANDARD.decode(request.data.as_bytes()).map_err(|err| {
        error(
            ErrorCode::InvalidParams,
            format!("pty.write: data is not canonical base64: {err}"),
        )
    })?;
    let target = ConditionedTarget::Pty { pty: request.pty };
    Ok(DecodedEffect::new(request, target, Some(payload)))
}

/// Decode browser terminal input with the same strict engine, and digest the decoded bytes the
/// way the router digests `pty.write`'s. `None` for any non-canonical text.
#[must_use]
pub fn decode_terminal_input(data: &str) -> Option<(Vec<u8>, PayloadDigest)> {
    let bytes = STANDARD.decode(data.as_bytes()).ok()?;
    let digest = crate::conditioned::payload_digest(&bytes);
    Some((bytes, digest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dudes_proto::rpc::pty::PtyId;
    use serde_json::json;

    fn decode(data: &str) -> Result<Vec<u8>, RpcError> {
        let params = json!({ "pty": 7, "data": data });
        let effect = conditioned_write(Some(&params), &PeerRole::Operator)?;
        Ok(effect.payload_for_test().to_vec())
    }

    /// The decoded bytes are exactly the caller's, including bytes JSON would have mangled.
    #[test]
    fn canonical_base64_decodes_to_the_exact_bytes() {
        assert_eq!(decode("aGk=").expect("hi"), b"hi");
        assert_eq!(decode("").expect("empty is a legal write"), b"");
        // A control byte, a NUL and a high byte: the three classes a keystroke stream carries and
        // a naive decoder loses.
        assert_eq!(
            decode("AwD/").expect("control, NUL and high byte"),
            vec![0x03, 0x00, 0xff]
        );
    }

    /// Every non-canonical spelling is `-32602`, so one byte string has one admitted encoding.
    ///
    /// Without this, two `data` values would share a `PayloadDigest` and the journal's digest
    /// would stop naming the frame it came from. The control below is the same decoder accepting
    /// the canonical form of the same bytes, so these are refusals of the spelling and not of the
    /// content.
    ///
    /// **`"aGl="` is the row that reaches the trailing-bits rule, and it has to be padded to reach
    /// it.** The engine checks padding first, so an *unpadded* string with non-zero trailing bits
    /// is rejected for its padding and the contract's "no non-zero trailing bits" rule is never
    /// consulted. Measured both ways against an engine relaxing only `decode_allow_trailing_bits`:
    /// without this row the whole test stayed green while `"aGk="` and `"aGl="` both decoded to
    /// `hi`; with it the mutant dies here, on the second spelling of one byte string that would
    /// otherwise share one `PayloadDigest`.
    #[test]
    fn a_non_canonical_spelling_is_refused() {
        for spelling in [
            "aGk",       // missing padding
            "aGk==",     // over-padded
            "aG k=",     // embedded space
            "aGk=\n",    // trailing newline
            "aGk=extra", // data after the pad
            "aGl",       // unpadded, and its trailing bits are non-zero
            "aGl=",      // padded, and its trailing bits are non-zero: `hi` spelled a second way
            "a-k=",      // base64url alphabet, not base64
        ] {
            let refused = decode(spelling)
                .map(|bytes| format!("{bytes:?}"))
                .expect_err("a non-canonical spelling is refused");
            assert_eq!(
                refused.code,
                ErrorCode::InvalidParams.code(),
                "{spelling:?} was not refused as invalid params"
            );
        }
        // Control: the canonical spelling of the same two bytes still decodes, so the refusals
        // above are about the spelling and not a decoder that refuses everything.
        assert_eq!(decode("aGk=").expect("the canonical spelling"), b"hi");
    }

    /// The target is derived from the typed `pty`, and no params key can carry identity.
    #[test]
    fn the_target_is_derived_and_identity_is_not_a_field() {
        let params = json!({ "pty": 41, "data": "" });
        let effect = conditioned_write(Some(&params), &PeerRole::Operator).expect("decoded");
        assert_eq!(
            effect.target_for_test(),
            &ConditionedTarget::Pty { pty: PtyId(41) }
        );

        for key in ["provenance", "actor", "by", "agent"] {
            let mut object = serde_json::Map::new();
            object.insert("pty".into(), json!(41));
            object.insert("data".into(), json!(""));
            object.insert(key.into(), json!("anything"));
            let refused = conditioned_write(Some(&Value::Object(object)), &PeerRole::Operator)
                .err()
                .unwrap_or_else(|| panic!("an unknown `{key}` key is refused"));
            assert_eq!(refused.code, ErrorCode::InvalidParams.code());
        }
    }
}
