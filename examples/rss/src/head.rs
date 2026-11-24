//! What an article says about itself before its first word.
//!
//! A feed's article body is the body and nothing else: the title, the writer
//! and the day live beside it in the feed, not in it. Opened as it came, an
//! article began mid-flow, on its first paragraph or on a bare "I.", and the
//! only sign of which article it was had been left behind on the list. A
//! printed page names its article at the top, and so does this one.
//!
//! The head goes into the markup itself, so the copy kept for offline reading
//! carries it too, and so a page fetched from behind a teaser gets the same
//! head as an article the feed carried whole.

use crate::feed;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Words an article's last paragraph may be when it is only a way to the
/// rest of it. With no browser to follow it to, it is a dead end on the page,
/// and the note under the article says where the rest is instead.
const WAYS_ON: [&str; 6] = [
    "read more",
    "continue reading",
    "keep reading",
    "read the full article",
    "read full article",
    "read the rest",
];

/// The day an article was published, as its head prints it: "19 Nov 2026".
///
/// Always the whole date and never "Today", because a kept copy carries its
/// head for as long as it is kept, and a day said against the moment it was
/// saved is wrong by the next morning. Read on the reader's own calendar when
/// the feed gave a time, and as the feed wrote it otherwise.
#[must_use]
pub fn printed_date(item: &feed::Item, now: Option<kobo_sdk::clock::Snapshot>) -> String {
    let offset = now.map_or(0, |now| now.utc_offset_minutes);
    item.moment()
        .and_then(|moment| {
            kobo_sdk::clock::Snapshot {
                unix_millis: moment,
                monotonic_millis: 0,
                utc_offset_minutes: offset,
            }
            .date()
        })
        .and_then(|date| {
            let month = MONTHS.get(usize::from(date.month).checked_sub(1)?)?;
            Some(format!("{} {month} {}", date.day, date.year))
        })
        .unwrap_or_else(|| item.identity_date().trim_start_matches('0').to_owned())
}

/// The head of an article: its title, then who wrote it and when.
///
/// The byline is set as a caption, the small muted line a picture is
/// credited in, so it reads as a note about the article and not as its
/// first sentence. Empty when the feed gave no title, since a byline alone
/// at the top of a page names nothing.
#[must_use]
pub fn head(item: &feed::Item, now: Option<kobo_sdk::clock::Snapshot>) -> String {
    let title = item.title.trim();
    if title.is_empty() {
        return String::new();
    }
    let date = printed_date(item, now);
    let byline = match (item.author.trim(), date.as_str()) {
        ("", "") => String::new(),
        ("", date) => date.to_owned(),
        (author, "") => author.to_owned(),
        (author, date) => format!("{author} \u{00b7} {date}"),
    };
    let mut out = format!("<h1>{}</h1>\n", feed::escape_html(title));
    if !byline.is_empty() {
        out.push_str(&format!(
            "<figcaption>{}</figcaption>\n",
            feed::escape_html(&byline)
        ));
    }
    out
}

/// `markup` with `head` in front of it, and without a last paragraph that
/// only pointed to the rest.
///
/// A page fetched from behind a teaser often opens with the article's own
/// title already, and two titles one under the other look like a fault, so
/// the head is left off when the markup's first line is the title.
#[must_use]
pub fn headed(title: &str, head: &str, markup: &str) -> String {
    let markup = without_way_on(markup);
    let opening = kobo_html::to_text(markup.get(..markup.len().min(2048)).unwrap_or(markup));
    let first = opening.lines().map(str::trim).find(|line| !line.is_empty());
    if head.is_empty() || first.is_some_and(|line| same_words(line, title)) {
        return markup.to_owned();
    }
    format!("{head}{markup}")
}

/// `markup` without its last paragraph when that paragraph is only a way to
/// the rest of the article.
fn without_way_on(markup: &str) -> &str {
    let Some(start) = markup.rfind("<p") else {
        return markup;
    };
    let last = kobo_html::to_text(&markup[start..]);
    let words = last
        .trim()
        .trim_end_matches(['.', '\u{2026}', '\u{2192}', '\u{00bb}', '\u{203a}', ' '])
        .to_lowercase();
    if WAYS_ON.contains(&words.as_str()) {
        &markup[..start]
    } else {
        markup
    }
}

/// Whether two lines say the same words, whatever their case and spacing.
fn same_words(one: &str, other: &str) -> bool {
    let words =
        |text: &str| -> Vec<String> { text.split_whitespace().map(str::to_lowercase).collect() };
    !other.trim().is_empty() && words(one) == words(other)
}

#[cfg(test)]
mod tests {
    use super::{head, headed, printed_date};
    use crate::feed;

    fn item(title: &str, author: &str, stamp: &str) -> feed::Item {
        feed::Item {
            title: title.to_owned(),
            author: author.to_owned(),
            stamp: stamp.to_owned(),
            ..feed::Item::default()
        }
    }

    #[test]
    fn an_article_opens_with_its_title_then_its_writer_and_its_whole_date() {
        let item = item(
            "The AI Forecasting Agenda",
            "Scott Alexander",
            "Wed, 19 Nov 2026 07:25:00 GMT",
        );
        let head = head(&item, None);
        assert_eq!(
            head,
            "<h1>The AI Forecasting Agenda</h1>\n<figcaption>Scott Alexander \u{00b7} 19 Nov 2026</figcaption>\n"
        );
        let opened = headed(
            &item.title,
            &head,
            "<p>AI forecasters now equal top humans.</p>",
        );
        assert!(
            opened.starts_with("<h1>The AI Forecasting Agenda</h1>"),
            "{opened}"
        );
        assert!(
            opened.ends_with("<p>AI forecasters now equal top humans.</p>"),
            "{opened}"
        );
    }

    #[test]
    fn the_printed_date_is_on_the_readers_own_calendar() {
        let item = item("Late", "", "2026-11-19T23:30:00Z");
        let east = kobo_sdk::clock::Snapshot {
            unix_millis: 0,
            monotonic_millis: 0,
            utc_offset_minutes: 330,
        };
        assert_eq!(printed_date(&item, None), "19 Nov 2026");
        assert_eq!(printed_date(&item, Some(east)), "20 Nov 2026");
    }

    #[test]
    fn a_title_is_escaped_and_a_missing_writer_or_date_leaves_no_separator() {
        assert_eq!(
            head(&item("Fish & <Chips>", "", ""), None),
            "<h1>Fish &amp; &lt;Chips&gt;</h1>\n"
        );
        assert_eq!(
            head(&item("Notes", "", "Wed, 19 Nov 2026 07:25:00 GMT"), None),
            "<h1>Notes</h1>\n<figcaption>19 Nov 2026</figcaption>\n"
        );
        assert_eq!(head(&item("", "Somebody", "2026-11-19"), None), "");
    }

    #[test]
    fn a_page_that_already_opens_with_the_title_is_not_given_a_second() {
        let item = item(
            "Find the levers, and pull",
            "Matthew Yglesias",
            "2026-11-19",
        );
        let head = head(&item, None);
        let page = "<h1>Find the  levers, and Pull</h1><p>Airports are noisy.</p>";
        assert_eq!(headed(&item.title, &head, page), page);
    }

    #[test]
    fn a_last_paragraph_that_only_points_to_the_rest_is_left_out() {
        let item = item("Permitting", "", "");
        let head = head(&item, None);
        let teaser = "<p>The Senate passed the act. Notabl\u{2026}</p>\n<p><a href=\"https://example.com/p/permitting\">Read more</a></p>";
        let opened = headed(&item.title, &head, teaser);
        assert!(!opened.contains("Read more"), "{opened}");
        assert!(opened.contains("Notabl\u{2026}"), "{opened}");
        let ordinary = "<p>Read more books.</p>";
        assert!(headed(&item.title, &head, ordinary).contains("Read more books."));
    }
}
