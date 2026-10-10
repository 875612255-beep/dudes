//! The three primitives every secret in this contract shares, in one place.
//!
//! There are two secrets on this wire — the per-agent [`HookToken`](super::hook::HookToken) and
//! the [`OperatorCredential`](super::cred::OperatorCredential) — and each of them needs the same
//! three things: a well-formedness check applied on the way in, a `Debug` that prints nothing, and
//! an equality that does not leak how many leading characters matched.
//!
//! They live here rather than once per type because a second copy is a second thing that can be
//! got wrong, and the copy that is wrong is the one nobody reads. `subtle`'s
//! [`ConstantTimeEq`] is named by the code of **exactly one module** in this crate — this one — so
//! a type that stops being constant-time has to delete a call rather than quietly change a derive.
//! `tests/secret_shape.rs` scans for that over the code half of each source only, because the
//! whole-text version of it passed on a copy of this file with the call deleted and the sentences
//! describing the call left behind.

use std::fmt;

use subtle::ConstantTimeEq;

/// Whether `text` is exactly `len` characters of the base64url alphabet.
///
/// The alphabet check is on bytes, and `len` is a byte length: every character the alphabet admits
/// is one ASCII byte, so the two agree, and text that is not ASCII fails the alphabet before the
/// length can mean anything else.
pub(super) fn well_formed(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `Type(<redacted>)`, the only `Debug` a secret gets.
pub(super) fn redacted(type_name: &str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.write_str(type_name)?;
    f.write_str("(<redacted>)")
}

/// Constant-time equality over the bytes of two secrets.
///
/// Constant time in the *contents*, not in the length: `ct_eq` is defined for equal-length slices
/// and `subtle` returns a false choice for unequal ones, so a comparison still discloses whether
/// the lengths matched. Both secret types validate their length on the way in, so every value that
/// reaches here is the same length as every other value of its type, and the residual is that a
/// caller comparing across types would learn nothing it did not already know from the types.
pub(super) fn eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alphabet and the length are both enforced, and each is asserted with a case that fails
    /// on it alone — so a check that dropped one half would not be answered for by the other.
    #[test]
    fn well_formed_enforces_both_the_length_and_the_alphabet() {
        assert!(well_formed(&"a".repeat(43), 43), "positive control");
        assert!(well_formed("aA0-_", 5), "the whole alphabet is admitted");
        assert!(!well_formed(&"a".repeat(42), 43), "too short");
        assert!(!well_formed(&"a".repeat(44), 43), "too long");
        assert!(!well_formed("aaa+", 4), "`+` is base64, not base64url");
        assert!(!well_formed("aaa/", 4), "`/` is base64, not base64url");
        assert!(!well_formed("aaa=", 4), "padding is refused");
        assert!(
            !well_formed("aaaé", 4),
            "non-ascii is refused by the alphabet"
        );
    }

    /// Equality answers both ways round, so "constant-time" has not been implemented as "always
    /// false" — which would pass every refusal test in the repository.
    #[test]
    fn eq_is_true_for_equal_text_and_false_otherwise() {
        assert!(
            eq("abc", "abc"),
            "positive control: equal text compares equal"
        );
        assert!(!eq("abc", "abd"));
        assert!(!eq("abc", "abcd"), "a length difference is not equal");
    }

    /// The redaction prints the type's name and nothing of the value.
    #[test]
    fn redacted_prints_no_content() {
        struct Shown<'a>(
            #[expect(dead_code, reason = "held so the value could leak if it were printed")]
            &'a str,
        );
        impl fmt::Debug for Shown<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                redacted("Shown", f)
            }
        }
        let text = format!("{:?}", Shown("hunter2"));
        assert_eq!(text, "Shown(<redacted>)");
        assert!(!text.contains("hunter2"));
    }
}
