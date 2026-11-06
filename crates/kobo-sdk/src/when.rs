//! Moments written the way people say them.
//!
//! A feed that says "Updated 2026-11-06 08:13" has handed the reader a log
//! line. Nobody reads a date like that: they read "today at eight", "yesterday",
//! "the third of November". [`spoken`] writes a moment that way, against the
//! reader's own clock, so "today" turns over at the owner's midnight rather
//! than Greenwich's. [`stamp_millis`] reads the timestamps services send, so an
//! application holding one can hand it straight over.

use crate::clock::Snapshot;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `moment`, in milliseconds since 1970, as it would be said at `now`.
///
/// "Today, 08:13" for earlier the same day on the reader's calendar, with
/// the time, because on the day itself the time is the news. "Yesterday" for
/// the day before, with no time, because by then the day is. "3 Nov" for
/// anything else this year, and "3 Nov 2025" for any other year. A moment
/// later today, from a clock a little ahead of the reader's, is still today;
/// one on a later day is written as its date.
///
/// `None` only when either moment is outside what a [`Snapshot`] can hold,
/// so a caller can fall back to saying nothing rather than something wrong.
#[must_use]
pub fn spoken(moment: u64, now: Snapshot) -> Option<String> {
    let then = Snapshot {
        unix_millis: moment,
        ..now
    };
    let (day, today) = (then.local_day()?, now.local_day()?);
    let date = then.date()?;
    if day == today {
        let (hour, minute) = then.hour_minute()?;
        return Some(format!("Today, {hour:02}:{minute:02}"));
    }
    if day + 1 == today {
        return Some("Yesterday".to_owned());
    }
    let month = MONTHS.get(usize::from(date.month).checked_sub(1)?)?;
    if date.year == now.date()?.year {
        Some(format!("{} {month}", date.day))
    } else {
        Some(format!("{} {month} {}", date.day, date.year))
    }
}

/// A timestamp as services write them, in milliseconds since 1970.
///
/// Reads `2026-11-06`, `2026-11-06T08:13`, `2026-11-06T08:13:27Z`, with a
/// space in place of the `T`, with fractions of a second, and with an offset
/// such as `+05:30` or `-0800`. A stamp with no offset is taken as UTC,
/// which is what a feed means by one far more often than not. `None` for
/// anything else, rather than a date that is a little bit wrong.
#[must_use]
pub fn stamp_millis(stamp: &str) -> Option<u64> {
    let stamp = stamp.trim();
    let number = |text: &str| -> Option<i64> {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let year = number(stamp.get(0..4)?)?;
    if stamp.get(4..5)? != "-" || stamp.get(7..8)? != "-" {
        return None;
    }
    let month = number(stamp.get(5..7)?)?;
    let day = number(stamp.get(8..10)?)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    let mut seconds = days_from_civil(year, month, day) * 86_400;
    let rest = &stamp[10..];
    let rest = match rest.chars().next() {
        None => "",
        Some('T' | 't' | ' ') => &rest[1..],
        Some(_) => return None,
    };
    let mut offset = 0;
    if !rest.is_empty() {
        let hour = number(rest.get(0..2)?)?;
        if rest.get(2..3)? != ":" {
            return None;
        }
        let minute = number(rest.get(3..5)?)?;
        let mut tail = &rest[5..];
        let mut second = 0;
        if let Some(after) = tail.strip_prefix(':') {
            second = number(after.get(0..2)?)?;
            tail = &after[2..];
            if let Some(fraction) = tail.strip_prefix('.') {
                let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
                if digits == 0 {
                    return None;
                }
                tail = &fraction[digits..];
            }
        }
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        seconds += hour * 3600 + minute * 60 + second;
        offset = match tail {
            "" | "Z" | "z" => 0,
            _ => {
                let sign = match tail.get(0..1)? {
                    "+" => 1,
                    "-" => -1,
                    _ => return None,
                };
                let digits = tail[1..].replace(':', "");
                if digits.len() != 4 {
                    return None;
                }
                let hours = number(&digits[..2])?;
                let minutes = number(&digits[2..])?;
                if hours > 14 || minutes > 59 {
                    return None;
                }
                sign * (hours * 3600 + minutes * 60)
            }
        };
    }
    u64::try_from(seconds - offset).ok()?.checked_mul(1000)
}

const fn leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1 January 1970 to the given date, on the proleptic Gregorian
/// calendar.
const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let shifted = (month + 9) % 12;
    let of_year = (153 * shifted + 2) / 5 + day - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

#[cfg(test)]
mod tests {
    use super::{spoken, stamp_millis};
    use crate::clock::Snapshot;

    /// 6 November 2026, 09:30 in London, which is UTC in November.
    const NOW: u64 = 1_793_957_400_000;

    fn at(utc_offset_minutes: i16) -> Snapshot {
        Snapshot {
            unix_millis: NOW,
            monotonic_millis: 0,
            utc_offset_minutes,
        }
    }

    fn said(stamp: &str, offset: i16) -> String {
        spoken(stamp_millis(stamp).expect("a stamp"), at(offset)).expect("a moment")
    }

    #[test]
    fn the_clock_this_test_stands_at_is_the_one_its_comment_names() {
        assert_eq!(stamp_millis("2026-11-06T09:30:00Z"), Some(NOW));
    }

    #[test]
    fn earlier_today_is_said_with_its_time() {
        assert_eq!(said("2026-11-06T08:13:00Z", 0), "Today, 08:13");
    }

    #[test]
    fn the_day_before_is_yesterday_and_needs_no_time() {
        assert_eq!(said("2026-11-05T23:59:00Z", 0), "Yesterday");
    }

    #[test]
    fn earlier_this_year_is_a_day_and_a_month() {
        assert_eq!(said("2026-11-03T12:00:00Z", 0), "3 Nov");
        assert_eq!(said("2026-01-31", 0), "31 Jan");
    }

    #[test]
    fn another_year_is_given_its_year() {
        assert_eq!(said("2025-11-03T12:00:00Z", 0), "3 Nov 2025");
    }

    #[test]
    fn today_turns_over_at_the_readers_midnight_rather_than_greenwichs() {
        // 23:30 UTC on the 5th is already 05:00 on the 6th in India.
        assert_eq!(said("2026-11-05T23:30:00Z", 330), "Today, 05:00");
        // And 01:00 UTC on the 6th is still the 5th in California.
        assert_eq!(said("2026-11-06T01:00:00Z", -480), "Yesterday");
    }

    #[test]
    fn a_stamp_is_read_with_its_own_offset_and_its_fractions() {
        assert_eq!(
            stamp_millis("2026-11-06T14:00:00+05:30"),
            stamp_millis("2026-11-06T08:30:00Z")
        );
        assert_eq!(
            stamp_millis("2026-11-06 00:30:00.250-0800"),
            stamp_millis("2026-11-06T08:30:00Z")
        );
        assert_eq!(
            stamp_millis("2026-11-06T08:13"),
            stamp_millis("2026-11-06T08:13:00Z")
        );
    }

    #[test]
    fn a_stamp_that_is_not_one_is_refused_rather_than_guessed_at() {
        for stamp in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-02-30",
            "2026-11-06T25:00",
            "2026-11-06T08:13+5",
            "06/11/2026",
        ] {
            assert_eq!(stamp_millis(stamp), None, "{stamp}");
        }
        assert!(stamp_millis("2024-02-29").is_some());
    }
}
