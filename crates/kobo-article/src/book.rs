//! The title page, the style sheet, and the file name.

use std::fmt::Write as _;

use crate::body::escape;

/// A light style sheet. The reader's own type settings rule the text; this
/// only gives figures, captions, code and tables the shape a reader expects.
pub const STYLESHEET: &str = "\
.title-page { margin-top: 2em; }
.title-page h1 { margin: 0.4em 0 0.6em 0; }
.site { font-size: 0.85em; letter-spacing: 0.05em; }
.byline, .date { margin: 0.2em 0; font-style: italic; }
.source { font-size: 0.8em; margin-top: 2em; word-wrap: break-word; }
figure { margin: 1em 0; text-align: center; }
figure img, .lead img { max-width: 100%; }
figcaption { font-size: 0.85em; font-style: italic; margin-top: 0.4em; }
blockquote { margin: 1em 1.5em; }
pre { white-space: pre-wrap; font-size: 0.85em; }
table { border-collapse: collapse; margin: 1em 0; }
th, td { border: 1px solid #888; padding: 0.2em 0.4em; vertical-align: top; }
";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// A `YYYY-MM-DD` date as a reader would write it: "5 November 2026" in
/// English, and the date as given in any other language, where an English
/// month would be wrong.
#[must_use]
pub fn spoken_date(date: &str, language: &str) -> String {
    let english = language.is_empty() || language.to_ascii_lowercase().starts_with("en");
    let parts: Vec<&str> = date.split('-').collect();
    if let (true, [year, month, day]) = (english, parts.as_slice()) {
        if let (Ok(month), Ok(day)) = (month.parse::<usize>(), day.parse::<u32>()) {
            if let Some(name) = month.checked_sub(1).and_then(|index| MONTHS.get(index)) {
                return format!("{day} {name} {year}");
            }
        }
    }
    date.to_owned()
}

/// What the first page says about the article.
pub struct TitlePage<'a> {
    pub title: &'a str,
    pub site: Option<&'a str>,
    pub byline: Option<&'a str>,
    pub date: Option<&'a str>,
    pub source: &'a str,
    /// The lead picture's file name, when it is shown here.
    pub lead: Option<&'a str>,
}

/// Writes the title page.
#[must_use]
pub fn title_page(page: &TitlePage<'_>) -> String {
    let mut out = String::from("<section class=\"title-page\" epub:type=\"titlepage\">\n");
    if let Some(site) = page.site {
        let _ = writeln!(out, "<p class=\"site\">{}</p>", escape(site));
    }
    let _ = writeln!(out, "<h1>{}</h1>", escape(page.title));
    if let Some(byline) = page.byline {
        let _ = writeln!(out, "<p class=\"byline\">{}</p>", escape(byline));
    }
    if let Some(date) = page.date {
        let _ = writeln!(out, "<p class=\"date\">{}</p>", escape(date));
    }
    if let Some(lead) = page.lead {
        let _ = writeln!(
            out,
            "<figure class=\"lead\"><img src=\"{}\" alt=\"\"/></figure>",
            escape(lead)
        );
    }
    let _ = writeln!(
        out,
        "<p class=\"source\">Saved from <a href=\"{0}\">{0}</a></p>",
        escape(page.source)
    );
    out.push_str("</section>");
    out
}

/// The longest file name stem, in characters. Long enough to say what the
/// article is, short enough for every file system the reader's card uses.
const MAX_STEM: usize = 80;

/// A file name for the book, from its title.
///
/// Readable rather than slugged, since this is what the owner sees in a
/// folder, but stripped of the characters FAT file systems refuse.
#[must_use]
pub fn file_name(title: &str) -> String {
    let mut stem = String::new();
    for character in title.chars() {
        let character = match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            control if control.is_control() => ' ',
            other => other,
        };
        if character == ' ' && (stem.is_empty() || stem.ends_with(' ')) {
            continue;
        }
        stem.push(character);
    }
    let mut stem = stem.trim().trim_matches('.').trim().to_owned();
    if stem.chars().count() > MAX_STEM {
        let cut: String = stem.chars().take(MAX_STEM).collect();
        // End at a word where there is one, rather than mid-word.
        stem = match cut.rfind(' ') {
            Some(at) if at > MAX_STEM / 2 => cut[..at].to_owned(),
            _ => cut,
        };
        stem = stem.trim_end_matches([' ', '.', ',', ';', '-']).to_owned();
    }
    if stem.is_empty() {
        "Article".clone_into(&mut stem);
    }
    format!("{stem}.epub")
}

/// The current time as EPUB 3 wants its modified date.
#[must_use]
pub fn timestamp(seconds_since_epoch: u64) -> String {
    let days = seconds_since_epoch / 86_400;
    let rest = seconds_since_epoch % 86_400;
    let (year, month, day) = civil_date(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The calendar date `days` after 1970-01-01, by the proleptic Gregorian
/// calendar.
fn civil_date(days: u64) -> (u64, u64, u64) {
    // Shifted so that the year starts in March and the leap day falls last.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_english_date_is_written_out_and_another_language_keeps_the_numbers() {
        assert_eq!(spoken_date("2026-11-05", "en-US"), "5 November 2026");
        assert_eq!(spoken_date("2026-01-30", ""), "30 January 2026");
        assert_eq!(spoken_date("2026-11-05", "de"), "2026-11-05");
    }

    #[test]
    fn the_file_name_is_the_title_without_characters_a_card_refuses() {
        assert_eq!(file_name("Tides: a/b \"study\"?"), "Tides a b study.epub");
        assert_eq!(file_name("  ...  "), "Article.epub");
        assert_eq!(file_name(""), "Article.epub");
    }

    #[test]
    fn a_long_title_is_cut_at_a_word() {
        let name = file_name(
            "New Chip-Based \u{2018}Tape Measure\u{2019} for Light Could Revolutionize Mobile Timekeeping and Enhance National Security",
        );
        assert_eq!(
            name,
            "New Chip-Based \u{2018}Tape Measure\u{2019} for Light Could Revolutionize Mobile Timekeeping.epub"
        );
    }

    #[test]
    fn the_modified_time_is_a_calendar_date_in_utc() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(timestamp(1_794_139_200), "2026-11-08T12:00:00Z");
    }

    #[test]
    fn the_title_page_carries_the_site_byline_date_and_source() {
        let page = title_page(&TitlePage {
            title: "Tides & Moons",
            site: Some("Example News"),
            byline: Some("Jane Doe"),
            date: Some("5 November 2026"),
            source: "https://example.org/a?b=1&c=2",
            lead: None,
        });
        assert!(page.contains("<h1>Tides &amp; Moons</h1>"));
        assert!(page.contains("<p class=\"site\">Example News</p>"));
        assert!(page.contains("<p class=\"byline\">Jane Doe</p>"));
        assert!(page.contains("<p class=\"date\">5 November 2026</p>"));
        assert!(page.contains("href=\"https://example.org/a?b=1&amp;c=2\""));
    }
}
