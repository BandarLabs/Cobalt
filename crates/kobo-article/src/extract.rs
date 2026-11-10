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
    /// The address the page says it lives at, when that is on the same site
    /// as the address it was read from.
    pub canonical: Option<String>,
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

/// What a page's `<title>` puts between the headline and the site's name.
const SEPARATORS: [&str; 7] = [
    " | ",
    " - ",
    " \u{2013} ",
    " \u{2014} ",
    " \u{b7} ",
    " :: ",
    " / ",
];

/// What a page says about itself outside the story, read before extraction
/// takes the page apart.
#[derive(Default)]
struct Labels {
    /// The name the page gives its site for link previews.
    site: Option<String>,
    /// The page's `<title>`, site name and all.
    title: Option<String>,
    /// Authors the page's structured data calls organisations.
    organisations: Vec<String>,
    /// The page's canonical link, as written.
    canonical: Option<String>,
}

fn labels(document: &Document) -> Labels {
    let site = document
        .select(r#"meta[property="og:site_name"]"#)
        .attr("content")
        .map(|value| value.to_string());
    let title = document
        .select("title")
        .nodes()
        .first()
        .map(|node| node.text().to_string());
    let mut organisations = Vec::new();
    for script in document
        .select(r#"script[type="application/ld+json"]"#)
        .nodes()
    {
        if let Ok(value) = kobo_json::parse(&script.text()) {
            organisations_in(&value, &mut organisations, 0);
        }
    }
    let canonical = document
        .select(r#"link[rel="canonical"]"#)
        .attr("href")
        .map(|value| value.trim().to_owned());
    Labels {
        site: cleaned(site),
        title: cleaned(title),
        organisations,
        canonical,
    }
}

/// The address a page says it lives at.
///
/// A wiki serves the same article at every name that redirects to it, each
/// under its own address, and one clipped under three names arrived as three
/// copies. The canonical link names the one page they all are. It is taken
/// only on the site the page was read from, or one of its own subdomains, so
/// a page cannot pass itself off as another site's.
fn canonical_of(written: Option<&str>, page: &str) -> Option<String> {
    let canonical = address::resolve(page, written?)?;
    if !address::is_web_address(&canonical) {
        return None;
    }
    let (read, named) = (address::host_of(page)?, address::host_of(&canonical)?);
    let within = |inner: &str, outer: &str| inner == outer || inner.ends_with(&format!(".{outer}"));
    (within(&read, &named) || within(&named, &read)).then_some(canonical)
}

/// Collects the names of authors typed as organisations, from a block of
/// structured data, a list of them, or a graph of several.
fn organisations_in(value: &kobo_json::Value, names: &mut Vec<String>, depth: u8) {
    if depth > 4 || names.len() > 16 {
        return;
    }
    if let Some(items) = value.as_array() {
        for item in items.iter().take(32) {
            organisations_in(item, names, depth + 1);
        }
        return;
    }
    if let Some(graph) = value.get("@graph") {
        organisations_in(graph, names, depth + 1);
    }
    let Some(author) = value.get("author") else {
        return;
    };
    let authors = author
        .as_array()
        .map_or_else(|| vec![author], |all| all.iter().collect());
    for author in authors.into_iter().take(16) {
        let organisation =
            author.get("@type").and_then(kobo_json::Value::as_str) == Some("Organization");
        if let Some(name) = author.get("name").and_then(kobo_json::Value::as_str) {
            if organisation {
                names.push(name.trim().to_owned());
            }
        }
    }
}

/// The name to show for the site.
///
/// Structured data names the company that publishes a page, which is not
/// always what its readers call it: a wiki's page said "Wikimedia
/// Foundation, Inc." where a reader looks for "Wikipedia". The name a page
/// gives link previews comes first. Then the name its `<title>` ends with,
/// but only when the page's own address carries that name, so that a
/// headline with a dash in it is not taken for a site. Then whatever the
/// extractor found.
fn site_of(labels: &Labels, page: &str, found: Option<String>) -> Option<String> {
    if let Some(site) = &labels.site {
        return Some(site.clone());
    }
    let host: String = address::host_of(page)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let titled = labels.title.as_deref().and_then(|title| {
        SEPARATORS
            .iter()
            .filter_map(|separator| title.rsplit_once(separator))
            .map(|(_, named)| named.trim())
            .find(|named| {
                let letters: String = named
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect();
                letters.len() >= 3 && host.contains(&letters)
            })
    });
    titled.map(str::to_owned).or_else(|| cleaned(found))
}

/// The date to show: when the page was published, unless it has been
/// rewritten since.
///
/// A wiki page gives the day it was first written as its publication date,
/// and a page started in 2001 and edited last week is not a 2001 page. News
/// is corrected within days, so an update more than a year after publication
/// marks a page that is kept up rather than one that was published once.
fn date_shown(published: Option<&str>, modified: Option<&str>) -> Option<String> {
    let published = published.and_then(date_of);
    let modified = modified.and_then(date_of);
    let months = |date: &str| -> Option<u32> {
        Some(date[..4].parse::<u32>().ok()? * 12 + date[5..7].parse::<u32>().ok()?)
    };
    match (published, modified) {
        (Some(published), Some(modified))
            if months(&modified)
                .zip(months(&published))
                .is_some_and(|(later, first)| later > first + 12) =>
        {
            Some(modified)
        }
        (Some(published), _) => Some(published),
        (None, modified) => modified,
    }
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
    for separator in SEPARATORS {
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
    let labels = labels(&document);
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
    let site = site_of(&labels, page, article.site_name);
    // An organisation credited as the author is the publisher again, or a
    // wiki's "Contributors to" line: neither is a byline.
    let byline = byline_of(article.byline).filter(|byline| {
        !labels
            .organisations
            .iter()
            .any(|name| name.eq_ignore_ascii_case(byline))
    });
    Ok(Article {
        title: without_site(
            &cleaned(Some(article.title)).unwrap_or_default(),
            site.as_deref(),
        ),
        byline,
        site,
        published: date_shown(
            article.published_time.as_deref(),
            article.modified_time.as_deref(),
        ),
        language: language_of(article.lang),
        lead_image,
        lead_figure: lead_figure.filter(|_| !article.content.contains(LEAD_MARK)),
        content: article.content.to_string(),
        canonical: canonical_of(labels.canonical.as_deref(), page),
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

    /// The head of a wiki page as it is served: no preview name, the
    /// publisher's company in the structured data, and the date the page was
    /// started.
    fn wiki_page() -> String {
        let paragraph = "<p>Electronic paper is a display that reflects light like paper \
                         rather than giving it out, and holds an image without power.</p>";
        format!(
            r#"<html><head><title>Electronic paper - Wikipedia</title>
            <script type="application/ld+json">{{"@context":"https:\/\/schema.org","@type":"Article",
            "author":{{"@type":"Organization","name":"Contributors to Wikimedia projects"}},
            "publisher":{{"@type":"Organization","name":"Wikimedia Foundation, Inc."}},
            "datePublished":"2001-02-25T10:02:09Z","dateModified":"2026-11-01T17:32:41Z"}}</script>
            </head><body><main><h1>Electronic paper</h1>{paragraph}{paragraph}{paragraph}{paragraph}</main></body></html>"#
        )
    }

    #[test]
    fn a_wiki_page_is_named_for_the_wiki_and_dated_by_its_last_edit() {
        let article = extract(
            &wiki_page(),
            "https://en.wikipedia.org/wiki/Electronic_paper",
        )
        .expect("an article");
        assert_eq!(article.site.as_deref(), Some("Wikipedia"));
        assert_eq!(article.title, "Electronic paper");
        assert_eq!(article.byline, None);
        assert_eq!(article.published.as_deref(), Some("2026-11-01"));
    }

    #[test]
    fn a_page_is_known_by_its_canonical_address_only_on_its_own_site() {
        let page = "https://en.wikipedia.org/wiki/Electrophoretic_display";
        assert_eq!(
            canonical_of(Some("https://en.wikipedia.org/wiki/Electronic_paper"), page).as_deref(),
            Some("https://en.wikipedia.org/wiki/Electronic_paper")
        );
        assert_eq!(
            canonical_of(Some("/wiki/Electronic_paper"), page).as_deref(),
            Some("https://en.wikipedia.org/wiki/Electronic_paper")
        );
        assert_eq!(
            canonical_of(Some("https://example.org/wiki/Electronic_paper"), page),
            None
        );
        assert_eq!(canonical_of(Some("javascript:void(0)"), page), None);
        assert_eq!(canonical_of(None, page), None);
    }

    #[test]
    fn a_title_ending_in_words_the_address_does_not_carry_is_not_a_site() {
        let labels = Labels {
            title: Some("Rain - and then sun".into()),
            ..Labels::default()
        };
        assert_eq!(
            site_of(
                &labels,
                "https://example.org/weather",
                Some("Example Press".into())
            )
            .as_deref(),
            Some("Example Press")
        );
        let labels = Labels {
            site: Some("The Gazette".into()),
            title: Some("Harbour wall | Gazette".into()),
            ..Labels::default()
        };
        assert_eq!(
            site_of(&labels, "https://gazette.example/a", None).as_deref(),
            Some("The Gazette")
        );
    }

    #[test]
    fn a_correction_days_later_keeps_the_publication_date() {
        assert_eq!(
            date_shown(
                Some("2026-10-16T08:00-04:00"),
                Some("2026-10-27T09:22-04:00")
            )
            .as_deref(),
            Some("2026-10-16")
        );
        assert_eq!(
            date_shown(Some("2025-01-10"), Some("2026-01-31")).as_deref(),
            Some("2025-01-10")
        );
        assert_eq!(
            date_shown(Some("2025-01-10"), Some("2026-02-01")).as_deref(),
            Some("2026-02-01")
        );
        assert_eq!(
            date_shown(None, Some("2026-02-01")).as_deref(),
            Some("2026-02-01")
        );
        assert_eq!(date_shown(None, None), None);
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
