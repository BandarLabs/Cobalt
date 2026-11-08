//! Finding the article in a page, and what the page says about it.

use dom_query::{Document, NodeRef};
use dom_smoothie::{Config, Readability, ReadabilityError};

use crate::address;
use crate::Refusal;

/// The most elements a page may have before extraction is refused. A long
/// article with comments is some tens of thousands; past this the page is
/// an application, not something to read.
const MAX_ELEMENTS: usize = 200_000;

/// An article found in a page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Article {
    pub title: String,
    pub byline: Option<String>,
    pub site: Option<String>,
    /// `YYYY-MM-DD`, when the page gave a date that could be read.
    pub published: Option<String>,
    pub language: Option<String>,
    /// The picture the page offers to link previews, already resolved.
    pub lead_image: Option<String>,
    /// The article as HTML, still in the page's own markup.
    pub content: String,
    /// The figure that heads the article, as HTML, when extraction left it
    /// behind.
    pub lead_figure: Option<String>,
}

/// Marks the figure found by [`find_lead_figure`], so that whether it
/// survived extraction can be seen afterwards.
const LEAD_MARK: &str = "data-cobalt-lead";

/// Finds the figure set above the story inside the page's article element.
///
/// News and magazine pages put the main photograph in the article's header,
/// next to the headline, where the extractor's scoring rarely reaches: it
/// picks the block of paragraphs and keeps what scores alongside it. Browsers
/// show that photograph when the page puts it in the paragraphs' block and
/// lose it otherwise. A book is better with it, so it is found here, before
/// extraction, and put back at the top of the story if extraction dropped it.
fn find_lead_figure(document: &Document) -> Option<String> {
    let container = document
        .select("article")
        .nodes()
        .first()
        .copied()
        .or_else(|| document.select("main").nodes().first().copied())?;
    // Collected first: marking the figure while the walk holds the tree
    // would borrow it twice.
    for node in container.descendants() {
        if !node.is_element() {
            continue;
        }
        match node.node_name().as_deref() {
            Some("p") if letters(&node) >= 100 => return None,
            Some("figure") => {
                let shows_picture = node
                    .descendants_it()
                    .any(|inner| inner.node_name().as_deref() == Some("img"));
                let in_furniture = node
                    .ancestors_it(None)
                    .take_while(|ancestor| ancestor.id != container.id)
                    .chain(std::iter::once(node))
                    .any(|element| crate::body::names_furniture(&element));
                if shows_picture && !in_furniture {
                    node.set_attr(LEAD_MARK, "1");
                    return Some(node.html().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn cleaned(value: Option<String>) -> Option<String> {
    let value = value?;
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!value.is_empty()).then_some(value)
}

/// The calendar date at the start of a timestamp, if it is one.
///
/// Pages write `2024-05-01`, `2024-05-01T09:30:00Z` and
/// `2024-05-01T09:30+02:00`. Anything that does not start with a real
/// date is left out rather than guessed at.
#[must_use]
pub fn date_of(value: &str) -> Option<String> {
    let value = value.trim();
    let date = value.get(..10)?;
    let bytes = date.as_bytes();
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && bytes[4] == b'-' && digits(5..7) && bytes[7] == b'-' && digits(8..10)) {
        return None;
    }
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    ((1..=12).contains(&month) && (1..=31).contains(&day)).then(|| date.to_owned())
}

/// The title without the site's name tacked onto either end of it, as a
/// page's `<title>` usually has it. The title page names the site already.
#[must_use]
pub fn without_site(title: &str, site: Option<&str>) -> String {
    let Some(site) = site.map(str::trim).filter(|site| !site.is_empty()) else {
        return title.to_owned();
    };
    // Compared lowercased, but cut where the separator sits in the title as
    // written: lowercasing can change how many bytes a letter takes, so an
    // offset found in the lowercased title would cut this one mid-letter.
    let same = |part: &str| part.trim().to_lowercase() == site.to_lowercase();
    for separator in [
        " | ",
        " - ",
        " \u{2013} ",
        " \u{2014} ",
        " \u{b7} ",
        " :: ",
        " / ",
    ] {
        if let Some((headline, named)) = title.rsplit_once(separator) {
            if same(named) && !headline.trim().is_empty() {
                return headline.trim().to_owned();
            }
        }
        if let Some((named, headline)) = title.split_once(separator) {
            if same(named) && !headline.trim().is_empty() {
                return headline.trim().to_owned();
            }
        }
    }
    title.to_owned()
}

/// A language tag, if the page's looks like one.
fn language_of(value: Option<String>) -> Option<String> {
    let value = value?;
    let value = value.trim().replace('_', "-");
    let valid = !value.is_empty()
        && value.len() <= 35
        && value.split('-').all(|part| {
            !part.is_empty() && part.len() <= 8 && part.bytes().all(|b| b.is_ascii_alphanumeric())
        });
    valid.then_some(value)
}

/// Bylines taken from metadata are sometimes a profile address.
fn byline_of(value: Option<String>) -> Option<String> {
    let value = cleaned(value)?;
    if address::is_web_address(&value) || value.len() > 200 {
        return None;
    }
    Some(value)
}

fn letters(node: &NodeRef<'_>) -> usize {
    node.text().chars().filter(|c| !c.is_whitespace()).count()
}

/// Lifts headings out of the small wrappers pages put them in.
///
/// Wikis and documentation sites set each heading in a `div` beside an
/// "edit" or permalink link. To the extractor that wrapper is a short block
/// that is mostly link, which is what navigation looks like, so it removed
/// the wrapper and every section heading of the article went with it. With
/// the wrapper gone the heading is judged as a heading.
fn lift_headings(document: &Document) {
    for heading in document.select("h1, h2, h3, h4, h5, h6").nodes() {
        let Some(wrapper) = heading.parent() else {
            continue;
        };
        if !matches!(wrapper.node_name().as_deref(), Some("div" | "span")) {
            continue;
        }
        let blocks = wrapper
            .element_children()
            .iter()
            .filter(|child| {
                child.is("h1, h2, h3, h4, h5, h6, p, img, picture, figure, ul, ol, table, div")
            })
            .count();
        // Only the heading itself, and at most a word or two beside it.
        if blocks != 1 || letters(&wrapper) > letters(heading) + 16 {
            continue;
        }
        wrapper.replace_with(heading);
    }
}

/// Finds the article in `html`, which came from `page`.
///
/// # Errors
///
/// [`Refusal::NotAnArticle`] when no block of readable text is found, and
/// [`Refusal::TooComplex`] for a page past [`MAX_ELEMENTS`].
pub fn extract(html: &str, page: &str) -> Result<Article, Refusal> {
    let config = Config {
        max_elements_to_parse: MAX_ELEMENTS,
        // Kept so that the furniture a page sets inside the story can still
        // be recognised by name afterwards; nothing of them is written out.
        keep_classes: true,
        ..Config::default()
    };
    let document = Document::from(html);
    lift_headings(&document);
    let lead_figure = find_lead_figure(&document);
    let mut readability = Readability::with_document(document, Some(page), Some(config))
        .map_err(|error| refusal(&error))?;
    let article = readability.parse().map_err(|error| refusal(&error))?;
    if article.text_content.trim().is_empty() {
        return Err(Refusal::NotAnArticle);
    }
    let lead_image = article
        .image
        .as_deref()
        .and_then(|image| address::resolve(page, image));
    let site = cleaned(article.site_name);
    Ok(Article {
        title: without_site(
            &cleaned(Some(article.title)).unwrap_or_default(),
            site.as_deref(),
        ),
        byline: byline_of(article.byline),
        site,
        published: article.published_time.as_deref().and_then(date_of),
        language: language_of(article.lang),
        lead_image,
        lead_figure: lead_figure.filter(|_| !article.content.contains(LEAD_MARK)),
        content: article.content.to_string(),
    })
}

fn refusal(error: &ReadabilityError) -> Refusal {
    match error {
        ReadabilityError::TooManyElements(found, limit) => Refusal::TooComplex {
            elements: *found,
            limit: *limit,
        },
        ReadabilityError::BadDocumentURL => Refusal::BadAddress(String::new()),
        ReadabilityError::GrabFailed => Refusal::NotAnArticle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_read_from_the_start_of_a_timestamp() {
        assert_eq!(
            date_of("2026-11-05T12:56:21Z").as_deref(),
            Some("2026-11-05")
        );
        assert_eq!(
            date_of("2026-10-16T08:00-04:00").as_deref(),
            Some("2026-10-16")
        );
        assert_eq!(date_of(" 2024-02-29 ").as_deref(), Some("2024-02-29"));
    }

    #[test]
    fn a_date_that_is_not_one_is_left_out() {
        assert_eq!(date_of("Nov 5, 2026"), None);
        assert_eq!(date_of("2026-13-01"), None);
        assert_eq!(date_of("2026"), None);
        assert_eq!(date_of(""), None);
    }

    #[test]
    fn the_site_name_is_taken_off_either_end_of_the_title() {
        let site = Some("NASA");
        assert_eq!(
            without_site("NASA Awards Moon Base Projects - NASA", site),
            "NASA Awards Moon Base Projects"
        );
        assert_eq!(
            without_site("Gazette | Harbour wall", Some("Gazette")),
            "Harbour wall"
        );
        assert_eq!(without_site("NASA", site), "NASA");
        assert_eq!(without_site("Moon - Base", None), "Moon - Base");
        // Letters whose lowercase takes a different number of bytes.
        assert_eq!(without_site("\u{e9} - \u{130}", Some("\u{130}")), "\u{e9}");
        assert_eq!(without_site("\u{130} - \u{e9}", Some("\u{130}")), "\u{e9}");
        assert_eq!(
            without_site("\u{130}stanbul | NASA", Some("nasa")),
            "\u{130}stanbul"
        );
    }

    #[test]
    fn a_language_tag_is_kept_only_when_it_looks_like_one() {
        assert_eq!(language_of(Some("en_US".into())).as_deref(), Some("en-US"));
        assert_eq!(language_of(Some("<script>".into())), None);
        assert_eq!(language_of(Some(String::new())), None);
    }

    #[test]
    fn a_byline_that_is_an_address_is_not_a_name() {
        assert_eq!(byline_of(Some("https://example.org/me".into())), None);
        assert_eq!(
            byline_of(Some("  Jane   Doe ".into())).as_deref(),
            Some("Jane Doe")
        );
    }

    #[test]
    fn a_heading_set_beside_an_edit_link_is_kept_as_a_heading() {
        let paragraph = "<p>The tide comes in twice a day, pulled by the moon and, to a lesser \
                         degree, by the sun, and the harbour fills and empties with it.</p>";
        let html = format!(
            "<html><body><article><h1>Tides</h1>{paragraph}{paragraph}\
             <div class=\"section-heading\"><h2>Spring tides</h2><span>[<a href=\"/edit\">edit</a>]</span></div>\
             {paragraph}{paragraph}{paragraph}</article></body></html>"
        );
        let article = extract(&html, "https://example.org/tides").expect("an article");
        assert!(
            article.content.contains("Spring tides"),
            "{}",
            article.content
        );
    }

    #[test]
    fn a_page_with_nothing_to_read_is_refused() {
        let html =
            "<html><head><title>Login</title></head><body><form><input></form></body></html>";
        assert_eq!(
            extract(html, "https://example.org/login"),
            Err(Refusal::NotAnArticle)
        );
    }
}
