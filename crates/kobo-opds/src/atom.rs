//! OPDS 1.2: an Atom feed carrying the `opds`, `dcterms`, `opensearch`, `thr`
//! and (in the wild, though never standardised) `media` namespaces.
//!
//! This is built on [`kobo_xml`]'s flat event scanner rather than a tree, the
//! same way `examples/rss`'s feed reader is, and for the same reason: the
//! documents here are written by strangers and served by machines nobody in
//! this workspace controls, and the scanner's policy of stopping at the first
//! thing it cannot make sense of — rather than guessing — is exactly the
//! policy this module wants too. What is added here, on top of the scanner,
//! is a small recursive-descent walk keyed on element *local* name (a
//! namespace prefix is whichever string the document's author happened to
//! bind it to, so `opds:price` and `o:price` must read the same), and enough
//! state to reconstruct the handful of places OPDS nests: a link's price and
//! indirect acquisition, an indirect acquisition nested inside another.
//!
//! # Summary and content
//!
//! OPDS 1.2 §5.1.3 says that when an entry carries both `<summary>` and
//! `<content>`, the richer one — `content` — wins. `content type="xhtml"`
//! wraps real child elements (Gutenberg's entry documents use this: a `<div>`
//! full of `<p>`s); `content type="html"` and `type="text"` carry the same
//! prose as escaped text instead. Both end up needing their markup reduced to
//! plain text, and both need it done exactly once — the scanner has already
//! decoded `&amp;lt;` into `<` by the time this module sees it, and running
//! that text back through the XML scanner a second time would decode it
//! again, which is how a title that quoted a `<script>` tag as prose becomes
//! one. So the `xhtml` case reads real elements and inserts its own line
//! breaks at block boundaries ([`read_rich_text`]); the `html`/`text` case
//! reads a single decoded run and strips angle-bracket spans out of it by
//! hand ([`strip_tags`]), touching no ampersand at all.

use crate::{
    acquisition_kind, kept_relation, Acquisition, AcquisitionKind, Category, Facet, FacetGroup,
    Feed, FeedKind, Group, Image, ImageSource, Indirect, Link, Navigation, Price, Publication,
    Relation, Version, MAX_ENTRIES, MAX_PER_ENTRY, MAX_TEXT_FIELD_BYTES,
};
use kobo_xml::{decode_entities, scan, split_name, Event};

/// The most indirect-acquisition or facet siblings read at one nesting level.
///
/// [`kobo_xml::MAX_DEPTH`] bounds how deep a document can nest, but not how
/// many same-depth siblings one element can have — a hostile `<link>` could
/// carry thousands of `<opds:indirectAcquisition>` children without ever
/// nesting deeply. The richest real example in the fixtures nests two deep
/// with one child at each level.
const MAX_SIBLINGS: usize = 32;

/// Parses one OPDS 1.2 document, whether it is a `<feed>` or — as OPDS 1.2
/// §5.1.2's "complete catalog entry" allows, and as
/// `feedbooks-test-catalog/acquisition/entry.xml` is shaped — a bare
/// `<entry>` with no enclosing feed at all.
pub(crate) fn parse(input: &str, base: &str) -> Feed {
    let mut decoded: Vec<String> = Vec::new();
    let mut events: Vec<Event<'_>> = Vec::new();
    scan(input, &mut decoded, |event| events.push(event));

    let mut feed = Feed {
        version: Version::Atom,
        ..Feed::default()
    };
    let mut cursor = Cursor {
        events: &events,
        decoded: &decoded,
        index: 0,
    };

    match cursor.next() {
        Some(Event::Open {
            name: "feed" | "atom:feed",
            ..
        }) => walk_feed(&mut cursor, base, &mut feed),
        Some(Event::Open {
            name: "entry" | "atom:entry",
            attributes: _,
        }) => {
            if let Some(outcome) = parse_entry(&mut cursor, base) {
                push_entry(&mut feed, outcome);
            }
        }
        _ => {}
    }
    feed
}

/// A cursor over the flat event stream a whole document was scanned into.
///
/// Materialising every event before walking them (rather than driving the
/// walk from inside [`kobo_xml::scan`]'s callback) is what lets this module
/// be written as ordinary recursive-descent functions that call
/// `cursor.next()` — the same trade `examples/rss` makes, for the same
/// reason: OPDS nests price and indirect-acquisition inside a link inside an
/// entry, and a callback-driven walk would need its own explicit stack to
/// track that context, which is just this struct with extra steps.
struct Cursor<'a> {
    events: &'a [Event<'a>],
    decoded: &'a [String],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn next(&mut self) -> Option<Event<'a>> {
        let event = self.events.get(self.index).copied();
        if event.is_some() {
            self.index += 1;
        }
        event
    }

    fn peek(&self) -> Option<Event<'a>> {
        self.events.get(self.index).copied()
    }

    fn text_of(&self, event: Event<'a>) -> &'a str {
        match event {
            Event::Text(text) => text,
            Event::Owned(index) => self.decoded.get(index).map_or("", String::as_str),
            Event::Open { .. } | Event::Close { .. } => "",
        }
    }
}

/// An element's local name, tolerating whatever namespace prefix the
/// document's author chose.
fn local(name: &str) -> &str {
    split_name(name).1
}

/// Consumes events until the matching close of the element whose `Open` was
/// already consumed, concatenating text and ignoring any nested markup's
/// structure (its text still counts, which is the deliberately permissive
/// reading `dcterms:language` and friends want — a stray inline element
/// inside a simple field should not make the field disappear).
///
/// Capped at [`MAX_TEXT_FIELD_BYTES`] while reading rather than after, so an
/// enormous field costs a bounded copy rather than an unbounded one.
fn read_text(cursor: &mut Cursor<'_>) -> String {
    let mut buffer = String::new();
    let mut depth = 0usize;
    while let Some(event) = cursor.next() {
        match event {
            Event::Open { .. } => depth += 1,
            Event::Close { .. } => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Event::Text(_) | Event::Owned(_) => {
                if buffer.len() < MAX_TEXT_FIELD_BYTES {
                    buffer.push_str(cursor.text_of(event));
                }
            }
        }
    }
    buffer.trim().to_owned()
}

/// Elements after which prose reduced from real markup gets a paragraph
/// break — the same list `examples/rss` uses for Atom `type="xhtml"` content,
/// because it is the same problem: without it, a five-paragraph book summary
/// arrives as one paragraph.
const BREAKS_LINE: [&str; 8] = ["p", "br", "div", "li", "h1", "h2", "h3", "blockquote"];

/// Reads `content type="xhtml"`'s real child elements as prose, inserting a
/// paragraph break at each block-level close.
fn read_rich_text(cursor: &mut Cursor<'_>) -> String {
    let mut buffer = String::new();
    let mut depth = 0usize;
    while let Some(event) = cursor.next() {
        match event {
            Event::Open { .. } => depth += 1,
            Event::Close { name } => {
                if BREAKS_LINE
                    .iter()
                    .any(|tag| local(name).eq_ignore_ascii_case(tag))
                {
                    buffer.push_str("\n\n");
                }
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Event::Text(_) | Event::Owned(_) => {
                if buffer.len() < MAX_TEXT_FIELD_BYTES {
                    buffer.push_str(cursor.text_of(event));
                }
            }
        }
    }
    collapse_blank_lines(buffer.trim())
}

/// Strips markup out of text that has already been through entity decoding
/// once — `content type="html"` writes its markup escaped, so by the time
/// [`read_text`] hands it back, `&lt;p&gt;` is already the two characters
/// `<p>`. This walks the string once, byte range by byte range, and decodes
/// no entity at all: running the result back through [`kobo_xml::decode_entities`]
/// is exactly how `&amp;lt;` would become a real `<` that nobody's markup
/// ever meant to write.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('>') else {
            // No closing bracket: this was never a tag, so keep it as text.
            out.push_str(&rest[start..]);
            return collapse_blank_lines(out.trim());
        };
        let tag = &tail[..end];
        let closing = tag.starts_with('/');
        let name = tag
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if closing && BREAKS_LINE.iter().any(|b| name.eq_ignore_ascii_case(b)) {
            out.push_str("\n\n");
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    collapse_blank_lines(out.trim())
}

/// Collapses runs of three or more newlines down to a single paragraph
/// break, so a source document's own blank lines do not multiply with the
/// breaks this module inserts.
fn collapse_blank_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0u8;
    for line in text.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out.trim().to_owned()
}

/// Reads whichever of `summary`/`content` is richer, per §5.1.3.
///
/// Kept as two optional slots that are reconciled once the entry is
/// complete, rather than compared incrementally, because `content` beats
/// `summary` regardless of which order the two arrive in — the same
/// "full text wins whichever order it arrives in" property `examples/rss`
/// tests for its own summary/content handling.
fn read_prose(cursor: &mut Cursor<'_>, attributes: &str) -> String {
    let kind = attr_local(attributes, "type").unwrap_or_default();
    if kind.eq_ignore_ascii_case("xhtml") {
        read_rich_text(cursor)
    } else {
        strip_tags(&read_text(cursor))
    }
}

/// Consumes and discards one element's whole subtree, having already
/// consumed its `Open`.
fn skip_element(cursor: &mut Cursor<'_>) {
    let mut depth = 0usize;
    while let Some(event) = cursor.next() {
        match event {
            Event::Open { .. } => depth += 1,
            Event::Close { .. } => {
                if depth == 0 {
                    return;
                }
                depth -= 1;
            }
            Event::Text(_) | Event::Owned(_) => {}
        }
    }
}

/// Walks one element's raw attribute text looking for `name` by *local*
/// name — tolerating a prefix the way [`local`] does for elements — since
/// [`kobo_xml::attribute`] only matches a name given exactly, and
/// `opds:facetGroup`, `opds:activeFacet` and `thr:count` all carry prefixes
/// this module cannot assume in advance.
fn attr_local(attributes: &str, want: &str) -> Option<String> {
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
        let value = &tail[..end];
        if !name.is_empty() && local(name).eq_ignore_ascii_case(want) {
            return Some(if value.contains('&') {
                decode_entities(value)
            } else {
                value.to_owned()
            });
        }
        rest = &tail[end + 1..];
    }
    None
}

/// Reads the `kind=navigation`/`kind=acquisition` parameter out of a `type`
/// attribute's value (OPDS 1.2 §7.1.3), e.g.
/// `application/atom+xml;profile=opds-catalog;kind=acquisition`.
fn feed_kind(type_attr: &str) -> Option<FeedKind> {
    type_attr.split(';').map(str::trim).find_map(|part| {
        let value = part.strip_prefix("kind=")?;
        if value.eq_ignore_ascii_case("navigation") {
            Some(FeedKind::Navigation)
        } else if value.eq_ignore_ascii_case("acquisition") {
            Some(FeedKind::Acquisition)
        } else {
            None
        }
    })
}

fn walk_feed(cursor: &mut Cursor<'_>, base: &str, feed: &mut Feed) {
    loop {
        match cursor.peek() {
            None => return,
            Some(Event::Close { .. }) => {
                cursor.next();
                return;
            }
            Some(Event::Open { name, attributes }) => {
                cursor.next();
                match local(name) {
                    "title" if feed.title.is_none() => feed.title = non_empty(read_text(cursor)),
                    "subtitle" if feed.subtitle.is_none() => {
                        feed.subtitle = non_empty(read_text(cursor));
                    }
                    "updated" if feed.updated.is_none() => {
                        feed.updated = non_empty(read_text(cursor));
                    }
                    "icon" => {
                        let href = read_text(cursor);
                        if let Some(href) = crate::url::safe_href(base, &href) {
                            push_link(&mut feed.links, vec![Relation::Icon], href, None, None);
                        }
                    }
                    "link" => {
                        let children = read_link_children(cursor);
                        handle_feed_link(feed, base, attributes, &children);
                    }
                    "totalResults" => {
                        feed.pagination.total = read_text(cursor).trim().parse().ok();
                    }
                    "itemsPerPage" => {
                        feed.pagination.per_page = read_text(cursor).trim().parse().ok();
                    }
                    "startIndex" => {
                        feed.pagination.start_index = read_text(cursor).trim().parse().ok();
                    }
                    "entry" => {
                        if feed.navigation.len() + feed.publications.len() < MAX_ENTRIES {
                            if let Some(outcome) = parse_entry(cursor, base) {
                                push_entry(feed, outcome);
                            }
                        } else {
                            skip_element(cursor);
                        }
                    }
                    _ => skip_element(cursor),
                }
            }
            Some(Event::Text(_) | Event::Owned(_)) => {
                cursor.next();
            }
        }
    }
}

fn non_empty(text: String) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// The pieces a `<link>`'s children (`opds:price`, `opds:indirectAcquisition`,
/// `opds:unavailable`) can carry. Populated whether the link was self-closing
/// (none of this) or not.
#[derive(Default)]
struct LinkChildren {
    price: Option<Price>,
    indirect: Vec<Indirect>,
    unavailable: bool,
}

fn read_link_children(cursor: &mut Cursor<'_>) -> LinkChildren {
    let mut result = LinkChildren::default();
    loop {
        match cursor.next() {
            None | Some(Event::Close { .. }) => return result,
            Some(Event::Open { name, attributes }) => match local(name) {
                "price" => {
                    let currency = attr_local(attributes, "currencycode");
                    let value = read_text(cursor).trim().to_owned();
                    result.price = Some(Price { currency, value });
                }
                "indirectAcquisition" => {
                    if result.indirect.len() < MAX_SIBLINGS {
                        result.indirect.push(read_indirect(cursor, attributes));
                    } else {
                        skip_element(cursor);
                    }
                }
                "unavailable" => {
                    result.unavailable = true;
                    skip_element(cursor);
                }
                _ => skip_element(cursor),
            },
            Some(Event::Text(_) | Event::Owned(_)) => {}
        }
    }
}

fn read_indirect(cursor: &mut Cursor<'_>, attributes: &str) -> Indirect {
    let media_type = attr_local(attributes, "type");
    let mut indirect = Vec::new();
    loop {
        match cursor.next() {
            None | Some(Event::Close { .. }) => {
                return Indirect {
                    media_type,
                    indirect,
                };
            }
            Some(Event::Open { name, attributes }) if local(name) == "indirectAcquisition" => {
                if indirect.len() < MAX_SIBLINGS {
                    indirect.push(read_indirect(cursor, attributes));
                } else {
                    skip_element(cursor);
                }
            }
            Some(Event::Open { .. }) => skip_element(cursor),
            Some(Event::Text(_) | Event::Owned(_)) => {}
        }
    }
}

/// Splits a `rel` attribute into its space-separated tokens (a single Atom
/// `link` may carry several relations at once).
fn rel_tokens(rel: &str) -> impl Iterator<Item = &str> {
    rel.split_whitespace()
}

fn push_link(
    links: &mut Vec<Link>,
    rel: Vec<Relation>,
    href: String,
    media_type: Option<String>,
    title: Option<String>,
) {
    if links.len() < MAX_PER_ENTRY {
        links.push(Link {
            rel,
            href,
            media_type,
            title,
        });
    }
}

/// Handles one `<link>` that is a direct child of `<feed>` (not of an
/// `<entry>`): facets, feed-level groups-by-relation are entry-only so do not
/// occur here, and everything in [`kept_relation`]'s table.
fn handle_feed_link(feed: &mut Feed, base: &str, attributes: &str, children: &LinkChildren) {
    let Some(raw_href) = attr_local(attributes, "href") else {
        return;
    };
    let rel = attr_local(attributes, "rel").unwrap_or_default();
    let media_type = attr_local(attributes, "type");
    let title = attr_local(attributes, "title");

    if rel_tokens(&rel).any(|token| token == "http://opds-spec.org/facet") {
        // A facet link's only children would be `opds:price` or
        // `opds:indirectAcquisition`, neither of which OPDS 1.2 ever puts on
        // a facet — `children` is read (its own children fully consumed by
        // `read_link_children` already) and then has nothing worth keeping.
        let _ = children;
        if let Some(href) = crate::url::safe_href(base, &raw_href) {
            add_facet(feed, attributes, href, title);
        }
        return;
    }

    let Some(href) = crate::url::safe_href(base, &raw_href) else {
        return;
    };
    let matched: Vec<Relation> = rel_tokens(&rel).filter_map(kept_relation).collect();
    if !matched.is_empty() {
        push_link(&mut feed.links, matched, href, media_type, title);
    }
}

fn add_facet(feed: &mut Feed, attributes: &str, href: String, title: Option<String>) {
    let group_title = attr_local(attributes, "facetGroup").unwrap_or_default();
    let active = attr_local(attributes, "activeFacet")
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let count = attr_local(attributes, "count").and_then(|value| value.parse().ok());
    let facet = Facet {
        title: title.unwrap_or_default(),
        href,
        active,
        count,
    };
    if let Some(group) = feed
        .facets
        .iter_mut()
        .find(|group| group.title == group_title)
    {
        if group.facets.len() < MAX_PER_ENTRY {
            group.facets.push(facet);
        }
    } else if feed.facets.len() < MAX_PER_ENTRY {
        feed.facets.push(FacetGroup {
            title: group_title,
            facets: vec![facet],
        });
    }
}

/// Accumulates one `<entry>`'s fields as they arrive, since the scanner's
/// flat stream means every field is read before it is known whether the
/// entry will turn out to be a [`Navigation`] or a [`Publication`].
#[derive(Default)]
struct EntryBuilder {
    title: String,
    id: Option<String>,
    authors: Vec<String>,
    summary: Option<String>,
    content: Option<String>,
    language: Option<String>,
    issued: Option<String>,
    published: Option<String>,
    updated: Option<String>,
    rights: Option<String>,
    extent: Option<String>,
    categories: Vec<Category>,
    images: Vec<Image>,
    acquisition: Vec<Acquisition>,
    /// `rel="enclosure"` links, kept aside rather than in `acquisition`
    /// directly: they only count as an acquisition link "when the entry has
    /// no other acquisition link" (Standard Ebooks marks every EPUB this
    /// way), so promoting them has to wait until the entry is complete.
    enclosure: Vec<Acquisition>,
    links: Vec<Link>,
    subsection: Option<(String, Option<String>, Option<FeedKind>)>,
    group: Option<(String, Option<String>)>,
    /// Whether the entry carried a link whose *relation* named it an
    /// acquisition (or an `enclosure`) link, independent of whether that
    /// link's href survived the `https`-only safety check. Classification
    /// into [`Publication`] vs [`Navigation`] has to key on this rather than
    /// on whether `acquisition`/`enclosure` ended up non-empty: an entry
    /// whose one download link happened to be `http://` is still a book that
    /// failed to offer a safe download, not a shelf to browse into.
    has_acquisition_relation: bool,
    has_enclosure_relation: bool,
}

enum EntryOutcome {
    Navigation(Navigation),
    // Boxed so a run of navigation entries — the common case for a purely
    // navigational feed like Gutenberg's search results — does not pay for
    // `Publication`'s much larger size in every `EntryOutcome` on the stack.
    Publication(Box<Publication>, Option<(String, Option<String>)>),
}

fn parse_entry(cursor: &mut Cursor<'_>, base: &str) -> Option<EntryOutcome> {
    let mut entry = EntryBuilder::default();
    loop {
        match cursor.next() {
            None | Some(Event::Close { .. }) => break,
            Some(Event::Open { name, attributes }) => {
                handle_entry_child(cursor, base, &mut entry, local(name), attributes);
            }
            Some(Event::Text(_) | Event::Owned(_)) => {}
        }
    }
    finish_entry(entry, base)
}

// One `match` arm per entry-level element name this module reads is the
// whole job of this function; splitting it into several functions to satisfy
// a line count would scatter the one place that answers "what does an
// `<entry>` child mean" across several, which is a worse read than the length.
#[allow(clippy::too_many_lines)]
fn handle_entry_child(
    cursor: &mut Cursor<'_>,
    base: &str,
    entry: &mut EntryBuilder,
    name: &str,
    attributes: &str,
) {
    match name {
        "title" => entry.title = read_text(cursor),
        "id" => entry.id = non_empty(read_text(cursor)),
        "published" => entry.published = non_empty(read_text(cursor)),
        "updated" => entry.updated = non_empty(read_text(cursor)),
        "rights" => entry.rights = non_empty(read_text(cursor)),
        "language" => entry.language = non_empty(read_text(cursor)),
        "issued" => entry.issued = non_empty(read_text(cursor)),
        "extent" => entry.extent = non_empty(read_text(cursor)),
        "summary" => entry.summary = non_empty(read_prose(cursor, attributes)),
        "content" => entry.content = non_empty(read_prose(cursor, attributes)),
        "author" => {
            if let Some(name) = read_person_name(cursor) {
                if entry.authors.len() < MAX_PER_ENTRY {
                    entry.authors.push(name);
                }
            }
        }
        "category" => {
            let term = attr_local(attributes, "term");
            skip_element(cursor);
            if let Some(term) = term {
                if entry.categories.len() < MAX_PER_ENTRY {
                    entry.categories.push(Category {
                        term,
                        label: attr_local(attributes, "label"),
                        scheme: attr_local(attributes, "scheme"),
                    });
                }
            }
        }
        "thumbnail" => {
            // Yahoo Media RSS's `media:thumbnail`, Standard Ebooks' cover.
            let raw_href = attr_local(attributes, "url");
            let media_type = attr_local(attributes, "type");
            let width = attr_local(attributes, "width").and_then(|v| v.parse().ok());
            let height = attr_local(attributes, "height").and_then(|v| v.parse().ok());
            skip_element(cursor);
            if let Some(href) = raw_href.and_then(|raw| crate::url::safe_href(base, &raw)) {
                if entry.images.len() < MAX_PER_ENTRY {
                    entry.images.push(Image {
                        href: ImageSource::Url(href),
                        media_type,
                        width,
                        height,
                        thumbnail: true,
                    });
                }
            }
        }
        "link" => {
            let raw_href = attr_local(attributes, "href");
            let rel = attr_local(attributes, "rel").unwrap_or_default();
            let media_type = attr_local(attributes, "type");
            let title = attr_local(attributes, "title");
            let length = attr_local(attributes, "length").and_then(|v| v.parse().ok());
            let children = read_link_children(cursor);
            let Some(raw_href) = raw_href else { return };
            handle_entry_link(
                base, entry, &raw_href, &rel, media_type, title, length, children,
            );
        }
        _ => skip_element(cursor),
    }
}

/// Reads an Atom `<author>`/`<contributor>`-shaped element's `<name>` child.
fn read_person_name(cursor: &mut Cursor<'_>) -> Option<String> {
    let mut name = None;
    loop {
        match cursor.next() {
            None | Some(Event::Close { .. }) => return name,
            Some(Event::Open { name: child, .. }) if local(child) == "name" => {
                let text = read_text(cursor);
                if name.is_none() && !text.is_empty() {
                    name = Some(text);
                }
            }
            Some(Event::Open { .. }) => skip_element(cursor),
            Some(Event::Text(_) | Event::Owned(_)) => {}
        }
    }
}

// Every argument here is a piece of one `<link>` element (its href, its rel,
// its type, its title, its length attribute, its price/indirect children):
// bundling them into a struct first would only move the same seven fields
// one level up without changing what the function does with them.
#[allow(clippy::too_many_arguments)]
fn handle_entry_link(
    base: &str,
    entry: &mut EntryBuilder,
    raw_href: &str,
    rel: &str,
    media_type: Option<String>,
    title: Option<String>,
    length: Option<u64>,
    children: LinkChildren,
) {
    let tokens: Vec<&str> = rel_tokens(rel).collect();

    if tokens.contains(&"http://opds-spec.org/image") {
        if let Some(image) = build_image(base, raw_href, media_type.clone(), false) {
            if entry.images.len() < MAX_PER_ENTRY {
                entry.images.push(image);
            }
        }
    }
    if tokens.contains(&"http://opds-spec.org/image/thumbnail") {
        if let Some(image) = build_image(base, raw_href, media_type.clone(), true) {
            if entry.images.len() < MAX_PER_ENTRY {
                entry.images.push(image);
            }
        }
    }

    if tokens.contains(&"http://opds-spec.org/group") {
        entry.group = Some((
            title.clone().unwrap_or_default(),
            crate::url::safe_href(base, raw_href),
        ));
    }

    if let Some(kind) = tokens.iter().find_map(|t| acquisition_kind(t)) {
        entry.has_acquisition_relation = true;
        if let Some(href) = crate::url::safe_href(base, raw_href) {
            if entry.acquisition.len() < MAX_PER_ENTRY {
                entry.acquisition.push(Acquisition {
                    kind,
                    href,
                    media_type,
                    title,
                    length,
                    price: children.price,
                    indirect: children.indirect,
                    available: !children.unavailable,
                });
            }
        }
        return;
    }

    if tokens.contains(&"enclosure") {
        entry.has_enclosure_relation = true;
        if let Some(href) = crate::url::safe_href(base, raw_href) {
            if entry.enclosure.len() < MAX_PER_ENTRY {
                entry.enclosure.push(Acquisition {
                    kind: AcquisitionKind::Generic,
                    href,
                    media_type,
                    title,
                    length,
                    price: None,
                    indirect: Vec::new(),
                    available: true,
                });
            }
        }
        return;
    }

    if tokens.contains(&"subsection") && entry.subsection.is_none() {
        entry.subsection = Some((
            raw_href.to_owned(),
            title.clone(),
            media_type.as_deref().and_then(feed_kind),
        ));
    }

    let kept: Vec<Relation> = tokens
        .iter()
        .filter(|t| matches!(**t, "alternate" | "related" | "self" | "subsection"))
        .filter_map(|t| kept_relation(t))
        .collect();
    if !kept.is_empty() {
        if let Some(href) = crate::url::safe_href(base, raw_href) {
            push_link(&mut entry.links, kept, href, media_type, title);
        }
    }
}

fn build_image(
    base: &str,
    raw_href: &str,
    media_type: Option<String>,
    thumbnail: bool,
) -> Option<Image> {
    if let Some((decoded_type, bytes)) = crate::url::decode_data_image(raw_href) {
        return Some(Image {
            href: ImageSource::Inline {
                media_type: decoded_type,
                bytes,
            },
            media_type,
            width: None,
            height: None,
            thumbnail,
        });
    }
    let href = crate::url::safe_href(base, raw_href)?;
    Some(Image {
        href: ImageSource::Url(href),
        media_type,
        width: None,
        height: None,
        thumbnail,
    })
}

fn finish_entry(entry: EntryBuilder, base: &str) -> Option<EntryOutcome> {
    // Classification keys on whether the entry *stated* an acquisition
    // relation, not on whether any such link survived the `https`-only
    // filter — a book whose only download link was `http://` is still a
    // book with a missing download, not a shelf to browse into. See
    // `EntryBuilder::has_acquisition_relation`.
    let is_publication = entry.has_acquisition_relation || entry.has_enclosure_relation;
    let acquisition = if !entry.acquisition.is_empty() {
        entry.acquisition
    } else if entry.has_acquisition_relation {
        Vec::new()
    } else {
        entry.enclosure
    };
    let summary = entry.content.or(entry.summary);

    if !is_publication {
        let (href, link_title, kind) = entry.subsection.or_else(|| {
            entry
                .links
                .iter()
                .find(|link| link.matches(&Relation::Alternate))
                .map(|link| (link.href.clone(), link.title.clone(), None))
        })?;
        let href = crate::url::safe_href(base, &href)?;
        let title = if entry.title.is_empty() {
            link_title.unwrap_or_default()
        } else {
            entry.title
        };
        return Some(EntryOutcome::Navigation(Navigation {
            title,
            href,
            summary,
            kind,
            rel: Some(Relation::Subsection),
            thumbnail: entry.images.into_iter().find(|image| image.thumbnail),
        }));
    }

    Some(EntryOutcome::Publication(
        Box::new(Publication {
            title: entry.title,
            identifier: entry.id,
            authors: entry.authors,
            summary,
            language: entry.language,
            issued: entry.issued,
            published: entry.published,
            updated: entry.updated,
            publisher: None,
            rights: entry.rights,
            extent: entry.extent,
            categories: entry.categories,
            series: None,
            images: entry.images,
            acquisition,
            links: entry.links,
        }),
        entry.group,
    ))
}

fn push_entry(feed: &mut Feed, outcome: EntryOutcome) {
    match outcome {
        EntryOutcome::Navigation(navigation) => feed.navigation.push(navigation),
        EntryOutcome::Publication(publication, group) => {
            if let Some((title, href)) = group {
                let existing = feed
                    .groups
                    .iter_mut()
                    .find(|g| g.title == title && g.href == href);
                let target = if let Some(group) = existing {
                    group
                } else {
                    feed.groups.push(Group {
                        title,
                        href,
                        ..Group::default()
                    });
                    feed.groups.last_mut().expect("just pushed")
                };
                target.publications.push((*publication).clone());
            }
            feed.publications.push(*publication);
        }
    }
}

/// A search template read from an `OpenSearch` description document (1.x
/// search is a separate document, linked via `rel="search"`, rather than a
/// link on the feed itself), already resolved to an absolute `https` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchTemplate {
    template: String,
}

impl SearchTemplate {
    /// Substitutes `terms` for `OpenSearch`'s `{searchTerms}` placeholder.
    ///
    /// Percent-encodes `terms` so that a search for `dickens & sons` cannot
    /// inject its own `&param=value` into the request — the template itself
    /// was already resolved and checked for `https` in [`parse_opensearch`],
    /// so this step can never fail.
    #[must_use]
    pub fn expand(&self, terms: &str) -> String {
        self.template
            .replace("{searchTerms}", &crate::percent_encode(terms))
    }
}

/// Rewrites an absolute `http://` URL to `https://`, leaving everything else
/// (including a relative reference, which inherits `base`'s scheme when it is
/// resolved) untouched.
///
/// Upgrading is safe where downgrading would not be: Gutenberg's `OpenSearch`
/// description hands out `http://m.gutenberg.org/ebooks/search.opds/?query=`
/// as its Atom search template even though the description document itself
/// and every other link in the catalog are `https`. Refusing the template
/// outright because of that one scheme would make Gutenberg's own search
/// unusable (`https://m.gutenberg.org/ebooks/search.opds/?query=dickens`
/// answers `200` with the feed the plain-`http` template promised), whereas
/// every safety rule elsewhere in this crate exists to stop a link from being
/// followed to somewhere *less* secure than the reader asked for — and `https`
/// is never that.
fn upgrade_http(href: &str) -> String {
    if href.len() >= 7 && href.as_bytes()[..7].eq_ignore_ascii_case(b"http://") {
        format!("https://{}", &href[7..])
    } else {
        href.to_owned()
    }
}

/// Picks the one `Url` element, among however many an `OpenSearch` description
/// offers, that is worth treating as an OPDS search.
///
/// Gutenberg's own description
/// (`https://www.gutenberg.org/catalog/osd-books.xml`) offers three: a
/// `text/html` template pointing at a web page, an `application/atom+xml`
/// template pointing at the feed this crate reads, and an
/// `application/x-suggestions+json` typeahead marked `rel="suggestions"`.
/// Only the second is a catalog; the first would hand a client an HTML page
/// to try to parse as OPDS, and the third is a different feature entirely.
/// When more than one feed-shaped template is offered, one whose type states
/// an explicit OPDS profile is preferred over a bare `application/atom+xml`,
/// on the reasoning that a catalog specific enough to say "this is OPDS"
/// probably means it more precisely than one that just says "this is Atom."
fn choose_opensearch_template(
    candidates: &[(Option<String>, Option<String>, String)],
) -> Option<&str> {
    let mut plain_atom: Option<&str> = None;
    let mut opds_profile: Option<&str> = None;
    for (type_attr, rel, template) in candidates {
        if rel
            .as_deref()
            .is_some_and(|rel| rel.eq_ignore_ascii_case("suggestions"))
        {
            continue;
        }
        let lower_type = type_attr
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if lower_type.contains("html") || lower_type.contains("suggestions") {
            continue;
        }
        let looks_like_a_feed =
            type_attr.is_none() || lower_type.contains("atom") || lower_type.contains("opds");
        if !looks_like_a_feed {
            continue;
        }
        if lower_type.contains("opds") {
            opds_profile.get_or_insert(template.as_str());
        } else {
            plain_atom.get_or_insert(template.as_str());
        }
    }
    opds_profile.or(plain_atom)
}

/// Reads an `OpenSearch` description document's search template, since OPDS
/// 1.x search is not a link on the feed but a whole separate document a
/// `rel="search"` link points at.
///
/// `base` is the description document's own URL — required, not optional,
/// because the template is frequently relative
/// (`feedbooks-test-catalog/opensearch.xml` writes
/// `search/results.xml#{searchTerms}`, putting the term in a fragment rather
/// than a query parameter) and because a `http://` template is upgraded to
/// `https` rather than refused (see [`upgrade_http`]) — an upgrade that only
/// makes sense relative to the page that offered it.
///
/// Returns `None` when no eligible `Url` element exists, or when the chosen
/// one does not resolve to something this crate will fetch — which the
/// `https`-only rule in [`crate::url::safe_href`] applies here exactly as it
/// does to every other link.
#[must_use]
pub fn parse_opensearch(bytes: &[u8], base: &str) -> Option<SearchTemplate> {
    let text = String::from_utf8_lossy(bytes);
    let mut decoded: Vec<String> = Vec::new();
    let mut candidates: Vec<(Option<String>, Option<String>, String)> = Vec::new();
    scan(&text, &mut decoded, |event| {
        if let Event::Open { name, attributes } = event {
            if local(name) == "Url" {
                if let Some(template) = attr_local(attributes, "template") {
                    if candidates.len() < MAX_SIBLINGS {
                        candidates.push((
                            attr_local(attributes, "type"),
                            attr_local(attributes, "rel"),
                            template,
                        ));
                    }
                }
            }
        }
    });
    let chosen = choose_opensearch_template(&candidates)?;
    let template = crate::url::safe_href(base, &upgrade_http(chosen))?;
    Some(SearchTemplate { template })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcquisitionKind, FeedKind, ImageSource};

    const GUTENBERG_SEARCH: &str =
        include_str!("../tests/fixtures/gutenberg/search-navigation.xml");
    const GUTENBERG_ENTRY: &str = include_str!("../tests/fixtures/gutenberg/entry-564.xml");
    const SE_NEW_RELEASES: &str = include_str!("../tests/fixtures/standardebooks/new-releases.xml");
    const FB_ROOT: &str = include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/root.xml");
    const FB_MAIN: &str =
        include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/acquisition/main.xml");
    const FB_PAGE2: &str =
        include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/acquisition/page2.xml");
    const FB_BLOCKS: &str =
        include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/acquisition/blocks.xml");
    const FB_OPENSEARCH: &str =
        include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/opensearch.xml");
    const SPEC_FACETS: &str = include_str!("../tests/fixtures/opds1/spec-facets.xml");
    const HOSTILE: &str = include_str!("../tests/fixtures/opds1/hostile.xml");

    const GUTENBERG_BASE: &str = "https://www.gutenberg.org/ebooks/search.opds/?query=dickens";
    const FB_MAIN_BASE: &str =
        "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/main.xml";
    const FB_ROOT_BASE: &str = "https://feedbooks.example/opds1/feedbooks-test-catalog/root.xml";

    #[test]
    fn a_search_answer_is_a_navigation_feed_rather_than_a_shelf_of_books() {
        let feed = parse(GUTENBERG_SEARCH, GUTENBERG_BASE);
        assert!(feed.is_navigation());
        assert!(!feed.navigation.is_empty());
        assert!(feed.publications.is_empty());
    }

    #[test]
    fn a_gutenberg_navigation_thumbnail_is_decoded_from_its_data_uri_rather_than_fetched() {
        let feed = parse(GUTENBERG_SEARCH, GUTENBERG_BASE);
        let entry = feed
            .navigation
            .iter()
            .find(|n| n.title == "Authors")
            .expect("the Authors entry");
        let thumbnail = entry.thumbnail.as_ref().expect("a thumbnail");
        match &thumbnail.href {
            ImageSource::Inline { media_type, bytes } => {
                assert_eq!(media_type, "image/png");
                assert!(!bytes.is_empty());
                // A PNG signature, proving these are real decoded bytes and
                // not the base64 text handed back unmolested.
                assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            }
            ImageSource::Url(_) => panic!("should be decoded, not left as a URL"),
        }
    }

    #[test]
    fn a_gutenberg_entry_document_yields_epub_acquisition_links_and_a_jpeg_cover() {
        let feed = parse(GUTENBERG_ENTRY, "https://www.gutenberg.org/ebooks/564.opds");
        assert_eq!(feed.publications.len(), 2);
        let publication = &feed.publications[0];
        assert!(publication
            .acquisition
            .iter()
            .any(|a| a.media_type.as_deref() == Some("application/epub+zip")));
        let cover = publication.cover().expect("a cover");
        assert_eq!(cover.media_type.as_deref(), Some("image/jpeg"));
        assert!(!cover.thumbnail);
    }

    #[test]
    fn gutenberg_offers_no_plain_text_so_the_epub_is_what_is_chosen() {
        let feed = parse(GUTENBERG_ENTRY, "https://www.gutenberg.org/ebooks/564.opds");
        let publication = &feed.publications[0];
        assert!(!publication
            .acquisition
            .iter()
            .any(|a| a.media_type.as_deref() == Some("text/plain")));
        let best = publication.best_acquisition().expect("an epub to read");
        assert_eq!(best.media_type.as_deref(), Some("application/epub+zip"));
    }

    #[test]
    fn an_enclosure_link_is_taken_as_an_acquisition_link_when_nothing_better_exists() {
        let feed = parse(
            SE_NEW_RELEASES,
            "https://standardebooks.org/feeds/atom/new-releases",
        );
        let publication = &feed.publications[0];
        assert!(!publication.acquisition.is_empty());
        assert!(publication
            .acquisition
            .iter()
            .all(|a| a.kind == AcquisitionKind::Generic));
    }

    #[test]
    fn a_media_thumbnail_is_taken_as_the_cover_when_there_is_no_opds_image_link() {
        let feed = parse(
            SE_NEW_RELEASES,
            "https://standardebooks.org/feeds/atom/new-releases",
        );
        let publication = &feed.publications[0];
        assert!(publication.images.iter().all(|i| i.thumbnail));
        let cover = publication.cover().expect("falls back to the thumbnail");
        assert!(cover.thumbnail);
    }

    #[test]
    fn the_several_epub_editions_are_ranked_so_the_plain_one_wins_over_the_kepub_and_the_azw3() {
        let feed = parse(
            SE_NEW_RELEASES,
            "https://standardebooks.org/feeds/atom/new-releases",
        );
        let publication = &feed.publications[0];
        let media_types: Vec<_> = publication
            .acquisition
            .iter()
            .filter_map(|a| a.media_type.as_deref())
            .collect();
        assert!(media_types.contains(&"application/kepub+zip"));
        assert!(media_types.contains(&"application/x-mobipocket-ebook"));
        let best = publication.best_acquisition().expect("an epub");
        assert_eq!(best.media_type.as_deref(), Some("application/epub+zip"));
    }

    #[test]
    fn a_navigation_feed_and_an_acquisition_feed_are_told_apart_by_their_kind_parameter() {
        let root = parse(FB_ROOT, FB_ROOT_BASE);
        assert!(root.is_navigation());
        let acquisition_entry = root
            .navigation
            .iter()
            .find(|n| n.title == "First Acquisition feed")
            .expect("an entry pointing at an acquisition feed");
        assert_eq!(acquisition_entry.kind, Some(FeedKind::Acquisition));
        let navigation_entry = root
            .navigation
            .iter()
            .find(|n| n.title == "Link: Featured")
            .expect("an entry pointing at a navigation feed");
        assert_eq!(navigation_entry.kind, Some(FeedKind::Navigation));

        let main = parse(FB_MAIN, FB_MAIN_BASE);
        assert!(main.is_acquisition());
    }

    #[test]
    fn a_relative_href_resolves_against_the_feed_it_came_from() {
        let feed = parse(FB_MAIN, FB_MAIN_BASE);
        let publication = &feed.publications[0];
        let cover = publication.cover().expect("a cover");
        match &cover.href {
            ImageSource::Url(url) => {
                assert_eq!(
                    url,
                    "https://feedbooks.example/opds1/feedbooks-test-catalog/larger.jpg"
                );
            }
            ImageSource::Inline { .. } => panic!("expected a URL"),
        }
    }

    #[test]
    fn pagination_follows_next_and_previous() {
        let main = parse(FB_MAIN, FB_MAIN_BASE);
        let next = main.next().expect("a next link");
        assert_eq!(
            next.href,
            "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/page2.xml"
        );

        let page2_base =
            "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/page2.xml";
        let page2 = parse(FB_PAGE2, page2_base);
        let previous = page2.previous().expect("a previous link");
        assert_eq!(
            previous.href,
            "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/main.xml"
        );
    }

    /// `feedbooks-test-catalog/acquisition/main.xml` writes every one of its
    /// own acquisition links as plain `http://www.feedbooks.com/...` — a
    /// 2012-era catalog captured as it actually answers today, not tidied up
    /// for the fixture. That makes it the wrong catalog to read a buy link's
    /// price or a borrow link's indirect acquisition out of even though it
    /// has entries named exactly that: every one of those links is dropped
    /// by the `https`-only rule before this crate ever gets to their
    /// children. `spec-facets.xml`, written from the specification's own
    /// examples with relative (and so `https`-safe) hrefs, is what actually
    /// exercises price and indirect-acquisition parsing; this test instead
    /// pins the safety behaviour, which is the more important thing main.xml
    /// actually proves.
    #[test]
    fn an_insecure_buy_link_is_dropped_even_though_it_carries_a_price() {
        let feed = parse(FB_MAIN, FB_MAIN_BASE);
        let publication = feed
            .publications
            .iter()
            .find(|p| p.title == "Acquisition: Buy")
            .expect("the buy entry is still a publication");
        assert!(
            publication
                .acquisition
                .iter()
                .all(|a| a.kind != AcquisitionKind::Buy),
            "an http:// buy link must not survive"
        );
    }

    #[test]
    fn a_buy_link_carries_its_price_and_its_indirect_acquisition_type() {
        let feed = parse(SPEC_FACETS, "https://example.org/facets.xml");
        let publication = feed
            .publications
            .iter()
            .find(|p| p.title == "A Book With Every Acquisition Kind")
            .expect("the spec example entry");
        let buy = publication
            .acquisition
            .iter()
            .find(|a| a.kind == AcquisitionKind::Buy)
            .expect("a buy acquisition");
        let price = buy.price.as_ref().expect("a price");
        assert_eq!(price.currency.as_deref(), Some("USD"));
        assert_eq!(price.value, "4.99");
        assert_eq!(buy.indirect.len(), 1);
        assert_eq!(
            buy.indirect[0].media_type.as_deref(),
            Some("application/epub+zip")
        );
    }

    #[test]
    fn a_borrow_link_carries_indirect_acquisition_nested_two_deep() {
        let feed = parse(SPEC_FACETS, "https://example.org/facets.xml");
        let publication = feed
            .publications
            .iter()
            .find(|p| p.title == "A Book With Every Acquisition Kind")
            .expect("the spec example entry");
        let borrow = publication
            .acquisition
            .iter()
            .find(|a| a.kind == AcquisitionKind::Borrow)
            .expect("a borrow acquisition");
        assert_eq!(borrow.indirect.len(), 1);
        assert_eq!(
            borrow.indirect[0].media_type.as_deref(),
            Some("application/atom+xml;type=entry;profile=opds-catalog")
        );
        assert_eq!(borrow.indirect[0].indirect.len(), 1);
        assert_eq!(
            borrow.indirect[0].indirect[0].media_type.as_deref(),
            Some("application/epub+zip")
        );
    }

    #[test]
    fn an_entry_marked_unavailable_says_so_rather_than_offering_a_download() {
        let feed = parse(SPEC_FACETS, "https://example.org/facets.xml");
        let publication = feed
            .publications
            .iter()
            .find(|p| p.title == "A Book That Is Not Available")
            .expect("the unavailable entry");
        let borrow = &publication.acquisition[0];
        assert!(!borrow.available);
        assert_eq!(publication.best_acquisition(), None);
    }

    #[test]
    fn entries_are_grouped_by_their_group_relation() {
        let feed = parse(
            FB_BLOCKS,
            "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/blocks.xml",
        );
        let sherlock = feed
            .groups
            .iter()
            .find(|g| g.title == "Sherlock Holmes")
            .expect("a Sherlock Holmes group");
        assert_eq!(sherlock.publications.len(), 9);
        let westerns = feed
            .groups
            .iter()
            .find(|g| g.title == "Westerns")
            .expect("a Westerns group");
        assert_eq!(westerns.publications.len(), 3);
        // Grouped entries still show up in the flat list too.
        assert_eq!(feed.publications.len(), 12);
    }

    #[test]
    fn the_opensearch_description_yields_a_template_that_a_query_can_be_put_into() {
        let base = "https://feedbooks.example/opds1/feedbooks-test-catalog/opensearch.xml";
        let template = parse_opensearch(FB_OPENSEARCH.as_bytes(), base).expect("a template");
        let url = template.expand("pride & prejudice");
        assert_eq!(
            url,
            "https://feedbooks.example/opds1/feedbooks-test-catalog/search/results.xml#pride%20%26%20prejudice"
        );
    }

    #[test]
    fn a_search_term_cannot_add_parameters_of_its_own_to_the_url() {
        let template = SearchTemplate {
            template: "https://example.org/opds/search?q={searchTerms}".to_owned(),
        };
        let url = template.expand("x&evil=1");
        assert!(url.contains("x%26evil%3D1"));
        assert!(!url.contains("&evil=1"));
    }

    /// The exact three `Url` elements
    /// `https://www.gutenberg.org/catalog/osd-books.xml` answers with, probed
    /// from the live catalog: a web page, the OPDS feed, and a typeahead
    /// suggestions endpoint sharing the same `{searchTerms}` grammar. Only the
    /// feed is a catalog.
    const GUTENBERG_OPENSEARCH: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
  <ShortName>gutenberg.org</ShortName>
  <Url type="text/html" template="http://www.gutenberg.org/ebooks/search/?query={searchTerms}"/>
  <Url type="application/atom+xml" template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
  <Url type="application/x-suggestions+json" rel="suggestions" template="http://www.gutenberg.org/ebooks/suggest/?query={searchTerms}"/>
</OpenSearchDescription>"#;

    #[test]
    fn a_web_page_and_a_typeahead_template_are_never_chosen_over_the_feed() {
        let template = parse_opensearch(
            GUTENBERG_OPENSEARCH.as_bytes(),
            "https://www.gutenberg.org/catalog/osd-books.xml",
        )
        .expect("the atom template");
        let url = template.expand("dickens");
        assert_eq!(
            url,
            "https://m.gutenberg.org/ebooks/search.opds/?query=dickens"
        );
    }

    #[test]
    fn an_http_search_template_is_upgraded_to_https_rather_than_refused() {
        let template = parse_opensearch(
            GUTENBERG_OPENSEARCH.as_bytes(),
            "https://www.gutenberg.org/catalog/osd-books.xml",
        )
        .expect("a template, not a refusal");
        assert!(template.expand("dickens").starts_with("https://"));
    }

    /// The chosen template's host (`m.gutenberg.org`) legitimately differs
    /// from the catalog's own host (`www.gutenberg.org`) — the description
    /// document is declaring where its search lives, which is a different
    /// question from whether a `next` page-turn link should be followed off
    /// the host the reader chose. This crate answers the first question by
    /// simply trusting the description document (same as every OPDS client
    /// does); [`crate::same_origin`] is what an application uses to answer
    /// the second, and the two must never be conflated into one rule.
    #[test]
    fn a_search_templates_host_is_not_held_to_the_same_origin_rule_a_paging_link_is() {
        let template = parse_opensearch(
            GUTENBERG_OPENSEARCH.as_bytes(),
            "https://www.gutenberg.org/catalog/osd-books.xml",
        )
        .expect("a template");
        let url = template.expand("dickens");
        assert!(
            !crate::same_origin("https://www.gutenberg.org/catalog/osd-books.xml", &url),
            "the fixture is only interesting if the hosts genuinely differ"
        );

        // A `next` link, by contrast, is exactly what `same_origin` exists to
        // gate — this crate hands the tool to the application rather than
        // enforcing it itself, since refusing to page is a product decision.
        let feed = parse(FB_MAIN, FB_MAIN_BASE);
        let next = feed.next().expect("a next link");
        assert!(crate::same_origin(FB_MAIN_BASE, &next.href));
    }

    #[test]
    fn facets_carry_their_group_active_state_and_count() {
        let feed = parse(SPEC_FACETS, "https://example.org/facets.xml");
        let language = feed
            .facets
            .iter()
            .find(|g| g.title == "Language")
            .expect("a Language facet group");
        assert_eq!(language.facets.len(), 2);
        let french = language
            .facets
            .iter()
            .find(|f| f.title == "French")
            .expect("French facet");
        assert!(french.active);
        assert_eq!(french.count, Some(200));
        let english = language
            .facets
            .iter()
            .find(|f| f.title == "English")
            .expect("English facet");
        assert!(!english.active);
        assert_eq!(english.count, Some(600));
    }

    #[test]
    fn a_link_that_is_not_https_never_becomes_something_to_fetch() {
        let feed = parse(HOSTILE, "https://catalog.example/hostile.xml");
        let clear_text = feed
            .publications
            .iter()
            .find(|p| p.title.contains("Clear Text"))
            .expect("the clear text entry");
        assert!(clear_text.acquisition.is_empty());

        let script = feed
            .publications
            .iter()
            .find(|p| p.title.contains("A Script"))
            .expect("the javascript entry");
        assert!(script.acquisition.is_empty());
        assert!(script.images.is_empty());

        let file = feed
            .publications
            .iter()
            .find(|p| p.title.contains("Local File"))
            .expect("the file entry");
        assert!(file.acquisition.is_empty());
    }

    #[test]
    fn a_data_uri_that_is_not_an_image_the_device_decodes_is_dropped() {
        let feed = parse(HOSTILE, "https://catalog.example/hostile.xml");
        let not_image = feed
            .publications
            .iter()
            .find(|p| p.title.contains("Not An Image"))
            .expect("the non-image data uri entry");
        assert!(not_image.images.is_empty());
        // The acquisition link on the same entry is a real https epub, so it
        // still shows up: a dropped image never takes the whole entry with it.
        assert!(!not_image.acquisition.is_empty());
    }

    #[test]
    fn a_title_containing_markup_arrives_as_text() {
        let feed = parse(HOSTILE, "https://catalog.example/hostile.xml");
        let markup = feed
            .publications
            .iter()
            .find(|p| p.title.contains("A Title With Markup In It"))
            .expect("the markup entry");
        assert!(markup.title.contains("<script>alert(1)</script>"));
        let summary = markup.summary.as_deref().unwrap_or_default();
        assert!(!summary.contains("<img"));
        assert!(!summary.to_lowercase().contains("onerror"));
    }

    #[test]
    fn a_feed_nested_past_the_depth_cap_stops_rather_than_growing() {
        let source = "<a>".repeat(10_000);
        let feed = parse(&source, "https://example.org/deep.xml");
        assert!(feed.title.is_none());
        assert!(feed.publications.is_empty());
    }

    #[test]
    fn one_malformed_entry_does_not_discard_the_entries_around_it() {
        let feed = parse(HOSTILE, "https://catalog.example/hostile.xml");
        // The document is truncated mid-element inside the last entry; the
        // entries that arrived complete before it are still there.
        assert!(feed
            .publications
            .iter()
            .any(|p| p.title.contains("Clear Text")));
        assert!(feed
            .publications
            .iter()
            .any(|p| p.title.contains("A Title With Markup In It")));
    }

    #[test]
    fn a_bare_entry_document_reads_as_one_publication() {
        const ENTRY: &str =
            include_str!("../tests/fixtures/opds1/feedbooks-test-catalog/acquisition/entry.xml");
        let feed = parse(
            ENTRY,
            "https://feedbooks.example/opds1/feedbooks-test-catalog/acquisition/entry.xml",
        );
        assert_eq!(feed.publications.len(), 1);
        assert_eq!(feed.publications[0].title, "Full entry view");
    }
}
