//! Reading a feed, whichever of the three dialects it happens to be written in.
//!
//! A subscription list is not a list of things you chose the format of. RSS
//! 2.0, Atom and JSON Feed are all in wide use, publishing systems disagree
//! about which of them they emit, and a reader that supports one of the three
//! is a reader that fails on two thirds of what people paste into it. So all
//! three are read here, into one shape.
//!
//! # Why this is not a general XML parser
//!
//! Because a general XML parser would be a liability. Feeds are documents
//! written by strangers and served by machines nobody here controls, and the
//! interesting inputs are the malformed ones: a `<` with no `>`, a character
//! section that never closes, an element nested four hundred deep, a namespace
//! prefix chosen to collide with a name this module reads. A parser that
//! models XML faithfully has to have an answer for all of it.
//!
//! The scanning itself — walking a flat run of elements and text, treating
//! everything else as text to be skipped, stopping rather than guessing at
//! the first malformed shape — now lives in [`kobo_xml`], because an OPDS
//! catalog reader needs exactly that same defensive walk over the same kind
//! of adversarial input. What stays here is the part that is genuinely about
//! feeds: which dozen or so element names mean anything, in [`field`].
//!
//! # Why namespace prefixes are matched whole
//!
//! The obvious shortcut is to strip the prefix and match the local name, so
//! that `content:encoded` matches `content`. It is also a bug: `media:content`
//! is a common extension whose payload is an image, and under that rule a
//! photograph becomes the article body. Names are matched entire, and the only
//! two prefixed names that mean anything are named explicitly.

use kobo_html::to_text;
use kobo_net::resolve_https_url;
use kobo_xml::{scan, Event};

/// The most items one feed contributes.
///
/// A feed's own idea of how much history to serve runs from ten items to
/// several hundred, and the difference is not useful on a device that shows
/// six rows at a time. This is deep enough that nobody reaches the end in a
/// sitting and shallow enough that a publisher who serves a year of archive
/// does not become the whole of this application's memory.
pub const MAX_ITEMS: usize = 50;

/// The longest title kept, in characters.
///
/// Titles are drawn clamped to two lines, so anything past this cannot be
/// seen. Cutting it here rather than at the layout means a stored subscription
/// does not carry a kilobyte of somebody's essay-as-headline.
const MAX_TITLE: usize = 300;

/// One entry in a feed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Item {
    pub title: String,
    /// A feed-provided GUID or id, falling back to the canonical article URL.
    ///
    /// This is deliberately an identifier rather than a display field: it is
    /// what the reader uses to retain read and starred state across refreshes.
    pub id: String,
    /// Where the full article lives, if the feed says.
    pub link: String,
    /// The publication date, as the feed wrote it.
    pub stamp: String,
    pub author: String,
    /// The body, already converted from whatever markup it arrived in.
    pub body: String,
}

impl Item {
    /// True when there is nothing here worth a row.
    fn is_empty(&self) -> bool {
        self.title.trim().is_empty() && self.body.trim().is_empty() && self.link.trim().is_empty()
    }

    /// The date in a form that fits the end of a row, or nothing.
    ///
    /// Deliberately not a parse into a calendar type. Feeds carry RFC 822 in
    /// RSS and RFC 3339 in Atom, both with a long tail of near-misses, and the
    /// only use here is six characters at the end of a row. Recognising the
    /// two shapes and giving up on anything else is honest about that.
    #[must_use]
    pub fn short_date(&self) -> String {
        let stamp = self.stamp.trim();
        if let Some(head) = stamp.get(..10) {
            let bytes = head.as_bytes();
            if bytes[4] == b'-' && bytes[7] == b'-' {
                if let (Some(month), Some(day)) = (head.get(5..7), head.get(8..10)) {
                    let month = month_name(month);
                    if !month.is_empty() {
                        return format!("{day} {month}");
                    }
                }
            }
        }
        let mut fields = stamp.split_whitespace();
        let first = fields.next().unwrap_or_default();
        let (day, month) = if first.ends_with(',') {
            (fields.next(), fields.next())
        } else {
            (Some(first), fields.next())
        };
        match (day, month) {
            (Some(day), Some(month))
                if !day.is_empty()
                    && day.len() <= 2
                    && day.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                format!("{day} {month}")
            }
            _ => String::new(),
        }
    }
}

/// A numeric month as the three letters a row shows.
fn month_name(month: &str) -> &'static str {
    match month {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => "",
    }
}

/// A feed, whatever it was written in.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Feed {
    pub title: String,
    /// The site the feed belongs to, as opposed to the feed's own address.
    pub site: String,
    pub items: Vec<Item>,
}

/// Reads a feed from whatever came back over the wire.
///
/// Returns nothing when the bytes are not a feed at all, which is a different
/// answer from a feed with no items, a publisher between posts is not an
/// error, and the screen says so differently.
#[must_use]
#[cfg(test)]
pub fn parse(bytes: &[u8]) -> Option<Feed> {
    parse_at(bytes, "")
}

/// Reads a feed using the URL requested by the application as the base.
///
/// `TaskOutcome` intentionally exposes only body bytes, not a redirect's
/// final URL. The requested subscription URL is therefore the only honest
/// base available to the parser; inventing a canonical redirect URL would
/// resolve relative links against an address we never received.
#[must_use]
pub fn parse_at(bytes: &[u8], requested_url: &str) -> Option<Feed> {
    let text = String::from_utf8_lossy(bytes);
    let body = text.trim_start_matches(['\u{feff}', ' ', '\n', '\r', '\t']);
    let mut feed = if body.starts_with('{') {
        json_feed(body)?
    } else {
        xml_feed(body)
    };
    if feed.items.is_empty() && feed.title.is_empty() {
        return None;
    }
    resolve_addresses(&mut feed, requested_url);
    Some(feed)
}

/// Resolves every URL a feed supplied and refuses anything the runtime cannot
/// fetch. This shares the transport's RFC 3986 resolver and HTTPS boundary,
/// rather than allowing each feed dialect to grow a subtly different parser.
fn resolve_addresses(feed: &mut Feed, requested_url: &str) {
    if !feed.site.trim().is_empty() {
        feed.site = resolve_https_url(requested_url, &feed.site).unwrap_or_default();
    }
    for item in &mut feed.items {
        if !item.link.trim().is_empty() {
            item.link = resolve_https_url(requested_url, &item.link).unwrap_or_default();
        }
        if item.id.trim().is_empty() {
            item.id.clone_from(&item.link);
        }
    }
}

/// Which of the names this module reads an element is, if any.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Item,
    Title,
    Id,
    Link,
    /// A body, with how complete a body of that name tends to be. A feed that
    /// carries both a summary and the full text keeps the full text, whichever
    /// order the two arrive in.
    Body(u8),
    Stamp,
    Author,
    Name,
    None,
}

/// Matches an element name entire, prefix included.
fn field(name: &str) -> Field {
    if name.eq_ignore_ascii_case("content:encoded") {
        return Field::Body(3);
    }
    if name.eq_ignore_ascii_case("dc:creator") {
        return Field::Author;
    }
    // Anything else carrying a prefix belongs to an extension this does not
    // read. `media:content` is the one that matters: it is an image, and
    // without this line it would become the article.
    if name.contains(':') {
        return Field::None;
    }
    if name.eq_ignore_ascii_case("item") || name.eq_ignore_ascii_case("entry") {
        return Field::Item;
    }
    if name.eq_ignore_ascii_case("title") {
        return Field::Title;
    }
    if name.eq_ignore_ascii_case("guid") || name.eq_ignore_ascii_case("id") {
        return Field::Id;
    }
    if name.eq_ignore_ascii_case("link") {
        return Field::Link;
    }
    if name.eq_ignore_ascii_case("content") || name.eq_ignore_ascii_case("description") {
        return Field::Body(2);
    }
    if name.eq_ignore_ascii_case("summary") || name.eq_ignore_ascii_case("subtitle") {
        return Field::Body(1);
    }
    if name.eq_ignore_ascii_case("pubdate")
        || name.eq_ignore_ascii_case("published")
        || name.eq_ignore_ascii_case("updated")
        || name.eq_ignore_ascii_case("date")
    {
        return Field::Stamp;
    }
    if name.eq_ignore_ascii_case("author") {
        return Field::Author;
    }
    if name.eq_ignore_ascii_case("name") {
        return Field::Name;
    }
    Field::None
}

/// Elements after which a line break belongs, when one appears inside a body.
///
/// Only reached by Atom's `type="xhtml"`, where the markup is real elements
/// rather than escaped text, and so is taken apart by the scanner before
/// [`to_text`] ever sees it. Without this a five-paragraph post arrives as one
/// paragraph.
const BREAKS_LINE: [&str; 11] = [
    "p",
    "br",
    "div",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "blockquote",
];

/// Reads one attribute out of the raw attribute text.
fn attribute<'a>(attributes: &'a str, want: &str) -> Option<&'a str> {
    let mut rest = attributes;
    while let Some(equals) = rest.find('=') {
        let name = rest[..equals].trim();
        let tail = rest[equals + 1..].trim_start();
        let quote = tail.chars().next()?;
        if quote != '"' && quote != '\'' {
            rest = &rest[equals + 1..];
            continue;
        }
        let tail = &tail[1..];
        let end = tail.find(quote)?;
        if name.eq_ignore_ascii_case(want) {
            return Some(&tail[..end]);
        }
        rest = &tail[end + 1..];
    }
    None
}

#[allow(clippy::too_many_lines)]
fn xml_feed(input: &str) -> Feed {
    let mut decoded: Vec<String> = Vec::new();
    let mut steps: Vec<Event<'_>> = Vec::new();
    // Collected first so that the decoded text outlives the walk: the scanner
    // hands back an index into that scratch rather than borrowing it.
    scan(input, &mut decoded, |event| steps.push(event));

    let mut feed = Feed::default();
    let mut stack: Vec<&str> = Vec::new();
    let mut buffer = String::new();
    let mut item = Item::default();
    let mut in_item = false;
    let mut body_rank = 0u8;

    for event in steps {
        match event {
            Event::Open { name, attributes } => {
                let field = field(name);
                match field {
                    Field::Item => {
                        in_item = true;
                        item = Item::default();
                        body_rank = 0;
                        buffer.clear();
                    }
                    // Not a name this module reads, so it is either inline
                    // markup inside a body or noise; either way the text
                    // gathered so far must survive it.
                    Field::None => {}
                    _ => buffer.clear(),
                }
                // Atom puts the address in an attribute and leaves the element
                // empty, so it has to be read here rather than at the close.
                if field == Field::Link {
                    if let Some(href) = attribute(attributes, "href") {
                        let relation = attribute(attributes, "rel").unwrap_or("alternate");
                        if relation.eq_ignore_ascii_case("alternate") {
                            if in_item {
                                if item.link.is_empty() {
                                    href.clone_into(&mut item.link);
                                }
                            } else if feed.site.is_empty() {
                                href.clone_into(&mut feed.site);
                            }
                        }
                    }
                }
                stack.push(name);
            }
            Event::Text(text) => buffer.push_str(text),
            Event::Owned(index) => {
                if let Some(text) = decoded.get(index) {
                    buffer.push_str(text);
                }
            }
            Event::Close { name } => {
                let parent = stack
                    .len()
                    .checked_sub(2)
                    .and_then(|index| stack.get(index))
                    .copied()
                    .unwrap_or_default();
                let value = buffer.trim().to_owned();
                match field(name) {
                    Field::Item => {
                        item.body = to_text(&item.body);
                        if !item.is_empty() && feed.items.len() < MAX_ITEMS {
                            feed.items.push(std::mem::take(&mut item));
                        }
                        in_item = false;
                        buffer.clear();
                    }
                    Field::Title => {
                        if !value.is_empty() {
                            if in_item {
                                item.title = clamp(&value, MAX_TITLE);
                            } else if feed.title.is_empty() {
                                feed.title = clamp(&value, MAX_TITLE);
                            }
                        }
                        buffer.clear();
                    }
                    Field::Id => {
                        if in_item && item.id.is_empty() && !value.is_empty() {
                            item.id = clamp(&value, MAX_TITLE);
                        }
                        buffer.clear();
                    }
                    Field::Link => {
                        if !value.is_empty() {
                            if in_item {
                                if item.link.is_empty() {
                                    item.link = value;
                                }
                            } else if feed.site.is_empty() {
                                feed.site = value;
                            }
                        }
                        buffer.clear();
                    }
                    Field::Body(rank) => {
                        if in_item && rank >= body_rank && !value.is_empty() {
                            body_rank = rank;
                            item.body = value;
                        }
                        buffer.clear();
                    }
                    Field::Stamp => {
                        if in_item && item.stamp.is_empty() {
                            item.stamp = value;
                        }
                        buffer.clear();
                    }
                    Field::Author => {
                        if in_item && !value.is_empty() {
                            item.author = clamp(&value, MAX_TITLE);
                        }
                        buffer.clear();
                    }
                    // Atom nests the author's name one deeper.
                    Field::Name => {
                        if in_item
                            && item.author.is_empty()
                            && !value.is_empty()
                            && parent.eq_ignore_ascii_case("author")
                        {
                            item.author = clamp(&value, MAX_TITLE);
                        }
                        buffer.clear();
                    }
                    Field::None => {
                        if BREAKS_LINE.iter().any(|tag| name.eq_ignore_ascii_case(tag)) {
                            buffer.push_str("\n\n");
                        }
                    }
                }
                stack.pop();
            }
        }
    }

    // A document truncated mid-item still described that item, so it is kept.
    item.body = to_text(&item.body);
    if !item.is_empty() && feed.items.len() < MAX_ITEMS {
        feed.items.push(item);
    }
    feed
}

fn json_feed(input: &str) -> Option<Feed> {
    let value = kobo_json::parse(input).ok()?;
    let mut feed = Feed {
        title: value
            .get("title")
            .and_then(kobo_json::Value::as_str)
            .map(|title| clamp(title, MAX_TITLE))
            .unwrap_or_default(),
        site: value
            .get("home_page_url")
            .and_then(kobo_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        items: Vec::new(),
    };
    for entry in value
        .get("items")
        .and_then(kobo_json::Value::as_array)
        .unwrap_or_default()
        .iter()
        .take(MAX_ITEMS)
    {
        let text = entry.get("content_text").and_then(kobo_json::Value::as_str);
        let html = entry.get("content_html").and_then(kobo_json::Value::as_str);
        let item = Item {
            title: entry
                .get("title")
                .and_then(kobo_json::Value::as_str)
                .map(|title| clamp(title, MAX_TITLE))
                .unwrap_or_default(),
            id: entry
                .get("id")
                .and_then(kobo_json::Value::as_str)
                .map(|id| clamp(id, MAX_TITLE))
                .unwrap_or_default(),
            link: entry
                .get("url")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            stamp: entry
                .get("date_published")
                .and_then(kobo_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            author: json_author(entry),
            // Plain text is preferred where the publisher supplied it, because
            // it is what they meant the words to be, rather than what a
            // converter made of their markup.
            body: text.map_or_else(|| to_text(html.unwrap_or_default()), to_text),
        };
        if !item.is_empty() {
            feed.items.push(item);
        }
    }
    Some(feed)
}

/// The author, from either the current spec or the one it replaced.
fn json_author(entry: &kobo_json::Value) -> String {
    entry
        .get("authors")
        .and_then(kobo_json::Value::as_array)
        .and_then(<[kobo_json::Value]>::first)
        .or_else(|| entry.get("author"))
        .and_then(|author| author.get("name"))
        .and_then(kobo_json::Value::as_str)
        .map(|name| clamp(name, MAX_TITLE))
        .unwrap_or_default()
}

/// Cuts to a character count, never inside a character.
fn clamp(text: &str, characters: usize) -> String {
    match text.char_indices().nth(characters) {
        Some((index, _)) => text[..index].trim_end().to_owned(),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{attribute, parse, parse_at, Feed, Item, MAX_ITEMS};
    use kobo_xml::decode_entities;
    use std::fmt::Write as _;

    fn read(source: &str) -> Feed {
        parse(source.as_bytes()).expect("a feed")
    }

    #[test]
    fn an_rss_channel_yields_its_items() {
        let feed = read(
            r#"<?xml version="1.0"?>
            <rss version="2.0"><channel>
              <title>Ars Technica</title>
              <link>https://arstechnica.com/</link>
              <item>
                <title>A headline</title>
                <link>https://example.com/one</link>
                <pubDate>Fri, 05 Jul 2019 16:00:30 +0000</pubDate>
                <description>&lt;p&gt;Some body text.&lt;/p&gt;</description>
              </item>
            </channel></rss>"#,
        );
        assert_eq!(feed.title, "Ars Technica");
        assert_eq!(feed.site, "https://arstechnica.com/");
        assert_eq!(feed.items.len(), 1);
        assert_eq!(feed.items[0].title, "A headline");
        assert_eq!(feed.items[0].link, "https://example.com/one");
        assert_eq!(feed.items[0].body, "Some body text.");
        assert_eq!(feed.items[0].short_date(), "05 Jul");
    }

    #[test]
    fn an_atom_feed_reads_its_addresses_out_of_attributes() {
        let feed = read(
            r#"<feed xmlns="http://www.w3.org/2005/Atom">
              <title>A journal</title>
              <link rel="self" href="https://example.com/feed.xml"/>
              <link rel="alternate" href="https://example.com/"/>
              <entry>
                <title>An entry</title>
                <link href="https://example.com/entry"/>
                <published>2019-07-05T16:00:30Z</published>
                <author><name>A Writer</name></author>
                <summary>Short.</summary>
              </entry>
            </feed>"#,
        );
        assert_eq!(feed.title, "A journal");
        assert_eq!(feed.site, "https://example.com/");
        assert_eq!(feed.items[0].link, "https://example.com/entry");
        assert_eq!(feed.items[0].author, "A Writer");
        assert_eq!(feed.items[0].short_date(), "05 Jul");
    }

    #[test]
    fn a_self_link_is_not_mistaken_for_the_site() {
        let feed = read(
            r#"<feed><title>T</title>
               <link rel="self" href="https://example.com/feed.xml"/>
               <entry><title>E</title></entry></feed>"#,
        );
        assert!(feed.site.is_empty(), "{:?}", feed.site);
    }

    #[test]
    fn a_json_feed_reads_the_same_way() {
        let feed = read(
            r#"{"version":"https://jsonfeed.org/version/1","title":"JSON Feed",
                "home_page_url":"https://jsonfeed.org/",
                "items":[{"id":"1","url":"https://jsonfeed.org/one","title":"First",
                          "date_published":"2019-07-05T16:00:30Z",
                          "authors":[{"name":"A Writer"}],
                          "content_html":"<p>Hello</p>"}]}"#,
        );
        assert_eq!(feed.title, "JSON Feed");
        assert_eq!(feed.items[0].title, "First");
        assert_eq!(feed.items[0].body, "Hello");
        assert_eq!(feed.items[0].author, "A Writer");
        assert_eq!(feed.items[0].id, "1");
    }

    #[test]
    fn rss_atom_and_json_relative_urls_share_the_requested_feed_base() {
        let rss = parse_at(
            br"<rss><channel><link>/site</link><item><guid>rss-guid</guid><link>stories/one</link><title>One</title></item></channel></rss>",
            "https://example.test/feeds/news.xml",
        )
        .expect("RSS");
        assert_eq!(rss.site, "https://example.test/site");
        assert_eq!(rss.items[0].link, "https://example.test/feeds/stories/one");
        assert_eq!(rss.items[0].id, "rss-guid");

        let atom = parse_at(
            br#"<feed><link rel="alternate" href="/site"/><entry><id>atom-id</id><link href="../two"/><title>Two</title></entry></feed>"#,
            "https://example.test/feeds/atom.xml",
        )
        .expect("Atom");
        assert_eq!(atom.site, "https://example.test/site");
        assert_eq!(atom.items[0].link, "https://example.test/two");
        assert_eq!(atom.items[0].id, "atom-id");

        let json = parse_at(
            br#"{"title":"JSON","home_page_url":"/site","items":[{"url":"three","title":"Three"}]}"#,
            "https://example.test/feeds/json.json",
        )
        .expect("JSON Feed");
        assert_eq!(json.site, "https://example.test/site");
        assert_eq!(json.items[0].link, "https://example.test/feeds/three");
        assert_eq!(json.items[0].id, "https://example.test/feeds/three");
    }

    #[test]
    fn unsafe_links_are_not_persisted_as_stable_url_fallbacks() {
        let feed = parse_at(
            br"<rss><channel><item><title>Nope</title><link>javascript:alert(1)</link></item></channel></rss>",
            "https://example.test/feed.xml",
        )
        .expect("feed");
        assert!(feed.items[0].link.is_empty());
        assert!(feed.items[0].id.is_empty());
    }

    #[test]
    fn the_full_text_wins_over_the_summary_whichever_order_they_arrive_in() {
        let first = read(
            "<rss><channel><item><title>T</title>\
             <description>Short.</description>\
             <content:encoded>The whole article.</content:encoded>\
             </item></channel></rss>",
        );
        let second = read(
            "<rss><channel><item><title>T</title>\
             <content:encoded>The whole article.</content:encoded>\
             <description>Short.</description>\
             </item></channel></rss>",
        );
        assert_eq!(first.items[0].body, "The whole article.");
        assert_eq!(second.items[0].body, "The whole article.");
    }

    #[test]
    fn a_media_extension_never_becomes_the_article() {
        let feed = read(
            "<rss><channel><item><title>T</title>\
             <media:content url=\"https://example.com/a.jpg\">an image caption</media:content>\
             <description>The real body.</description>\
             </item></channel></rss>",
        );
        assert_eq!(feed.items[0].body, "The real body.");
    }

    #[test]
    fn a_character_section_is_not_decoded_twice() {
        let feed = read(
            "<rss><channel><item><title>T</title>\
             <description><![CDATA[<p>A &amp; B</p>]]></description>\
             </item></channel></rss>",
        );
        assert_eq!(feed.items[0].body, "A & B");
    }

    #[test]
    fn escaped_markup_is_unescaped_once_and_then_stripped() {
        let feed = read(
            "<rss><channel><item><title>T</title>\
             <description>&lt;a href=\"x\"&gt;linked&lt;/a&gt; &amp;amp; more</description>\
             </item></channel></rss>",
        );
        assert_eq!(feed.items[0].body, "linked & more");
    }

    #[test]
    fn real_xhtml_content_keeps_its_paragraphs() {
        let feed = read(
            "<feed><title>T</title><entry><title>E</title>\
             <content type=\"xhtml\"><p>One.</p><p>Two.</p></content>\
             </entry></feed>",
        );
        let body = &feed.items[0].body;
        assert!(body.contains("One."), "{body:?}");
        assert!(body.contains("Two."), "{body:?}");
        assert!(body.contains('\n'), "paragraphs ran together: {body:?}");
    }

    #[test]
    fn a_truncated_document_keeps_what_it_managed_to_say() {
        let feed = read(
            "<rss><channel><title>T</title>\
             <item><title>Complete</title><description>One.</description></item>\
             <item><title>Cut off</title><description>Tw",
        );
        assert_eq!(feed.title, "T");
        assert_eq!(feed.items.len(), 2);
        assert_eq!(feed.items[1].title, "Cut off");
    }

    #[test]
    fn nothing_that_is_not_a_feed_parses_as_one() {
        assert!(parse(b"").is_none());
        assert!(parse(b"not markup at all").is_none());
        assert!(parse(b"<html><body><h1>A web page</h1></body></html>").is_none());
        assert!(parse(b"{\"unrelated\":true}").is_none());
        assert!(parse(b"{ this is not json").is_none());
    }

    #[test]
    fn every_malformed_shape_has_a_boring_outcome() {
        for source in [
            "<rss><channel><item><title>T",
            "<<<<<<",
            "<rss>&",
            "<rss>&#;",
            "<rss>&#xZZ;",
            "<![CDATA[unclosed",
            "<!-- unclosed",
            "<rss><item><description><![CDATA[<p>]]></description></item></rss>",
            "<rss><item><link href=></item></rss>",
            "<rss><item><link href='unterminated></item></rss>",
            "<>",
            "</>",
        ] {
            let _ = parse(source.as_bytes());
        }
    }

    #[test]
    fn text_that_is_not_ascii_survives_being_cut_at_a_boundary() {
        let long = "é".repeat(400);
        let feed = read(&format!(
            "<rss><channel><item><title>{long}</title></item></channel></rss>"
        ));
        assert_eq!(feed.items[0].title.chars().count(), 300);
    }

    #[test]
    fn a_document_nested_past_all_reason_stops_rather_than_growing() {
        let source = "<a>".repeat(10_000);
        let _ = parse(source.as_bytes());
    }

    #[test]
    fn a_publisher_with_a_year_of_archive_does_not_become_the_whole_application() {
        let mut source = String::from("<rss><channel><title>T</title>");
        for index in 0..500 {
            source.push_str("<item><title>Item ");
            source.push_str(&index.to_string());
            source.push_str("</title></item>");
        }
        source.push_str("</channel></rss>");
        assert_eq!(read(&source).items.len(), MAX_ITEMS);
    }

    #[test]
    fn an_attribute_is_read_whichever_quote_it_used() {
        assert_eq!(attribute(" href='x' rel=\"y\"", "href"), Some("x"));
        assert_eq!(attribute(" href='x' rel=\"y\"", "rel"), Some("y"));
        assert_eq!(attribute(" href='x'", "missing"), None);
        assert_eq!(attribute("", "href"), None);
    }

    #[test]
    fn only_the_five_entities_xml_defines_are_decoded_here() {
        assert_eq!(decode_entities("a &amp; b"), "a & b");
        assert_eq!(decode_entities("&lt;p&gt;"), "<p>");
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        // Left for the HTML converter, which knows the full set.
        assert_eq!(decode_entities("&nbsp;"), "&nbsp;");
        assert_eq!(decode_entities("bare & ampersand"), "bare & ampersand");
    }

    #[test]
    fn an_item_with_nothing_in_it_is_not_a_row() {
        assert!(Item::default().is_empty());
        let feed = parse(b"<rss><channel><title>T</title><item></item></channel></rss>");
        assert_eq!(feed.expect("a feed").items.len(), 0);
    }

    #[test]
    fn an_entry_that_is_only_a_picture_still_reads_as_something() {
        // The shape a comic feed sends: one escaped <img> in the summary and
        // no prose at all. Stripping the tag left the article blank, which on
        // the panel is indistinguishable from a download that failed.
        let atom = br#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>A comic</title>
<link href="https://example.com/" rel="alternate"></link>
<entry><title>Forth</title>
<link href="https://example.com/1/" rel="alternate"></link>
<updated>2026-07-27T00:00:00Z</updated>
<summary type="html">&lt;img src="https://example.com/1.png" alt="NOTATION POLISH REVERSE" /&gt;</summary>
</entry></feed>"#;
        let feed = parse(atom).expect("an Atom feed");
        assert_eq!(feed.title, "A comic");
        let item = &feed.items[0];
        assert_eq!(item.title, "Forth");
        assert_eq!(item.body, "[NOTATION POLISH REVERSE]");
    }

    #[test]
    fn a_decorative_image_does_not_become_an_empty_pair_of_brackets() {
        let atom = br#"<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title>
<entry><title>E</title><link href="https://example.com/1/" rel="alternate"></link>
<summary type="html">&lt;img src="px.gif" alt="" /&gt;Real words.</summary>
</entry></feed>"#;
        let feed = parse(atom).expect("an Atom feed");
        assert_eq!(feed.items[0].body, "Real words.");
    }

    #[test]
    fn a_feed_cut_off_mid_flight_keeps_every_item_that_arrived_whole() {
        // This is what the fetch budget's comment promises, so it is measured
        // rather than assumed: a body that stops in the middle of an item
        // keeps the items before it, and keeps them intact.
        let mut xml = String::from("<rss><channel><title>Long</title>");
        for number in 0..20 {
            let _ = write!(
                xml,
                "<item><title>Item {number}</title>\
                 <link>https://example.com/{number}</link>\
                 <description>Body of item {number}.</description></item>"
            );
        }
        xml.push_str("</channel></rss>");
        let whole = parse(xml.as_bytes()).expect("the whole feed");
        assert_eq!(whole.items.len(), 20);

        let cut = parse(&xml.as_bytes()[..xml.len() / 2]).expect("a cut feed still reads");
        assert!(
            (1..20).contains(&cut.items.len()),
            "expected some but not all, got {}",
            cut.items.len()
        );
        assert_eq!(cut.title, "Long", "the channel title survives the cut");
        for (index, item) in cut.items.iter().enumerate() {
            assert_eq!(item.title, format!("Item {index}"));
            assert_eq!(item.body, format!("Body of item {index}."));
            assert_eq!(item.link, format!("https://example.com/{index}"));
        }
    }

    #[test]
    fn half_a_json_feed_is_not_a_feed_at_all() {
        // The other half of the same promise, and the reason a cut answer is
        // reported as too large rather than as not a feed: there is no prefix
        // of a JSON document to recover, so nothing can be kept.
        let mut items = String::new();
        for number in 0..20 {
            if number > 0 {
                items.push(',');
            }
            let _ = write!(
                items,
                r#"{{"title":"Item {number}","url":"https://example.com/{number}","content_text":"Body {number}."}}"#
            );
        }
        let json = format!(
            r#"{{"version":"https://jsonfeed.org/version/1","title":"Long","items":[{items}]}}"#
        );
        assert_eq!(
            parse(json.as_bytes()).expect("the whole feed").items.len(),
            20
        );
        assert!(parse(&json.as_bytes()[..json.len() / 2]).is_none());
    }
}
