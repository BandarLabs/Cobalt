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
        })
    }
}
impl Clock for SystemClock {
    fn now(&self) -> Result<Snapshot, Error> {
        let snapshot = Snapshot {
            unix_millis: u64::try_from(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| Error::OutOfRange)?
                    .as_millis(),
            )
            .map_err(|_| Error::OutOfRange)?,
            monotonic_millis: u64::try_from(self.started.elapsed().as_millis())
                .map_err(|_| Error::OutOfRange)?,
            utc_offset_minutes: self.offset,
        };
        if snapshot.valid() {
            Ok(snapshot)
        } else {
            Err(Error::OutOfRange)
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
