//! The time zones an owner can pick from, and the offset each one has at a
//! given moment.
//!
//! A reader's firmware may set no `TZ` and ship no zone files, and keep its
//! system clock in UTC, so the runtime cannot learn the owner's time from the
//! reader alone. The owner picks a city instead. Each city carries the POSIX
//! rule its zone follows today, daylight saving included, so the clock moves
//! on its own twice a year rather than being an hour out for half of it. The
//! rules are the zones' current ones; a city whose government changes its
//! rule needs a new release, which is also how a reader learns of anything
//! else.

/// One city on the list: its zone's name, the city as the owner reads it,
/// and the rule the zone follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Zone {
    /// The zone's standard name, such as `Asia/Kolkata`. This is what is
    /// stored, and what a lookup of the network's location answers with.
    pub id: &'static str,
    /// The city, as the list shows it.
    pub city: &'static str,
    /// The POSIX `TZ` rule for the zone as it is today.
    pub rule: &'static str,
}

const EU_CENTRAL: &str = "CET-1CEST,M3.5.0,M10.5.0/3";
const EU_EASTERN: &str = "EET-2EEST,M3.5.0/3,M10.5.0/4";
const US_EASTERN: &str = "EST5EDT,M3.2.0,M11.1.0";
const US_CENTRAL: &str = "CST6CDT,M3.2.0,M11.1.0";
const US_MOUNTAIN: &str = "MST7MDT,M3.2.0,M11.1.0";
const US_PACIFIC: &str = "PST8PDT,M3.2.0,M11.1.0";
const AU_EASTERN: &str = "AEST-10AEDT,M10.1.0,M4.1.0/3";

const fn zone(id: &'static str, city: &'static str, rule: &'static str) -> Zone {
    Zone { id, city, rule }
}

/// Every city on the list, in the order it is shown: by name.
pub const ZONES: &[Zone] = &[
    zone("Africa/Abidjan", "Abidjan", "GMT0"),
    zone("Africa/Accra", "Accra", "GMT0"),
    zone("Africa/Addis_Ababa", "Addis Ababa", "EAT-3"),
    zone(
        "Australia/Adelaide",
        "Adelaide",
        "ACST-9:30ACDT,M10.1.0,M4.1.0/3",
    ),
    zone("Africa/Algiers", "Algiers", "CET-1"),
    zone("Asia/Almaty", "Almaty", "<+05>-5"),
    zone("Europe/Amsterdam", "Amsterdam", EU_CENTRAL),
    zone("America/Anchorage", "Anchorage", "AKST9AKDT,M3.2.0,M11.1.0"),
    zone("Europe/Athens", "Athens", EU_EASTERN),
    zone(
        "Pacific/Auckland",
        "Auckland",
        "NZST-12NZDT,M9.5.0,M4.1.0/3",
    ),
    zone(
        "Atlantic/Azores",
        "Azores",
        "<-01>1<+00>,M3.5.0/0,M10.5.0/1",
    ),
    zone("Asia/Baghdad", "Baghdad", "<+03>-3"),
    zone("Asia/Baku", "Baku", "<+04>-4"),
    zone("Asia/Bangkok", "Bangkok", "<+07>-7"),
    zone("Asia/Beirut", "Beirut", "EET-2EEST,M3.5.0/0,M10.5.0/0"),
    zone("Europe/Belgrade", "Belgrade", EU_CENTRAL),
    zone("Europe/Berlin", "Berlin", EU_CENTRAL),
    zone("America/Bogota", "Bogotá", "<-05>5"),
    zone("Australia/Brisbane", "Brisbane", "AEST-10"),
    zone("Europe/Brussels", "Brussels", EU_CENTRAL),
    zone("Europe/Bucharest", "Bucharest", EU_EASTERN),
    zone("Europe/Budapest", "Budapest", EU_CENTRAL),
    zone("America/Argentina/Buenos_Aires", "Buenos Aires", "<-03>3"),
    zone("Africa/Cairo", "Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24"),
    zone("America/Caracas", "Caracas", "<-04>4"),
    zone("Africa/Casablanca", "Casablanca", "<+01>-1"),
    zone("America/Chicago", "Chicago", US_CENTRAL),
    zone("Asia/Colombo", "Colombo", "<+0530>-5:30"),
    zone("Europe/Copenhagen", "Copenhagen", EU_CENTRAL),
    zone("Australia/Darwin", "Darwin", "ACST-9:30"),
    zone("America/Denver", "Denver", US_MOUNTAIN),
    zone("Asia/Dhaka", "Dhaka", "<+06>-6"),
    zone("Asia/Qatar", "Doha", "<+03>-3"),
    zone("Asia/Dubai", "Dubai", "<+04>-4"),
    zone("Europe/Dublin", "Dublin", "IST-1GMT0,M10.5.0,M3.5.0/1"),
    zone("America/Edmonton", "Edmonton", US_MOUNTAIN),
    zone("Pacific/Fiji", "Fiji", "<+12>-12"),
    zone("America/Guatemala", "Guatemala City", "CST6"),
    zone("America/Halifax", "Halifax", "AST4ADT,M3.2.0,M11.1.0"),
    zone("America/Havana", "Havana", "CST5CDT,M3.2.0/0,M11.1.0/1"),
    zone("Europe/Helsinki", "Helsinki", EU_EASTERN),
    zone("Asia/Ho_Chi_Minh", "Ho Chi Minh City", "<+07>-7"),
    zone("Australia/Hobart", "Hobart", AU_EASTERN),
    zone("Asia/Hong_Kong", "Hong Kong", "HKT-8"),
    zone("Pacific/Honolulu", "Honolulu", "HST10"),
    zone("Europe/Istanbul", "Istanbul", "<+03>-3"),
    zone("Asia/Jakarta", "Jakarta", "WIB-7"),
    zone("Asia/Jerusalem", "Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0"),
    zone("Africa/Johannesburg", "Johannesburg", "SAST-2"),
    zone("Asia/Kabul", "Kabul", "<+0430>-4:30"),
    zone("Asia/Karachi", "Karachi", "PKT-5"),
    zone("Asia/Kathmandu", "Kathmandu", "<+0545>-5:45"),
    zone("Pacific/Kiritimati", "Kiritimati", "<+14>-14"),
    zone("Asia/Kolkata", "Kolkata", "IST-5:30"),
    zone("Asia/Kuala_Lumpur", "Kuala Lumpur", "<+08>-8"),
    zone("Europe/Kyiv", "Kyiv", EU_EASTERN),
    zone("America/La_Paz", "La Paz", "<-04>4"),
    zone("Africa/Lagos", "Lagos", "WAT-1"),
    zone("America/Lima", "Lima", "<-05>5"),
    zone("Europe/Lisbon", "Lisbon", "WET0WEST,M3.5.0/1,M10.5.0"),
    zone("Europe/London", "London", "GMT0BST,M3.5.0/1,M10.5.0"),
    zone("America/Los_Angeles", "Los Angeles", US_PACIFIC),
    zone("Europe/Madrid", "Madrid", EU_CENTRAL),
    zone("Asia/Manila", "Manila", "PST-8"),
    zone("Australia/Melbourne", "Melbourne", AU_EASTERN),
    zone("America/Mexico_City", "Mexico City", "CST6"),
    zone("Europe/Minsk", "Minsk", "<+03>-3"),
    zone("America/Montevideo", "Montevideo", "<-03>3"),
    zone("Europe/Moscow", "Moscow", "MSK-3"),
    zone("Africa/Nairobi", "Nairobi", "EAT-3"),
    zone("America/New_York", "New York", US_EASTERN),
    zone("Pacific/Noumea", "Nouméa", "<+11>-11"),
    zone("Asia/Novosibirsk", "Novosibirsk", "<+07>-7"),
    zone("Europe/Oslo", "Oslo", EU_CENTRAL),
    zone("America/Panama", "Panama City", "EST5"),
    zone("Europe/Paris", "Paris", EU_CENTRAL),
    zone("Australia/Perth", "Perth", "AWST-8"),
    zone("America/Phoenix", "Phoenix", "MST7"),
    zone("Pacific/Port_Moresby", "Port Moresby", "<+10>-10"),
    zone("Europe/Prague", "Prague", EU_CENTRAL),
    zone("America/Puerto_Rico", "Puerto Rico", "AST4"),
    zone("America/Regina", "Regina", "CST6"),
    zone("Atlantic/Reykjavik", "Reykjavík", "GMT0"),
    zone("Europe/Riga", "Riga", EU_EASTERN),
    zone("Asia/Riyadh", "Riyadh", "<+03>-3"),
    zone("Europe/Rome", "Rome", EU_CENTRAL),
    zone(
        "America/Santiago",
        "Santiago",
        "<-04>4<-03>,M9.1.6/24,M4.1.6/24",
    ),
    zone("America/Santo_Domingo", "Santo Domingo", "AST4"),
    zone("America/Sao_Paulo", "São Paulo", "<-03>3"),
    zone("Asia/Seoul", "Seoul", "KST-9"),
    zone("Asia/Shanghai", "Shanghai", "CST-8"),
    zone("Asia/Singapore", "Singapore", "<+08>-8"),
    zone("Europe/Sofia", "Sofia", EU_EASTERN),
    zone(
        "America/St_Johns",
        "St. John's",
        "NST3:30NDT,M3.2.0,M11.1.0",
    ),
    zone("Europe/Stockholm", "Stockholm", EU_CENTRAL),
    zone("Australia/Sydney", "Sydney", AU_EASTERN),
    zone("Asia/Taipei", "Taipei", "CST-8"),
    zone("Europe/Tallinn", "Tallinn", EU_EASTERN),
    zone("Asia/Tashkent", "Tashkent", "<+05>-5"),
    zone("Asia/Tbilisi", "Tbilisi", "<+04>-4"),
    zone("Asia/Tehran", "Tehran", "<+0330>-3:30"),
    zone("Asia/Tokyo", "Tokyo", "JST-9"),
    zone("Pacific/Tongatapu", "Tongatapu", "<+13>-13"),
    zone("America/Toronto", "Toronto", US_EASTERN),
    zone("Africa/Tunis", "Tunis", "CET-1"),
    zone("Asia/Ulaanbaatar", "Ulaanbaatar", "<+08>-8"),
    zone("America/Vancouver", "Vancouver", US_PACIFIC),
    zone("Europe/Vienna", "Vienna", EU_CENTRAL),
    zone("Europe/Vilnius", "Vilnius", EU_EASTERN),
    zone("Asia/Vladivostok", "Vladivostok", "<+10>-10"),
    zone("Europe/Warsaw", "Warsaw", EU_CENTRAL),
    zone("America/Winnipeg", "Winnipeg", US_CENTRAL),
    zone("Asia/Yangon", "Yangon", "<+0630>-6:30"),
    zone("Asia/Yekaterinburg", "Yekaterinburg", "<+05>-5"),
    zone("Asia/Yerevan", "Yerevan", "<+04>-4"),
    zone("Europe/Zurich", "Zurich", EU_CENTRAL),
];

/// The parts of the world the list is split into, so no one pages through
/// every city on Earth to find theirs.
pub const REGIONS: [&str; 5] = [
    "Africa",
    "Americas",
    "Asia",
    "Australia and Pacific",
    "Europe",
];

/// Which of [`REGIONS`] a zone is listed under: by its name's first part,
/// with the Atlantic islands under Europe and the Pacific with Australia.
#[must_use]
pub fn region_of(zone: &Zone) -> usize {
    match zone.id.split('/').next() {
        Some("Africa") => 0,
        Some("America") => 1,
        Some("Asia") => 2,
        Some("Australia" | "Pacific") => 3,
        _ => 4,
    }
}

/// The longest zone name accepted from anyone: the stored choice, a request,
/// or a lookup's answer.
pub const MAX_ZONE_ID_LEN: usize = 64;

/// The city on the list for a zone name, if it is one. Older names a lookup
/// may still answer with are taken as the zone they became.
#[must_use]
pub fn find(id: &str) -> Option<&'static Zone> {
    let id = id.trim();
    let id = match id {
        "Asia/Calcutta" => "Asia/Kolkata",
        "Europe/Kiev" => "Europe/Kyiv",
        "Asia/Saigon" => "Asia/Ho_Chi_Minh",
        "Asia/Katmandu" => "Asia/Kathmandu",
        "Asia/Rangoon" => "Asia/Yangon",
        "America/Buenos_Aires" => "America/Argentina/Buenos_Aires",
        other => other,
    };
    ZONES.iter().find(|zone| zone.id == id)
}

/// Minutes east of UTC in `zone` now, by the system clock.
#[must_use]
pub fn offset_now(zone: &Zone) -> Option<i16> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    offset_at(zone.rule, i64::try_from(now.as_secs()).ok()?)
}

/// The answer to a reading of the zone: the choice, if there is one, and
/// the offset the reader's clock is at, `fallback` when nothing was chosen
/// or the choice's rule could not be read.
#[must_use]
pub fn answer(zone: Option<&Zone>, fallback: i16) -> kobo_protocol::DeviceResult {
    kobo_protocol::DeviceResult::TimeZone {
        zone: zone.map(|zone| zone.id.to_owned()),
        offset_minutes: zone.and_then(offset_now).unwrap_or(fallback),
    }
}

/// Minutes east of UTC that `rule` gives at `unix_seconds`.
///
/// `None` when the rule is not one this understands, so a caller can fall
/// back rather than show a time that is confidently wrong.
#[must_use]
pub fn offset_at(rule: &str, unix_seconds: i64) -> Option<i16> {
    let rule = Rule::parse(rule)?;
    let Some(summer) = rule.summer else {
        return Some(rule.standard);
    };
    // The year as the zone sees it; a transition near New Year is measured
    // against the year it falls in.
    let local = unix_seconds + i64::from(rule.standard) * 60;
    let year = civil_from_days(local.div_euclid(86_400)).0;
    let begins = summer.begins.utc_seconds(year, rule.standard);
    let ends = summer.ends.utc_seconds(year, summer.offset);
    let in_summer = if begins < ends {
        (begins..ends).contains(&unix_seconds)
    } else {
        // Southern summers, and a zone whose "summer" is its winter, run
        // across New Year.
        !(ends..begins).contains(&unix_seconds)
    };
    Some(if in_summer {
        summer.offset
    } else {
        rule.standard
    })
}

/// A rule read apart: the standard offset, and daylight saving if it has any.
struct Rule {
    standard: i16,
    summer: Option<Summer>,
}

struct Summer {
    offset: i16,
    begins: Transition,
    ends: Transition,
}

/// `Mm.w.d/time`: day `d` of week `w` of month `m`, at `time` local.
struct Transition {
    month: u8,
    week: u8,
    weekday: u8,
    seconds: i64,
}

impl Rule {
    fn parse(rule: &str) -> Option<Self> {
        let mut rest = rule.trim();
        rest = skip_name(rest)?;
        let (standard, after) = offset(rest)?;
        rest = after;
        if rest.is_empty() {
            return Some(Self {
                standard,
                summer: None,
            });
        }
        rest = skip_name(rest)?;
        let (summer_offset, after) = if rest.starts_with(',') {
            (standard.checked_add(60)?, rest)
        } else {
            offset(rest)?
        };
        let rest = after.strip_prefix(',')?;
        let (begins, ends) = rest.split_once(',')?;
        Some(Self {
            standard,
            summer: Some(Summer {
                offset: summer_offset,
                begins: Transition::parse(begins)?,
                ends: Transition::parse(ends)?,
            }),
        })
    }
}

/// Past a zone abbreviation, plain or `<quoted>`.
fn skip_name(text: &str) -> Option<&str> {
    if let Some(quoted) = text.strip_prefix('<') {
        return quoted.get(quoted.find('>')? + 1..);
    }
    let end = text
        .find(|character: char| !character.is_ascii_alphabetic())
        .unwrap_or(text.len());
    (end >= 3).then(|| &text[end..])
}

/// A POSIX offset, which counts west, as minutes east, and what follows it.
fn offset(text: &str) -> Option<(i16, &str)> {
    let (sign, rest) = match text.as_bytes().first()? {
        b'-' => (1, &text[1..]),
        b'+' => (-1, &text[1..]),
        _ => (-1, text),
    };
    let end = rest
        .find(|character: char| !character.is_ascii_digit() && character != ':')
        .unwrap_or(rest.len());
    let seconds = clock_seconds(&rest[..end])?;
    let minutes = i16::try_from(seconds / 60).ok()?;
    let total = sign * minutes;
    (-900..=900)
        .contains(&total)
        .then_some((total, &rest[end..]))
}

/// `h[:mm[:ss]]` as seconds, hours up to 167 as POSIX allows.
fn clock_seconds(text: &str) -> Option<i64> {
    let mut parts = text.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    let seconds: i64 = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    let valid = parts.next().is_none()
        && (0..=167).contains(&hours)
        && (0..60).contains(&minutes)
        && (0..60).contains(&seconds);
    valid.then_some(hours * 3600 + minutes * 60 + seconds)
}

impl Transition {
    fn parse(text: &str) -> Option<Self> {
        let (date, time) = text.split_once('/').unwrap_or((text, "2"));
        let mut fields = date.strip_prefix('M')?.split('.');
        let month: u8 = fields.next()?.parse().ok()?;
        let week: u8 = fields.next()?.parse().ok()?;
        let weekday: u8 = fields.next()?.parse().ok()?;
        let (negative, time) = match time.strip_prefix('-') {
            Some(time) => (true, time),
            None => (false, time),
        };
        let seconds = clock_seconds(time)?;
        let valid = fields.next().is_none()
            && (1..=12).contains(&month)
            && (1..=5).contains(&week)
            && weekday <= 6;
        valid.then_some(Self {
            month,
            week,
            weekday,
            seconds: if negative { -seconds } else { seconds },
        })
    }

    /// When this happens in `year`, in UTC, given the offset in force just
    /// before it.
    fn utc_seconds(&self, year: i64, offset_before: i16) -> i64 {
        let first = days_from_civil(year, self.month, 1);
        // 1970-01-01 was a Thursday, weekday 4.
        let first_weekday = (first + 4).rem_euclid(7);
        let mut day = 1
            + (i64::from(self.weekday) - first_weekday).rem_euclid(7)
            + 7 * (i64::from(self.week) - 1);
        let length = days_in_month(year, self.month);
        while day > length {
            day -= 7;
        }
        let local = (first + day - 1) * 86_400 + self.seconds;
        local - i64::from(offset_before) * 60
    }
}

const fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

const fn days_in_month(year: i64, month: u8) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(year: i64, month: u8, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let month = i64::from(month);
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_cycle - 719_468
}

/// The civil date of a count of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let of_era = days - era * 146_097;
    let of_cycle = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * of_cycle + of_cycle / 4 - of_cycle / 100);
    let shifted = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted + 2) / 5 + 1;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    };
    let year = of_cycle + era * 400 + i64::from(month <= 2);
    (
        year,
        u8::try_from(month).unwrap_or(1),
        u8::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seconds since 1970 of a UTC date and time.
    fn utc(year: i64, month: u8, day: i64, hour: i64, minute: i64) -> i64 {
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60
    }

    fn at(id: &str, seconds: i64) -> i16 {
        offset_at(find(id).expect("on the list").rule, seconds).expect("understood")
    }

    #[test]
    fn every_city_on_the_list_has_a_rule_that_is_understood_all_year() {
        for zone in ZONES {
            for month in 1..=12 {
                assert!(
                    offset_at(zone.rule, utc(2026, month, 15, 12, 0)).is_some(),
                    "{} ({})",
                    zone.city,
                    zone.rule
                );
            }
            assert!(zone.id.len() <= MAX_ZONE_ID_LEN, "{}", zone.id);
        }
    }

    #[test]
    fn the_list_is_in_the_order_the_owner_reads_it_and_names_each_zone_once() {
        let fold = |city: &str| {
            city.chars()
                .map(|c| match c {
                    'á' | 'ã' => 'a',
                    'é' => 'e',
                    'í' => 'i',
                    other => other.to_ascii_lowercase(),
                })
                .collect::<String>()
        };
        for pair in ZONES.windows(2) {
            assert!(fold(pair[0].city) < fold(pair[1].city), "{pair:?}");
        }
        let mut ids: Vec<&str> = ZONES.iter().map(|zone| zone.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ZONES.len());
    }

    #[test]
    fn every_region_has_cities_and_every_city_a_region() {
        for (region, name) in REGIONS.iter().enumerate() {
            assert!(ZONES.iter().any(|zone| region_of(zone) == region), "{name}");
        }
        assert_eq!(region_of(find("Atlantic/Reykjavik").expect("listed")), 4);
        assert_eq!(region_of(find("Pacific/Honolulu").expect("listed")), 3);
        assert_eq!(region_of(find("America/St_Johns").expect("listed")), 1);
    }

    #[test]
    fn a_zone_without_daylight_saving_keeps_one_offset() {
        assert_eq!(at("Asia/Kolkata", utc(2026, 1, 1, 0, 0)), 330);
        assert_eq!(at("Asia/Kolkata", utc(2026, 7, 1, 0, 0)), 330);
        assert_eq!(at("Asia/Kathmandu", utc(2026, 7, 1, 0, 0)), 345);
        assert_eq!(at("America/Caracas", utc(2026, 7, 1, 0, 0)), -240);
        assert_eq!(at("Pacific/Kiritimati", utc(2026, 7, 1, 0, 0)), 840);
    }

    #[test]
    fn london_changes_at_one_in_the_morning_utc_on_the_last_sundays() {
        // 2026: summer from 29 March, back on 25 October, both at 01:00 UTC.
        assert_eq!(at("Europe/London", utc(2026, 3, 29, 0, 59)), 0);
        assert_eq!(at("Europe/London", utc(2026, 3, 29, 1, 0)), 60);
        assert_eq!(at("Europe/London", utc(2026, 10, 25, 0, 59)), 60);
        assert_eq!(at("Europe/London", utc(2026, 10, 25, 1, 0)), 0);
        assert_eq!(at("Europe/Berlin", utc(2026, 7, 1, 0, 0)), 120);
        assert_eq!(at("Europe/Berlin", utc(2026, 12, 1, 0, 0)), 60);
    }

    #[test]
    fn new_york_changes_on_the_second_sunday_of_march_and_first_of_november() {
        // 8 March 2026 at 02:00 EST is 07:00 UTC; 1 November at 02:00 EDT is
        // 06:00 UTC.
        assert_eq!(at("America/New_York", utc(2026, 3, 8, 6, 59)), -300);
        assert_eq!(at("America/New_York", utc(2026, 3, 8, 7, 0)), -240);
        assert_eq!(at("America/New_York", utc(2026, 11, 1, 5, 59)), -240);
        assert_eq!(at("America/New_York", utc(2026, 11, 1, 6, 0)), -300);
    }

    #[test]
    fn a_southern_summer_runs_across_new_year() {
        assert_eq!(at("Australia/Sydney", utc(2026, 1, 15, 0, 0)), 660);
        assert_eq!(at("Australia/Sydney", utc(2026, 7, 15, 0, 0)), 600);
        assert_eq!(at("Australia/Sydney", utc(2026, 12, 15, 0, 0)), 660);
        assert_eq!(at("Pacific/Auckland", utc(2026, 1, 15, 0, 0)), 780);
        assert_eq!(at("Pacific/Auckland", utc(2026, 7, 15, 0, 0)), 720);
        assert_eq!(at("Australia/Adelaide", utc(2026, 1, 15, 0, 0)), 630);
        assert_eq!(at("America/Santiago", utc(2026, 1, 15, 0, 0)), -180);
        assert_eq!(at("America/Santiago", utc(2026, 7, 15, 0, 0)), -240);
    }

    #[test]
    fn dublin_keeps_its_winter_as_the_exception() {
        assert_eq!(at("Europe/Dublin", utc(2026, 7, 1, 0, 0)), 60);
        assert_eq!(at("Europe/Dublin", utc(2026, 12, 1, 0, 0)), 0);
    }

    #[test]
    fn a_transition_past_midnight_is_read_as_the_rule_writes_it() {
        // Jerusalem: the Friday before the last Sunday of March, at 02:00,
        // written as 26:00 on the Thursday. 27 March 2026 02:00 IST is
        // 00:00 UTC.
        assert_eq!(at("Asia/Jerusalem", utc(2026, 3, 26, 23, 59)), 120);
        assert_eq!(at("Asia/Jerusalem", utc(2026, 3, 27, 0, 0)), 180);
    }

    #[test]
    fn older_names_are_taken_as_the_zones_they_became() {
        assert_eq!(find("Asia/Calcutta").map(|zone| zone.city), Some("Kolkata"));
        assert_eq!(find(" Europe/Kiev\n").map(|zone| zone.city), Some("Kyiv"));
        assert_eq!(find("Mars/Olympus_Mons"), None);
    }

    #[test]
    fn a_rule_that_is_not_understood_is_not_guessed_at() {
        assert_eq!(offset_at("", 0), None);
        assert_eq!(offset_at("EST5EDT,J60,J300", 0), None);
        assert_eq!(offset_at("EST5EDT,M13.1.0,M11.1.0", 0), None);
        assert_eq!(offset_at("XX", 0), None);
    }

    #[test]
    fn days_and_dates_agree_both_ways() {
        for days in [-1, 0, 59, 365, 10_957, 20_767, 24_855] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, i64::from(day)), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_767), (2026, 11, 10));
    }
}
