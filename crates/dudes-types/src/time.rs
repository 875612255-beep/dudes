//! A point in time as the daemon's clock reports it. This crate never reads a clock; the
//! supervisor's `Clock` trait produces timestamps and the journal carries them.

use core::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// UTC milliseconds since the Unix epoch. An integer so it sorts, diffs and reads in `jq`.
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
pub struct Timestamp(i64);

impl Timestamp {
    /// The epoch itself.
    pub const UNIX_EPOCH: Self = Self(0);

    /// From milliseconds since the Unix epoch.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn unix_millis(self) -> i64 {
        self.0
    }

    /// This instant plus `millis`; saturates.
    #[must_use]
    pub const fn saturating_add_millis(self, millis: i64) -> Self {
        Self(self.0.saturating_add(millis))
    }

    /// Milliseconds from `earlier` to `self`; negative when `earlier` is later. Saturates.
    #[must_use]
    pub const fn millis_since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }

    /// The [`UtcDay`] this instant falls in.
    ///
    /// Floor division, not truncation toward zero, so the last millisecond before the epoch
    /// belongs to day `-1` rather than to day `0`. The boundary is UTC midnight, never the
    /// operator's: a day bucket has to be reproducible from the journal on any machine in any
    /// time zone, so the floor and the CLI render local time from the [`Timestamp`] instead.
    ///
    /// Total on every `i64`: a value further from the epoch than `i32` days can name is a
    /// corrupt record, and saturating keeps the bucket monotone so a projection never sees a
    /// day jump backwards. Not `const`, because saturating without a truncating cast needs
    /// [`i32::try_from`].
    #[must_use]
    pub fn utc_day(self) -> UtcDay {
        const MILLIS_PER_DAY: i64 = 86_400_000;
        let days = self.0.div_euclid(MILLIS_PER_DAY);
        UtcDay(i32::try_from(days).unwrap_or(if days.is_negative() {
            i32::MIN
        } else {
            i32::MAX
        }))
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// A UTC day, as whole days since the Unix epoch; negative before 1970-01-01.
///
/// The bucket every day-scoped view agrees on — `budget_by_day`, `spent_today`, `dudes why` —
/// computed from a [`Timestamp`] by [`Timestamp::utc_day`] and by nothing else, so two readers
/// of the same journal bucket identically. An integer, not a
/// date: this crate reads no clock and carries no calendar.
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
pub struct UtcDay(i32);

impl UtcDay {
    /// From whole days since the Unix epoch.
    #[must_use]
    pub const fn from_days_since_epoch(days: i32) -> Self {
        Self(days)
    }

    /// Whole days since the Unix epoch.
    #[must_use]
    pub const fn days_since_epoch(self) -> i32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Timestamps are integer milliseconds on the wire and compare as integers.
    #[test]
    fn integer_millis() {
        let t = Timestamp::from_unix_millis(1_787_394_600_000);
        assert_eq!(serde_json::to_string(&t).unwrap(), "1787394600000");
        assert_eq!(t.saturating_add_millis(500).millis_since(t), 500);
        assert!(t > Timestamp::UNIX_EPOCH);
    }

    /// the budget day is the **UTC** day, so the bucket is a pure
    /// function of the millis and replay is stable across machines and time zones. Two millis
    /// either side of a UTC midnight give adjacent days, and the division floors, so an
    /// instant before the epoch buckets with the day that contains it rather than the day
    /// after it.
    #[test]
    fn utc_day_steps_at_utc_midnight_and_floors_before_the_epoch() {
        const DAY: i64 = 86_400_000;
        assert_eq!(Timestamp::UNIX_EPOCH.utc_day().days_since_epoch(), 0);

        // 2026-08-23T00:00:00Z is day 20_688; one milli either side of that midnight.
        let midnight = Timestamp::from_unix_millis(20_688 * DAY);
        assert_eq!(midnight.utc_day(), UtcDay::from_days_since_epoch(20_688));
        assert_eq!(
            midnight.saturating_add_millis(-1).utc_day(),
            UtcDay::from_days_since_epoch(20_687),
            "the last milli of a day belongs to that day"
        );
        assert_eq!(
            midnight.saturating_add_millis(DAY - 1).utc_day(),
            UtcDay::from_days_since_epoch(20_688),
            "the whole day buckets together"
        );

        // Floor, not truncate-toward-zero: `-1 / DAY` is 0 in Rust, which would put the last
        // milli before the epoch in day 0 alongside the first milli after it.
        assert_eq!(
            Timestamp::from_unix_millis(-1).utc_day(),
            UtcDay::from_days_since_epoch(-1)
        );
        assert_eq!(
            Timestamp::from_unix_millis(-DAY).utc_day(),
            UtcDay::from_days_since_epoch(-1)
        );
        assert_eq!(
            Timestamp::from_unix_millis(-DAY - 1).utc_day(),
            UtcDay::from_days_since_epoch(-2)
        );

        // Total on every `i64`: a corrupt record saturates rather than wrapping, so a
        // projection never sees a day bucket jump backwards.
        assert_eq!(
            Timestamp::from_unix_millis(i64::MAX).utc_day(),
            UtcDay::from_days_since_epoch(i32::MAX)
        );
        assert_eq!(
            Timestamp::from_unix_millis(i64::MIN).utc_day(),
            UtcDay::from_days_since_epoch(i32::MIN)
        );
    }

    /// A `UtcDay` is a bare integer on the wire, and days order as integers.
    #[test]
    fn utc_day_is_an_integer_on_the_wire() {
        let day = UtcDay::from_days_since_epoch(20_688);
        assert_eq!(serde_json::to_string(&day).unwrap(), "20688");
        assert_eq!(serde_json::from_str::<UtcDay>("20688").unwrap(), day);
        assert!(day > UtcDay::from_days_since_epoch(20_687));
    }
}
