//! Injectable civil and monotonic time. Wall-clock corrections never alter
//! elapsed time; a fixed UTC offset is explicit, not a guessed time zone.
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const LAST_MILLISECOND: u64 = 253_402_300_799_999;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub unix_millis: u64,
    pub monotonic_millis: u64,
    pub utc_offset_minutes: i16,
}
impl Snapshot {
    #[must_use]
    pub fn valid(self) -> bool {
        self.unix_millis <= LAST_MILLISECOND && (-840..=840).contains(&self.utc_offset_minutes)
    }
    /// Local civil date at this snapshot's explicit offset, from 1970 to 9999.
    #[must_use]
    pub fn date(self) -> Option<Date> {
        if !self.valid() {
            return None;
        }
        let local = i64::try_from(self.unix_millis / 1000)
            .ok()?
            .checked_add(i64::from(self.utc_offset_minutes) * 60)?;
        let mut days = u64::try_from(local).ok()? / 86_400;
        let mut year = 1970_u16;
        loop {
            let length = if leap(year) { 366 } else { 365 };
            if days < length {
                break;
            }
            days -= length;
            year = year.checked_add(1)?;
            if year > 9999 {
                return None;
            }
        }
        let lengths = [
            31,
            if leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        for (index, length) in lengths.into_iter().enumerate() {
            if days < length {
                return Some(Date {
                    year,
                    month: u8::try_from(index + 1).ok()?,
                    day: u8::try_from(days + 1).ok()?,
                });
            }
            days -= length;
        }
        None
    }
    /// Days since 1 January 1970 on the reader's own calendar.
    ///
    /// A streak, a due date or "today's puzzle" turns over at the owner's
    /// midnight, not Greenwich's. Counting in UTC put an evening check-in in
    /// Sydney on tomorrow and one in California on the day before.
    #[must_use]
    pub fn local_day(self) -> Option<i64> {
        if !self.valid() {
            return None;
        }
        let local = i64::try_from(self.unix_millis / 1000)
            .ok()?
            .checked_add(i64::from(self.utc_offset_minutes) * 60)?;
        Some(local.div_euclid(86_400))
    }
    #[must_use]
    pub fn hour_minute(self) -> Option<(u8, u8)> {
        self.date()?;
        let seconds =
            i64::try_from(self.unix_millis / 1000).ok()? + i64::from(self.utc_offset_minutes) * 60;
        Some((
            u8::try_from(seconds.rem_euclid(86_400) / 3600).ok()?,
            u8::try_from(seconds.rem_euclid(3600) / 60).ok()?,
        ))
    }
}
fn leap(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}
impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    OutOfRange,
    Unavailable,
}
pub trait Clock: Send + Sync {
    /// # Errors
    /// Returns unavailable or out-of-range time instead of inventing a date.
    fn now(&self) -> Result<Snapshot, Error>;
}

#[derive(Debug)]
pub struct SystemClock {
    started: Instant,
    offset: i16,
    /// Wall time at `started`, when a simulator chose it instead of the
    /// computer's clock.
    origin: Option<u64>,
}
impl SystemClock {
    /// # Errors
    /// Refuses UTC offsets outside -14 to +14 hours.
    pub fn new(utc_offset_minutes: i16) -> Result<Self, Error> {
        if !(-840..=840).contains(&utc_offset_minutes) {
            return Err(Error::OutOfRange);
        }
        Ok(Self {
            started: Instant::now(),
            offset: utc_offset_minutes,
            origin: None,
        })
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Result<Snapshot, Error> {
        let monotonic_millis =
            u64::try_from(self.started.elapsed().as_millis()).map_err(|_| Error::OutOfRange)?;
        let unix_millis = match self.origin {
            Some(origin) => origin
                .checked_add(monotonic_millis)
                .ok_or(Error::OutOfRange)?,
            None => u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| Error::OutOfRange)?
                    .as_millis(),
            )
            .map_err(|_| Error::OutOfRange)?,
        };
        let snapshot = Snapshot {
            unix_millis,
            monotonic_millis,
            utc_offset_minutes: self.offset,
        };
        if snapshot.valid() {
            Ok(snapshot)
        } else {
            Err(Error::OutOfRange)
        }
    }
}
/// The variable the runtime sets on every application to the reader's own
/// offset from UTC, in minutes east.
///
/// Applications start with an empty environment, so the firmware's `TZ` never
/// reaches them. The runtime reads it once at launch and hands over the
/// number, which is all an application needs and cannot be misparsed twice.
pub const READER_OFFSET_VARIABLE: &str = "KOBO_UTC_OFFSET_MINUTES";

/// The variable a simulator sets to start the wall clock at a chosen moment,
/// in milliseconds since 1970.
///
/// The simulator's own clock already follows it, but an application read the
/// computer's clock, so two runs meant to fall a day apart showed the same
/// day's puzzle or poem. Read only by [`SystemClock::reader`]; the runtime
/// starts applications with an empty environment, so a reader never sets it.
pub const SIMULATED_START_VARIABLE: &str = "KOBO_SIM_CLOCK_MILLIS";

/// Minutes east of UTC for a POSIX `TZ` value such as `EST5` or `IST-5:30`.
///
/// POSIX inverts the sign from what everyone expects: `EST5` is five hours
/// *behind* UTC. Only the standard-time offset is read. A daylight rule is
/// ignored rather than guessed at, which leaves a clock an hour out for half
/// the year instead of a whole timezone out all year. Anything not understood
/// is `None`, so a caller can tell "UTC" from "unknown".
#[must_use]
pub fn posix_offset_minutes(tz: &str) -> Option<i16> {
    let tz = tz.trim();
    // A name that is not plain letters is quoted, as in `<+0545>-5:45`.
    let rest = match tz.strip_prefix('<') {
        Some(quoted) => quoted.get(quoted.find('>')? + 1..)?,
        None => tz.trim_start_matches(|character: char| character.is_ascii_alphabetic()),
    };
    let (sign, digits) = match rest.strip_prefix('-') {
        Some(digits) => (1, digits),
        None => (-1, rest.strip_prefix('+').unwrap_or(rest)),
    };
    let digits = digits
        .split(|character: char| !character.is_ascii_digit() && character != ':')
        .next()?;
    let mut parts = digits.split(':');
    let hours = parts.next()?.parse::<i16>().ok()?;
    let minutes = match parts.next() {
        Some(part) => part.parse::<i16>().ok()?,
        None => 0,
    };
    let total = sign * hours.checked_mul(60)?.checked_add(minutes)?;
    (-840..=840).contains(&total).then_some(total)
}

/// The reader's offset from UTC as the runtime passed it, or UTC when the
/// runtime is too old to pass one or the value is out of range.
#[must_use]
pub fn reader_offset_minutes() -> i16 {
    std::env::var(READER_OFFSET_VARIABLE)
        .ok()
        .and_then(|value| value.trim().parse::<i16>().ok())
        .filter(|minutes| (-840..=840).contains(minutes))
        .unwrap_or(0)
}

impl SystemClock {
    /// The wall clock at the reader's own offset.
    ///
    /// Never fails: an offset the runtime should not have sent falls back to
    /// UTC, because a day boundary a few hours out is a smaller harm than an
    /// application that cannot tell the time at all.
    #[must_use]
    pub fn reader() -> Self {
        Self {
            started: Instant::now(),
            offset: reader_offset_minutes(),
            origin: std::env::var(SIMULATED_START_VARIABLE)
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|&millis| millis <= LAST_MILLISECOND),
        }
    }
}

/// Only advances when explicitly requested. Reading it has no side effects.
#[derive(Debug)]
pub struct ManualClock {
    snapshot: Mutex<Snapshot>,
}
impl ManualClock {
    /// # Errors
    /// Refuses an invalid civil timestamp or UTC offset.
    pub fn new(snapshot: Snapshot) -> Result<Self, Error> {
        if !snapshot.valid() {
            return Err(Error::OutOfRange);
        }
        Ok(Self {
            snapshot: Mutex::new(snapshot),
        })
    }
    /// # Errors
    /// Refuses overflow or a poisoned clock; no partial update occurs.
    pub fn advance(&self, duration: Duration) -> Result<Snapshot, Error> {
        let millis = u64::try_from(duration.as_millis()).map_err(|_| Error::OutOfRange)?;
        let mut current = self.snapshot.lock().map_err(|_| Error::Unavailable)?;
        let next = Snapshot {
            unix_millis: current
                .unix_millis
                .checked_add(millis)
                .ok_or(Error::OutOfRange)?,
            monotonic_millis: current
                .monotonic_millis
                .checked_add(millis)
                .ok_or(Error::OutOfRange)?,
            ..*current
        };
        if !next.valid() {
            return Err(Error::OutOfRange);
        }
        *current = next;
        Ok(next)
    }
    /// # Errors
    /// Refuses invalid wall time or offset; monotonic time is preserved.
    pub fn set_wall(&self, unix_millis: u64, utc_offset_minutes: i16) -> Result<Snapshot, Error> {
        let mut current = self.snapshot.lock().map_err(|_| Error::Unavailable)?;
        let next = Snapshot {
            unix_millis,
            utc_offset_minutes,
            ..*current
        };
        if !next.valid() {
            return Err(Error::OutOfRange);
        }
        *current = next;
        Ok(next)
    }
}
impl Clock for ManualClock {
    fn now(&self) -> Result<Snapshot, Error> {
        self.snapshot
            .lock()
            .map(|value| *value)
            .map_err(|_| Error::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn civil_dates_cover_leap_centuries_offsets_and_boundaries() {
        for (seconds, offset, date) in [
            (0, 0, "1970-01-01"),
            (951_782_400, 0, "2000-02-29"),
            (4_107_542_400, 0, "2100-03-01"),
            (1_704_067_200, -60, "2023-12-31"),
            (1_704_063_600, 330, "2024-01-01"),
        ] {
            let snapshot = Snapshot {
                unix_millis: seconds * 1000,
                monotonic_millis: 0,
                utc_offset_minutes: offset,
            };
            assert_eq!(snapshot.date().unwrap().to_string(), date);
        }
        assert!(Snapshot {
            unix_millis: 0,
            monotonic_millis: 0,
            utc_offset_minutes: -1
        }
        .date()
        .is_none());
        assert!(Snapshot {
            unix_millis: 0,
            monotonic_millis: 0,
            utc_offset_minutes: i16::MIN
        }
        .date()
        .is_none());
    }
    #[test]
    fn posix_offsets_read_with_the_sign_posix_gives_them() {
        assert_eq!(posix_offset_minutes("EST5"), Some(-300));
        assert_eq!(posix_offset_minutes("EST5EDT,M3.2.0,M11.1.0"), Some(-300));
        assert_eq!(posix_offset_minutes("IST-5:30"), Some(330));
        assert_eq!(
            posix_offset_minutes("AEST-10AEDT,M10.1.0,M4.1.0/3"),
            Some(600)
        );
        assert_eq!(posix_offset_minutes("UTC0"), Some(0));
        assert_eq!(posix_offset_minutes("<+0545>-5:45"), Some(345));
        assert_eq!(posix_offset_minutes("Europe/London"), None);
        assert_eq!(posix_offset_minutes("XYZ-15"), None);
    }
    #[test]
    fn a_clock_started_at_a_chosen_moment_runs_on_from_it() {
        let clock = SystemClock {
            started: Instant::now(),
            offset: 0,
            origin: Some(1_704_067_200_000),
        };
        let now = clock.now().unwrap();
        assert!(now.unix_millis >= 1_704_067_200_000);
        assert!(now.unix_millis - 1_704_067_200_000 < 60_000);
        assert_eq!(now.date().unwrap().to_string(), "2024-01-01");
    }
    #[test]
    fn the_local_day_turns_over_at_the_readers_midnight() {
        // 23:30 UTC on 1 January 2024 is still the 1st in London and already
        // the 2nd in Sydney.
        let late = |offset| Snapshot {
            unix_millis: (1_704_067_200 + 23 * 3600 + 1800) * 1000,
            monotonic_millis: 0,
            utc_offset_minutes: offset,
        };
        assert_eq!(late(0).local_day(), Some(19_723));
        assert_eq!(late(600).local_day(), Some(19_724));
        assert_eq!(late(-480).local_day(), Some(19_723));
    }
    #[test]
    fn wall_corrections_do_not_change_elapsed_time_and_overflow_is_atomic() {
        let clock = ManualClock::new(Snapshot {
            unix_millis: 100_000,
            monotonic_millis: 0,
            utc_offset_minutes: 0,
        })
        .unwrap();
        clock.advance(Duration::from_secs(2)).unwrap();
        clock.set_wall(50_000, 330).unwrap();
        assert_eq!(clock.now().unwrap().monotonic_millis, 2000);
        let before = clock.now().unwrap();
        assert!(clock.advance(Duration::MAX).is_err());
        assert_eq!(clock.now().unwrap(), before);
        assert!(clock.set_wall(u64::MAX, 0).is_err());
        assert_eq!(clock.now().unwrap(), before);
    }
}
