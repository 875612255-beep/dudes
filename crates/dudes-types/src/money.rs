//! Money and tokens as integers. No floats: a budget ceiling is compared exactly, and a sum of
//! many small spends must not drift.

use core::fmt;
use core::iter::Sum;
use core::ops::{Add, AddAssign};

use alloc::format;
use alloc::string::String;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// US dollars in micro-dollars (`1_000_000` = `$1`). Never negative: nothing in the contract
/// models a refund, so a cost below zero is a vendor bug and is refused at the boundary by
/// [`Usd::try_from_micros`] rather than subtracted from a spend. Arithmetic saturates rather than
/// wrapping.
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
#[serde(transparent)]
pub struct Usd(u64);

impl Usd {
    /// Micro-dollars per dollar.
    pub const MICROS_PER_DOLLAR: u64 = 1_000_000;
    /// Nothing.
    pub const ZERO: Self = Self(0);

    /// From micro-dollars.
    #[must_use]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// From a signed micro-dollar count as a vendor may report it. The boundary check: a
    /// negative cost is an error, never a credit.
    pub const fn try_from_micros(micros: i64) -> Result<Self, NegativeUsd> {
        if micros < 0 {
            Err(NegativeUsd { micros })
        } else {
            Ok(Self(micros.unsigned_abs()))
        }
    }

    /// From whole dollars; saturates.
    #[must_use]
    pub const fn from_dollars(dollars: u64) -> Self {
        Self(dollars.saturating_mul(Self::MICROS_PER_DOLLAR))
    }

    /// From cents; saturates.
    #[must_use]
    pub const fn from_cents(cents: u64) -> Self {
        Self(cents.saturating_mul(Self::MICROS_PER_DOLLAR / 100))
    }

    /// Micro-dollars.
    #[must_use]
    pub const fn micros(self) -> u64 {
        self.0
    }

    /// The decimal dollar amount with no currency sign and no thousands separator, always six
    /// fractional places: `Usd::from_cents(5)` is `"0.050000"`, not `"0.05"` and not `".05"`.
    ///
    /// This is the crate's only place that turns micro-dollars into decimal text.
    /// [`Display`](fmt::Display) prefixes it with `$`; a provider flag that takes dollars
    /// (Claude Code's `--max-budget-usd`) sends it as-is. Keeping one implementation is what
    /// stops the two from drifting — the sub-dollar case is where they would.
    #[must_use]
    pub fn to_decimal(self) -> String {
        format!(
            "{}.{:06}",
            self.0 / Self::MICROS_PER_DOLLAR,
            self.0 % Self::MICROS_PER_DOLLAR
        )
    }

    /// Dollars written as decimal text, as an operator writes a number in a TOML file: an optional
    /// leading `+`, digits with optional `_` separators, and at most six decimal places. `None` for
    /// anything else, a leading `-` included, and for an amount too large to hold.
    ///
    /// The daemon reads its configured dollars with this, and the CLI checks the dollars it writes
    /// with it, so the two cannot disagree on which text is a dollar amount.
    #[must_use]
    pub fn from_decimal(text: &str) -> Option<Self> {
        let text = text.strip_prefix('+').unwrap_or(text);
        if text.starts_with('-') {
            return None;
        }
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty() || fraction.contains('.') {
            return None;
        }
        let whole = decimal_digits(whole)?;
        let fraction: String = fraction
            .chars()
            .filter(|character| *character != '_')
            .collect();
        if fraction.len() > 6 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let fraction = if fraction.is_empty() {
            (0, 0)
        } else {
            decimal_digits(&fraction)?
        };
        let scale = 10_u64.checked_pow(u32::try_from(6 - fraction.1).ok()?)?;
        let micros = whole
            .0
            .checked_mul(Self::MICROS_PER_DOLLAR)?
            .checked_add(fraction.0.checked_mul(scale)?)?;
        Some(Self(micros))
    }

    /// Saturating addition.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Saturating subtraction: what remains of a ceiling, floored at zero.
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    /// Checked addition; `None` on overflow.
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(v) => Some(Self(v)),
            None => None,
        }
    }
}

/// The integer value and number of digits after removing TOML digit separators.
fn decimal_digits(raw: &str) -> Option<(u64, usize)> {
    let mut value = 0_u64;
    let mut digits = 0_usize;
    for byte in raw.bytes() {
        if byte == b'_' {
            continue;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
        digits += 1;
    }
    (digits != 0).then_some((value, digits))
}

impl Add for Usd {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Usd {
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl Sum for Usd {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Self::saturating_add)
    }
}

impl fmt::Display for Usd {
    /// `$1.234567`: the currency sign, then [`Usd::to_decimal`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${}", self.to_decimal())
    }
}

/// A vendor reported a cost below zero. Refused at the boundary ([`Usd::try_from_micros`]) so
/// it can never subtract from a spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("negative cost: {micros} micro-dollars")]
pub struct NegativeUsd {
    /// What the vendor reported.
    pub micros: i64,
}

/// A token count. Arithmetic saturates rather than wrapping.
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
#[serde(transparent)]
pub struct Tokens(u64);

impl Tokens {
    /// Nothing.
    pub const ZERO: Self = Self(0);

    /// From a count.
    #[must_use]
    pub const fn new(count: u64) -> Self {
        Self(count)
    }

    /// The count.
    #[must_use]
    pub const fn count(self) -> u64 {
        self.0
    }

    /// Saturating addition.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Add for Tokens {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        self.saturating_add(rhs)
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, rhs: Self) {
        *self = self.saturating_add(rhs);
    }
}

impl Sum for Tokens {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Self::saturating_add)
    }
}

impl fmt::Display for Tokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// What one turn (or one event) consumed.
///
/// Every field defaults so a record written before a field existed still reads. `usd` is absent
/// when the provider reports tokens but not cost; the budget gate then prices tokens itself or,
/// with no usage source at all, refuses admission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Usage {
    /// Prompt tokens that were not served from a cache.
    pub input: Tokens,
    /// Completion tokens.
    pub output: Tokens,
    /// Prompt tokens read from the vendor's prompt cache.
    pub cache_read: Tokens,
    /// Prompt tokens written to the vendor's prompt cache.
    pub cache_creation: Tokens,
    /// Cost as the vendor reported it, if it did.
    pub usd: Option<Usd>,
}

impl Usage {
    /// Nothing consumed.
    pub const ZERO: Self = Self {
        input: Tokens::ZERO,
        output: Tokens::ZERO,
        cache_read: Tokens::ZERO,
        cache_creation: Tokens::ZERO,
        usd: None,
    };

    /// Every token counted once, cached or not.
    #[must_use]
    pub const fn total_tokens(self) -> Tokens {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }

    /// Field-wise saturating sum. `usd` is `None` only when both sides are `None`.
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        let usd = match (self.usd, other.usd) {
            (None, None) => None,
            (Some(a), None) | (None, Some(a)) => Some(a),
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
        };
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_creation: self.cache_creation.saturating_add(other.cache_creation),
            usd,
        }
    }

    /// What this running total adds to an `earlier` one, or `None` when it fell: any token count
    /// below `earlier`'s, or a cost below `earlier`'s or no longer reported.
    #[must_use]
    pub fn checked_sub(self, earlier: Self) -> Option<Self> {
        let tokens =
            |now: Tokens, then: Tokens| now.count().checked_sub(then.count()).map(Tokens::new);
        let usd = match (self.usd, earlier.usd) {
            (now, None) => now,
            (Some(now), Some(then)) => {
                Some(Usd::from_micros(now.micros().checked_sub(then.micros())?))
            }
            (None, Some(_)) => return None,
        };
        Some(Self {
            input: tokens(self.input, earlier.input)?,
            output: tokens(self.output, earlier.output)?,
            cache_read: tokens(self.cache_read, earlier.cache_read)?,
            cache_creation: tokens(self.cache_creation, earlier.cache_creation)?,
            usd,
        })
    }
}

impl Add for Usage {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        self.saturating_add(rhs)
    }
}

impl Sum for Usage {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, Self::saturating_add)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;

    use super::*;

    /// Invariant 4 (budget is admission) compares money exactly: integer micro-dollars on the
    /// wire, saturating arithmetic, no float anywhere.
    #[test]
    fn usd_is_integer_micros_and_saturates() {
        assert_eq!(Usd::from_dollars(15).micros(), 15_000_000);
        assert_eq!(Usd::from_cents(199).micros(), 1_990_000);
        assert_eq!(
            serde_json::to_string(&Usd::from_dollars(15)).unwrap(),
            "15000000"
        );
        assert_eq!(
            serde_json::from_str::<Usd>("31575").unwrap(),
            Usd::from_micros(31_575)
        );
        assert!(serde_json::from_str::<Usd>("15.0").is_err());
        assert_eq!(
            Usd::from_micros(u64::MAX) + Usd::from_micros(1),
            Usd::from_micros(u64::MAX)
        );
        assert_eq!(
            Usd::from_micros(u64::MAX).checked_add(Usd::from_micros(1)),
            None
        );
        assert_eq!(
            Usd::from_cents(1).saturating_sub(Usd::from_cents(2)),
            Usd::ZERO
        );
        assert_eq!(Usd::from_micros(31_575).to_string(), "$0.031575");
        assert_eq!(Usd::from_micros(1_500_000).to_string(), "$1.500000");
        let total: Usd = [Usd::from_cents(1), Usd::from_cents(2)].into_iter().sum();
        assert_eq!(total, Usd::from_cents(3));
    }

    /// Sub-dollar money keeps its leading zero and all six places — `$0.05` is `"0.050000"`,
    /// never `"0.50000"` — and `Display` is exactly `to_decimal` with a `$` in front, so the
    /// text a provider flag carries and the text an operator reads cannot drift apart.
    ///
    /// The two halves prove different things and only one of them is about correctness. The
    /// literals pin the output; the loop pins only *agreement*, because `Display` calls
    /// `to_decimal`, so both sides of that assertion move together and no error inside
    /// `to_decimal` can ever fail it. Agreement is still worth asserting — it is what catches
    /// `Display` being given a second implementation later — but every value whose text
    /// matters needs a literal above, which is why the top of the range has one.
    #[test]
    fn to_decimal_pads_sub_dollar_money_and_display_is_it_with_a_sign() {
        assert_eq!(Usd::from_cents(5).to_decimal(), "0.050000");
        assert_eq!(Usd::from_micros(5).to_decimal(), "0.000005");
        assert_eq!(Usd::ZERO.to_decimal(), "0.000000");
        assert_eq!(Usd::from_cents(250).to_decimal(), "2.500000");
        assert_eq!(Usd::from_dollars(15).to_decimal(), "15.000000");
        // The top of the range, as a literal. Without it the loop below is the only cover the
        // largest value has, and that loop compares `Display` against `to_decimal` — both of
        // which call `to_decimal`, so no arithmetic error inside it can fail them. Narrowing
        // the division to `u32` renders this value `"4294.967295"`, and only this line fails.
        assert_eq!(
            Usd::from_micros(u64::MAX).to_decimal(),
            "18446744073709.551615"
        );
        for usd in [
            Usd::ZERO,
            Usd::from_micros(5),
            Usd::from_cents(5),
            Usd::from_cents(250),
            Usd::from_micros(31_575),
            Usd::from_micros(u64::MAX),
        ] {
            assert_eq!(
                usd.to_string(),
                alloc::format!("${}", usd.to_decimal()),
                "Display must be the sign plus to_decimal, not a second implementation"
            );
            assert!(!usd.to_decimal().contains('$'));
        }
    }

    #[test]
    fn from_decimal_reads_toml_dollar_text_and_refuses_the_rest() {
        for (text, micros) in [
            ("0", 0),
            ("0.25", 250_000),
            ("0.000001", 1),
            ("+1", 1_000_000),
            ("1_000.5_0", 1_000_500_000),
            ("18446744073709.551615", u64::MAX),
        ] {
            assert_eq!(
                Usd::from_decimal(text),
                Some(Usd::from_micros(micros)),
                "{text}"
            );
        }
        for text in [
            "",
            "-1",
            ".5",
            "1.2.3",
            "0.0000001",
            "1e3",
            "inf",
            "$1",
            "18446744073709.551616",
        ] {
            assert_eq!(Usd::from_decimal(text), None, "{text}");
        }
    }

    /// A negative vendor cost is rejected, never subtracted from spend: the signed boundary
    /// constructor refuses it, the wire refuses it, and nothing in the type can go below zero.
    #[test]
    fn negative_vendor_cost_is_rejected_never_subtracted_from_spend() {
        assert_eq!(Usd::try_from_micros(-1), Err(NegativeUsd { micros: -1 }));
        assert_eq!(
            Usd::try_from_micros(i64::MIN),
            Err(NegativeUsd { micros: i64::MIN })
        );
        assert_eq!(Usd::try_from_micros(0), Ok(Usd::ZERO));
        assert_eq!(Usd::try_from_micros(31_575), Ok(Usd::from_micros(31_575)));
        assert_eq!(
            NegativeUsd { micros: -7 }.to_string(),
            "negative cost: -7 micro-dollars"
        );
        assert!(serde_json::from_str::<Usd>("-1").is_err());
        assert!(serde_json::from_str::<Usage>(r#"{"usd":-5}"#).is_err());
        let spent = Usd::from_micros(100);
        let vendor_cost = Usd::try_from_micros(-50).ok();
        assert_eq!(spent + vendor_cost.unwrap_or(Usd::ZERO), spent);
        let schema = schemars::schema_for!(Usd);
        assert_eq!(schema.as_value()["minimum"], 0, "the schema refuses it too");
    }

    /// `Usage` reads with missing fields (older records, vendors that omit a counter) and sums
    /// field-wise; `usd` stays `None` only when no side had a cost.
    #[test]
    fn usage_defaults_and_sums() {
        let partial: Usage = serde_json::from_str(r#"{"input":10,"output":2}"#).unwrap();
        assert_eq!(partial.input, Tokens::new(10));
        assert_eq!(partial.cache_read, Tokens::ZERO);
        assert_eq!(partial.usd, None);
        let priced = Usage {
            usd: Some(Usd::from_micros(5)),
            cache_read: Tokens::new(100),
            ..Usage::ZERO
        };
        let sum = partial + priced;
        assert_eq!(sum.total_tokens(), Tokens::new(112));
        assert_eq!(sum.usd, Some(Usd::from_micros(5)));
        assert_eq!(
            Tokens::new(u64::MAX) + Tokens::new(1),
            Tokens::new(u64::MAX)
        );
    }
}
