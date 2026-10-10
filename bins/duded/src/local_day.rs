//! The daemon host's local calendar day, for the Floor's finished-goals shelf.
//!
//! Read-side work only. The zone and the clock are read here, in the Floor stream, and nowhere in
//! `dudes_policy::decide()` or in any fold: the journal stores a zone-independent fact and this
//! module applies the host's zone when the daemon answers a read.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, TimeZone, Utc};
use dudes_supervisor::host::Clock;
use dudes_types::Timestamp;

/// One local calendar day as the host zone has it at a sampled instant.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayWindow {
    /// `YYYY-MM-DD`, the date of the sampled instant in the host zone.
    pub local_date: String,
    /// The host's offset from UTC at the sampled instant, in minutes east.
    pub utc_offset_minutes: i32,
    /// The first instant of that date in the host zone.
    pub start: Timestamp,
    /// The first instant of the next date, which is not part of this day.
    pub end: Timestamp,
}

/// The source of the daemon's current local day. Production reads the operating system's zone and
/// the daemon's clock; a test supplies a zone and a clock of its own.
#[doc(hidden)]
#[derive(Clone)]
pub struct LocalDays(Arc<dyn Fn() -> Option<DayWindow> + Send + Sync>);

impl LocalDays {
    /// The operating system's zone and offset, as they stand at each call, over the daemon's clock.
    /// Dudes bundles no timezone database and takes no zone from configuration, a record or a
    /// client. A host whose zone cannot be resolved has no day, and is never read as UTC.
    #[must_use]
    pub fn system<C: Clock + Send + Sync + 'static>(clock: Arc<C>) -> Self {
        Self::system_with(clock, |name| std::env::var(name).ok(), Path::exists)
    }

    /// [`LocalDays::system`] over an environment and a filesystem the caller supplies, so a test
    /// can stand in a host with no usable zone.
    #[must_use]
    pub fn system_with<C: Clock + Send + Sync + 'static>(
        clock: Arc<C>,
        env: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
        exists: impl Fn(&Path) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(move || {
            os_zone_resolves(env("TZ").as_deref(), &exists)
                .then(|| window_in(&chrono::Local, clock.now()))
                .flatten()
        }))
    }

    /// A zone read at each call, over a clock read at each call: a test's way to cross a date
    /// boundary, change zone while the daemon runs, or stand in a daylight-saving transition.
    #[must_use]
    pub fn from_fn(sample: impl Fn() -> Option<DayWindow> + Send + Sync + 'static) -> Self {
        Self(Arc::new(sample))
    }

    /// `None` when the zone gave no usable answer or the date is not representable.
    #[must_use]
    pub fn window(&self) -> Option<DayWindow> {
        (self.0)()
    }
}

impl fmt::Debug for LocalDays {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalDays").finish_non_exhaustive()
    }
}

/// Whether the operating system names a zone the way chrono reads one, so that its answer is the
/// host's and not the UTC chrono falls back to when nothing resolves.
///
/// Mirrors chrono's own sources on Unix: `TZ` as a zone file, a zoneinfo name or a POSIX rule, and
/// otherwise `/etc/localtime`. Other platforms ask the system directly and have no such fallback.
#[cfg(unix)]
fn os_zone_resolves(tz: Option<&str>, exists: &dyn Fn(&Path) -> bool) -> bool {
    let Some(tz) = tz else {
        return exists(Path::new("/etc/localtime"));
    };
    let name = tz.strip_prefix(':').unwrap_or(tz);
    if name.is_empty() {
        return true;
    }
    let file = if name.starts_with('/') {
        Path::new(name).to_path_buf()
    } else {
        Path::new("/usr/share/zoneinfo").join(name)
    };
    exists(&file) || is_posix_rule(name)
}

#[cfg(not(unix))]
fn os_zone_resolves(_tz: Option<&str>, _exists: &dyn Fn(&Path) -> bool) -> bool {
    true
}

/// A POSIX `TZ` rule: a standard-time name of three letters or more, then an offset.
#[cfg(unix)]
fn is_posix_rule(rule: &str) -> bool {
    let rest = rule.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    rule.len() - rest.len() >= 3
        && rest.starts_with(|c: char| c.is_ascii_digit() || c == '+' || c == '-')
}

/// The local day of `now` in `zone`, as the window `[start, end)` the finished-goals count reads.
///
/// A transition day is 23 or 25 hours and is neither averaged nor split. A date whose midnight
/// does not exist starts at the first instant that does, and a date whose midnight occurs twice
/// starts at its first occurrence, so each instant is in exactly one window.
#[doc(hidden)]
#[must_use]
pub fn window_in<Z: TimeZone>(zone: &Z, now: Timestamp) -> Option<DayWindow> {
    let local = DateTime::<Utc>::from_timestamp_millis(now.unix_millis())?.with_timezone(zone);
    let date = local.date_naive();
    if !(0..=9999).contains(&chrono::Datelike::year(&date)) {
        return None;
    }
    let start = first_instant(zone, date)?;
    let end = first_instant(zone, date.succ_opt()?)?;
    let offset_seconds = chrono::Offset::fix(local.offset()).local_minus_utc();
    Some(DayWindow {
        local_date: date.format("%Y-%m-%d").to_string(),
        utc_offset_minutes: offset_seconds / 60,
        start,
        end,
    })
}

/// The first instant at which `zone`'s clock reads `date` or later.
fn first_instant<Z: TimeZone>(zone: &Z, date: NaiveDate) -> Option<Timestamp> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    // A gap can swallow midnight and, in the rare case of a skipped date, the whole day. The first
    // local time that exists is then exactly the transition instant.
    (0..=2 * 24 * 60)
        .find_map(|minutes| {
            let probe: NaiveDateTime = midnight.checked_add_signed(Duration::minutes(minutes))?;
            match zone.from_local_datetime(&probe) {
                LocalResult::Single(at) => Some(at),
                LocalResult::Ambiguous(first, _) => Some(first),
                LocalResult::None => None,
            }
        })
        .map(|at| Timestamp::from_unix_millis(at.timestamp_millis()))
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use chrono_tz::Tz;

    use super::*;

    fn at_utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> Timestamp {
        let instant = Utc
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("a real instant");
        Timestamp::from_unix_millis(instant.timestamp_millis())
    }

    fn hours(window: &DayWindow) -> i64 {
        window.end.millis_since(window.start) / 3_600_000
    }

    fn zone(name: &str) -> Tz {
        name.parse().expect("a zone in the test database")
    }

    #[test]
    fn a_fixed_offset_day_is_twenty_four_hours_and_names_its_date_and_offset() {
        let plus13 = FixedOffset::east_opt(13 * 3600).unwrap();
        let window = window_in(&plus13, at_utc(2026, 10, 7, 0, 0)).unwrap();
        assert_eq!(window.local_date, "2026-10-07");
        assert_eq!(window.utc_offset_minutes, 780);
        assert_eq!(window.start, at_utc(2026, 10, 6, 11, 0));
        assert_eq!(window.end, at_utc(2026, 10, 7, 11, 0));
    }

    #[test]
    fn an_auckland_host_has_its_utc_midnight_in_the_middle_of_its_afternoon() {
        let auckland = zone("Pacific/Auckland");
        let window = window_in(&auckland, at_utc(2026, 10, 7, 0, 0)).unwrap();
        assert_eq!(
            window.local_date, "2026-10-07",
            "UTC midnight is 13:00 there"
        );
        assert_eq!(window.utc_offset_minutes, 780);
        assert_eq!(hours(&window), 24);
        let before = window_in(&auckland, at_utc(2026, 10, 7, 10, 59)).unwrap();
        let after = window_in(&auckland, at_utc(2026, 10, 7, 11, 0)).unwrap();
        assert_eq!(before.local_date, "2026-10-07");
        assert_eq!(after.local_date, "2026-10-08");
        assert_eq!(before.end, after.start, "adjacent days share one boundary");
    }

    #[test]
    fn a_spring_forward_day_is_twenty_three_hours_and_a_fall_back_day_twenty_five() {
        let new_york = zone("America/New_York");
        let spring = window_in(&new_york, at_utc(2026, 3, 8, 12, 0)).unwrap();
        assert_eq!(
            (spring.local_date.as_str(), hours(&spring)),
            ("2026-03-08", 23)
        );
        assert_eq!(spring.start, at_utc(2026, 3, 8, 5, 0));
        assert_eq!(spring.end, at_utc(2026, 3, 9, 4, 0));
        let fall = window_in(&new_york, at_utc(2026, 11, 1, 12, 0)).unwrap();
        assert_eq!((fall.local_date.as_str(), hours(&fall)), ("2026-11-01", 25));
        assert_eq!(fall.start, at_utc(2026, 11, 1, 4, 0));
        assert_eq!(fall.end, at_utc(2026, 11, 2, 5, 0));
        let ordinary = window_in(&new_york, at_utc(2026, 6, 1, 12, 0)).unwrap();
        assert_eq!(hours(&ordinary), 24);
    }

    #[test]
    fn a_nonexistent_midnight_starts_at_the_first_instant_that_exists() {
        let sao_paulo = zone("America/Sao_Paulo");
        // Clocks jumped from 00:00 to 01:00 on 2018-11-04, so that date has no midnight.
        let day = window_in(&sao_paulo, at_utc(2018, 11, 4, 12, 0)).unwrap();
        assert_eq!(day.local_date, "2018-11-04");
        assert_eq!(day.start, at_utc(2018, 11, 4, 3, 0), "01:00 at UTC-2");
        assert_eq!(hours(&day), 23);
        let eve = window_in(&sao_paulo, at_utc(2018, 11, 4, 2, 59)).unwrap();
        assert_eq!(eve.local_date, "2018-11-03");
        assert_eq!(eve.end, day.start, "no instant falls between the two days");
    }

    #[test]
    fn a_repeated_midnight_starts_at_its_first_occurrence_and_counts_the_hour_once() {
        let havana = zone("America/Havana");
        // Clocks fell back from 01:00 to 00:00 on 2026-11-01, so 00:00 to 01:00 happens twice.
        let first = window_in(&havana, at_utc(2026, 11, 1, 4, 30)).unwrap();
        let second = window_in(&havana, at_utc(2026, 11, 1, 5, 30)).unwrap();
        assert_eq!(first.local_date, second.local_date);
        assert_eq!(
            (first.start, first.end),
            (second.start, second.end),
            "both passes through the repeated hour are one day"
        );
        assert_eq!(first.start, at_utc(2026, 11, 1, 4, 0));
        assert_eq!(hours(&first), 25);
        assert_ne!(
            first.utc_offset_minutes, second.utc_offset_minutes,
            "the offset moves, which is what makes the daemon resync"
        );
    }

    #[test]
    fn a_date_the_zone_skipped_ends_the_day_before_it_at_the_jump() {
        let apia = zone("Pacific/Apia");
        // Samoa went from 2011-12-29 to 2011-12-31, so 12-30 never happened.
        let before = window_in(&apia, at_utc(2011, 12, 30, 9, 0)).unwrap();
        assert_eq!(before.local_date, "2011-12-29");
        let after = window_in(&apia, at_utc(2011, 12, 30, 11, 0)).unwrap();
        assert_eq!(after.local_date, "2011-12-31");
        assert_eq!(before.end, after.start);
    }

    #[test]
    fn every_instant_is_in_exactly_one_window_across_a_year_of_transitions() {
        for name in [
            "America/New_York",
            "Pacific/Auckland",
            "Australia/Lord_Howe",
            "America/Havana",
        ] {
            let zone = zone(name);
            let mut now = at_utc(2026, 1, 1, 0, 0);
            let stop = at_utc(2027, 1, 1, 0, 0);
            while now < stop {
                let window = window_in(&zone, now).unwrap();
                assert!(window.start <= now && now < window.end, "{name} {now}");
                let next = window_in(&zone, window.end).unwrap();
                assert_eq!(next.start, window.end, "{name}: windows tile at {now}");
                assert_ne!(next.local_date, window.local_date);
                now = now.saturating_add_millis(1_800_000);
            }
        }
    }

    #[test]
    fn a_date_beyond_four_digits_is_unavailable_rather_than_wrong() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert!(window_in(&utc, at_utc(9999, 12, 31, 23, 59)).is_some());
        assert!(window_in(&utc, at_utc(10_000, 1, 1, 0, 0)).is_none());
        assert!(window_in(&utc, Timestamp::from_unix_millis(i64::MAX)).is_none());
    }

    #[cfg(unix)]
    struct Noon;
    #[cfg(unix)]
    impl Clock for Noon {
        fn now(&self) -> Timestamp {
            at_utc(2026, 10, 7, 12, 0)
        }
    }

    /// A host with no usable zone has no day. Without the check chrono answers with UTC, and the
    /// shelf would show a UTC day as if it were the host's.
    #[cfg(unix)]
    #[test]
    fn a_host_with_no_resolvable_zone_has_no_day_and_is_never_read_as_utc() {
        let days = |tz: Option<&'static str>, exists: fn(&Path) -> bool| {
            LocalDays::system_with(
                Arc::new(Noon),
                move |name| (name == "TZ").then(|| tz.map(str::to_owned)).flatten(),
                exists,
            )
        };
        assert!(days(Some("Nowhere/Invalid"), |_| false).window().is_none());
        assert!(
            days(None, |_| false).window().is_none(),
            "no TZ and no /etc/localtime"
        );
        assert!(
            days(Some(":"), |_| false).window().is_some(),
            "an empty TZ is UTC by definition"
        );
        assert!(
            days(Some("Pacific/Auckland"), |p| p
                == Path::new("/usr/share/zoneinfo/Pacific/Auckland"))
            .window()
            .is_some()
        );
        assert!(
            days(None, |p| p == Path::new("/etc/localtime"))
                .window()
                .is_some()
        );
        assert!(
            days(Some("NZST-12NZDT,M9.5.0,M4.1.0/3"), |_| false)
                .window()
                .is_some(),
            "a POSIX rule"
        );
        assert!(
            days(Some("/zones/mine"), |p| p == Path::new("/zones/mine"))
                .window()
                .is_some()
        );
        assert!(
            days(Some("ab1"), |_| false).window().is_none(),
            "a name shorter than three letters is no rule"
        );
    }
}
