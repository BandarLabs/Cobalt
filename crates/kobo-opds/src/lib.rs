#![forbid(unsafe_code)]

//! A reader for OPDS catalogs — the format nearly every ebook library on the
//! open web answers in, from Project Gutenberg to Standard Ebooks to every
//! Calibre server and every library running Library Simplified.
//!
//! There are two versions of OPDS in the world at once, and a client that
//! only reads one of them has a deadline or a hole. **OPDS 1.2** is an Atom
//! feed with a small vocabulary of extensions (`opds:price`,
//! `opds:indirectAcquisition`, `dcterms:language`, and the rest); it is what
//! nearly everything serves today, including Project Gutenberg, which expects
//! to retire it in 2027. **OPDS 2.0** is a JSON document shaped like a Readium
//! Web Publication Manifest; it is what the specification calls current, and
//! what Gutenberg is testing as a replacement. Both are read here into the
//! same [`Feed`], so the rest of an application never has to ask which kind
//! of catalog it is looking at — see [`atom`] for the 1.2 reader and [`json`]
//! for the 2.0 one.
//!
//! # Why this crate has no I/O in it
//!
//! Because the interesting inputs are the ones a real catalog actually sends,
//! and the only way to pin those down for good is to capture them once and
//! replay them forever. This crate takes bytes and a base URL and gives back
//! a [`Feed`] or a [`Fault`]; it never opens a socket, never knows what a
//! `Task` is, and never decides whether to fetch a `next` link. That is what
//! lets its conformance suite run against the vendored fixtures under
//! `tests/fixtures` — captures of Project Gutenberg, Standard Ebooks, and the
//! OPDS 1.x and 2.0 community test catalogs — with no network reachable from
//! CI at all, and it is what makes the parser safe to fuzz: nothing it does
//! has a side effect further away than the [`Feed`] it returns.
//!
//! # Never guess
//!
//! A field that is absent from a catalog is [`None`], not a default a caller
//! might mistake for something the server said. A malformed entry is skipped,
//! not repaired — but one bad entry never discards the feed around it, on the
//! same reasoning [`kobo_xml`] documents for its own scanner: a feed
//! truncated by a proxy is a far more common shape than one that is subtly
//! wrong, and most of a catalog is a more useful answer than none of it.
//!
//! # Safety
//!
//! Every href this crate hands back has already been resolved against `base`
//! per RFC 3986 and checked for `https`; see [`url`] for the detail and
//! `tests/fixtures/opds1/hostile.xml` for the catalog every one of those
//! rules is tested against. A catalog is a stranger, and following one of its
//! links unchecked is how a redirected catalog sends a device somewhere it
//! was never pointed.

mod atom;
mod json;
mod url;

pub use atom::{parse_opensearch, SearchTemplate};
pub use json::expand_search;
pub use url::{is_https, safe_href, same_origin};

use std::fmt;

/// The most entries — navigation items and publications together — kept from
/// one feed.
///
/// A shelf on a six-row screen never shows more than a few dozen at once, and
/// deep paging is what `next` links exist for. This is deep enough that no
/// legitimate single response gets truncated (the vendored catalogs top out
/// well under a hundred entries per page) and shallow enough that a crawlable
/// feed advertising a whole library in one document cannot make this crate's
/// memory grow past what a device with 512 MiB and no swap can spare for a
/// catalog page.
pub const MAX_ENTRIES: usize = 500;

/// The most bytes kept from any single text field: a title, a summary, a
/// content block, rights text.
///
/// Nothing this crate hands back is ever shown past a few paragraphs on a
/// shelf or a book's detail screen. Capping while the field is being read,
/// rather than after it has been fully accumulated, means a hostile or
/// merely enormous `<summary>` costs a bounded copy instead of an unbounded
/// one — the same reasoning `kobo-xml`'s [`kobo_xml::MAX_DEPTH`] documents
/// for element nesting, applied to the size of a single run of text instead
/// of the shape of the document around it.
pub(crate) const MAX_TEXT_FIELD_BYTES: usize = 32 * 1024;

/// The most categories, images, acquisition links or kept links read from a
/// single entry.
///
/// The XML scanner bounds how deep a document can nest and how many entries a
/// feed can contribute, but nothing bounds how many *siblings* one entry can
/// have — a hostile `<entry>` could carry ten thousand `<category>` elements
/// at a single depth without ever tripping [`kobo_xml::MAX_DEPTH`]. The
/// richest real entry in the vendored fixtures (Gutenberg's) carries under
/// twenty related links; this is generous past any real catalog and still
/// bounded.
pub(crate) const MAX_PER_ENTRY: usize = 64;

/// The `Accept` header a client should send.
///
/// Both media types are offered because both versions are in the world at
/// once; the JSON one is listed first because the specification calls it
/// current. Neither preference is load-bearing to this crate — [`parse`]
/// decides the version by sniffing the body's first non-space byte, because
/// servers ignore `Accept` constantly and a client that trusted it would fail
/// on catalogs that work fine.
pub const ACCEPT: &str = "application/opds+json, application/atom+xml;profile=opds-catalog;q=0.9";

/// Which of the two protocol versions a [`Feed`] was read from.
///
/// Carried for diagnostics only — a log line, a "why did this look odd"
/// question — never as something the rest of an application branches on. If
/// a caller finds itself matching on this, the model underneath it is
/// missing a field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Version {
    /// OPDS 1.2, an Atom feed.
    #[default]
    Atom,
    /// OPDS 2.0, a JSON document.
    Json,
}

/// Why [`parse`] could not produce a [`Feed`] at all.
///
/// This is distinct from a feed with nothing in it (an empty shelf, a
/// publisher between releases, is not an error) and distinct from a
/// malformed *entry* (which is simply skipped). A [`Fault`] means the bytes
/// were not recognisably OPDS in either version.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    /// The body's first non-whitespace byte was neither `{` nor `<`.
    NotAFeed,
    /// The body looked like JSON but was not well formed.
    Json(kobo_json::ParseError),
}

impl fmt::Display for Fault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAFeed => formatter.write_str("not an OPDS catalog"),
            Self::Json(error) => write!(formatter, "invalid OPDS 2.0 JSON: {error}"),
        }
    }
}

impl std::error::Error for Fault {}

/// A relation a [`Link`] or a [`Navigation`] entry carries.
///
/// Acquisition relations (`buy`, `borrow`, ...) and image relations become
/// [`AcquisitionKind`] and [`Image`] instead of a `Relation`, because those
/// carry structured data of their own; this enum is for the relations that
/// are just "this is a link, and here is what kind of link." A single Atom
/// `rel` attribute may hold several space-separated values, and an OPDS 2.0
/// `rel` may be a JSON array, so a [`Link`] keeps every relation it matched
/// rather than the first.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Relation {
    SelfLink,
    Start,
    Next,
    Previous,
    First,
    Last,
    Alternate,
    Related,
    Search,
    Subsection,
    /// The feed's icon, read from Atom's `<icon>` element rather than a
    /// `<link>`, but modelled the same way so a caller looking for "is there
    /// an icon" does not need a second field to check.
    Icon,
    /// `http://opds-spec.org/sort/*` — the suffix is kept because a catalog
    /// can offer several sort orders and a caller needs to tell them apart.
    Sort(String),
    Featured,
    Recommended,
    Crawlable,
    /// Anything else. Kept rather than dropped, because a caller looking for
    /// a catalog-specific relation (Standard Ebooks' `license`, say) should
    /// not lose it to a vocabulary this crate did not anticipate — only the
    /// relations this crate itself acts on (group, facet, the acquisition
    /// and image kinds) are ever consumed instead of kept.
    Other(String),
}

/// A link kept for what it points to, not for what it is: pagination, search,
/// the feed's own address, an icon, a sort order, a catalog extension.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub rel: Vec<Relation>,
    pub href: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
}

impl Link {
    fn matches(&self, relation: &Relation) -> bool {
        self.rel.iter().any(|candidate| candidate == relation)
    }
}

/// Which kind of feed a [`Navigation`] entry points to, when the catalog
/// states it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedKind {
    Navigation,
    Acquisition,
}

/// Somewhere else to go: a shelf, a search, a subject — anything that is not
/// itself a book.
#[derive(Clone, Debug, PartialEq)]
pub struct Navigation {
    pub title: String,
    pub href: String,
    pub summary: Option<String>,
    /// Read from the `type` parameter's `kind=navigation` or
    /// `kind=acquisition` (OPDS 1.2 §7.1.3), when the catalog states it.
    /// `None` when it does not, which the application resolves by fetching
    /// and looking rather than this crate guessing.
    pub kind: Option<FeedKind>,
    pub rel: Option<Relation>,
    pub thumbnail: Option<Image>,
}

/// One way to acquire a [`Publication`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquisitionKind {
    Generic,
    OpenAccess,
    Borrow,
    Buy,
    Sample,
    Subscribe,
}

/// A price, kept as the text it was written in.
///
/// OPDS 2.0's `properties.price.value` is a JSON number, and a JSON number is
/// an `f64` in [`kobo_json`] — but rendering that `f64` back out with a fixed
/// number of decimal places would round `4.999` into a price nobody wrote.
/// `value` is instead the shortest decimal string that reads back as the same
/// `f64` `kobo_json` parsed (see [`kobo_json::Value::to_json`]'s own
/// guarantee), which for the two- and three-digit prices every real catalog
/// writes is exactly the string the catalog sent.
#[derive(Clone, Debug, PartialEq)]
pub struct Price {
    pub currency: Option<String>,
    pub value: String,
}

/// One level of "if you can't get it directly, get it through this instead."
///
/// OPDS 1.2's `opds:indirectAcquisition` nests (a borrow link's indirect
/// acquisition can itself have an indirect acquisition, for a library that
/// hands out an ACSM that hands out an EPUB); OPDS 2.0's
/// `properties.indirectAcquisition` calls the same nesting `child`. Both
/// become this one shape.
#[derive(Clone, Debug, PartialEq)]
pub struct Indirect {
    pub media_type: Option<String>,
    pub indirect: Vec<Indirect>,
}

/// One way to obtain a [`Publication`]'s content.
#[derive(Clone, Debug, PartialEq)]
pub struct Acquisition {
    pub kind: AcquisitionKind,
    pub href: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub length: Option<u64>,
    pub price: Option<Price>,
    pub indirect: Vec<Indirect>,
    /// `false` when `opds:unavailable` (or, in 2.0, `properties.availability`
    /// stating the same) marks this link as not currently obtainable — a
    /// library book every copy of which is checked out, most commonly.
    pub available: bool,
}

/// Where an [`Image`]'s bytes come from.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageSource {
    Url(String),
    /// A `data:` URI, already decoded — never fetched, per the safety rule in
    /// [`url::decode_data_image`]. Gutenberg's navigation thumbnails are
    /// these: 22×22 base64 PNGs inlined directly into the feed.
    Inline {
        media_type: String,
        bytes: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub href: ImageSource,
    pub media_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Category {
    pub term: String,
    pub label: Option<String>,
    pub scheme: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub name: String,
    pub position: Option<f64>,
}

/// Something to read.
#[derive(Clone, Debug, PartialEq)]
pub struct Publication {
    pub title: String,
    pub identifier: Option<String>,
    pub authors: Vec<String>,
    /// The richer of `content`/`summary` (Atom) or `description` (JSON) — see
    /// [`atom`]'s module documentation for how the two are compared in 1.2.
    pub summary: Option<String>,
    pub language: Option<String>,
    pub issued: Option<String>,
    pub published: Option<String>,
    pub updated: Option<String>,
    pub publisher: Option<String>,
    pub rights: Option<String>,
    pub extent: Option<String>,
    pub categories: Vec<Category>,
    pub series: Option<Series>,
    pub images: Vec<Image>,
    pub acquisition: Vec<Acquisition>,
    /// `alternate`, `related`, `self`, `subsection` links that were not
    /// consumed as something more specific.
    pub links: Vec<Link>,
}

/// Format ranking for [`Publication::best_acquisition`]: higher reads better.
///
/// `kobo-doc` reads EPUB (and Kobo's `kepub` variant, which is a valid EPUB
/// with Kobo's own spans stitched in). Plain text is readable too, but only
/// when there is nothing better — it throws away every heading, every
/// italic, the table of contents, everything an EPUB carries — so it ranks
/// below both. Anything else (`azw3`, Amazon's `kf8`, classic `mobi`) is not
/// a format this device can open at all: offering it as "the" acquisition
/// would be a download that cannot be read, which is worse than telling the
/// reader there is nothing to download, so it is not ranked — it is refused.
fn format_rank(media_type: Option<&str>) -> u8 {
    let base = media_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    if base.eq_ignore_ascii_case("application/epub+zip") {
        3
    } else if base.eq_ignore_ascii_case("application/kepub+zip") {
        2
    } else {
        u8::from(base.eq_ignore_ascii_case("text/plain"))
    }
}

impl Publication {
    /// The acquisition link this crate recommends actually reading, or
    /// nothing when none of the offered links are.
    ///
    /// Never a `buy`, `borrow` or `subscribe` link — those are offers to
    /// *obtain* the book, not something to hand to a reader, and returning
    /// one as though it were a plain download would start a purchase or a
    /// library hold the reader never asked for. Never an unavailable link,
    /// and never a format this device cannot open (see [`format_rank`]).
    /// Among what is left, the highest-ranked format wins outright — an EPUB
    /// beats a plain-text link regardless of which acquisition kind offered
    /// it — and only when two candidates share a format does the acquisition
    /// kind break the tie, preferring a generic or open-access link over a
    /// sample. Ties within that are resolved in the catalog's own order,
    /// since the first format a catalog lists is usually the one it means as
    /// the default.
    #[must_use]
    pub fn best_acquisition(&self) -> Option<&Acquisition> {
        let mut best: Option<(u8, u8, &Acquisition)> = None;
        for acquisition in &self.acquisition {
            if !acquisition.available {
                continue;
            }
            if !matches!(
                acquisition.kind,
                AcquisitionKind::Generic | AcquisitionKind::OpenAccess | AcquisitionKind::Sample
            ) {
                continue;
            }
            let format = format_rank(acquisition.media_type.as_deref());
            if format == 0 {
                continue;
            }
            let kind_rank = u8::from(!matches!(acquisition.kind, AcquisitionKind::Sample));
            let candidate = (format, kind_rank);
            if best.is_none_or(|(best_format, best_kind, _)| candidate > (best_format, best_kind)) {
                best = Some((format, kind_rank, acquisition));
            }
        }
        best.map(|(_, _, acquisition)| acquisition)
    }

    /// The image to show as the cover: a non-thumbnail image when the
    /// catalog offered one, falling back to a thumbnail when it did not.
    ///
    /// Standard Ebooks' new-releases feed offers only a `media:thumbnail` and
    /// no `http://opds-spec.org/image` at all — this is the fallback that
    /// makes that catalog show a cover instead of a blank shelf slot.
    #[must_use]
    pub fn cover(&self) -> Option<&Image> {
        self.images
            .iter()
            .find(|image| !image.thumbnail)
            .or_else(|| self.images.iter().find(|image| image.thumbnail))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub title: String,
    pub href: String,
    pub active: bool,
    pub count: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FacetGroup {
    pub title: String,
    pub facets: Vec<Facet>,
}

/// One of OPDS 2.0's several collections shown together in one feed, or OPDS
/// 1.2's `opds:group` pseudo-grouping of entries within a single flat feed.
///
/// Entries that belong to a group are kept both here and in the feed's own
/// [`Feed::navigation`]/[`Feed::publications`] — that duplication is not a
/// bug, it is what the OPDS 2.0 test catalog's own `home.json` does (Moby-Dick
/// appears in the "English Classics" group and in the feed's flat
/// `publications` list), and mirroring it rather than picking one keeps a
/// caller that only wants "everything on this page" and a caller that wants
/// "everything organised into shelves" both correct.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub title: String,
    pub href: Option<String>,
    pub navigation: Vec<Navigation>,
    pub publications: Vec<Publication>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub total: Option<u32>,
    pub per_page: Option<u32>,
    pub start_index: Option<u32>,
    pub current_page: Option<u32>,
}

/// One OPDS catalog page, whichever version it was written in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub updated: Option<String>,
    pub links: Vec<Link>,
    pub navigation: Vec<Navigation>,
    pub publications: Vec<Publication>,
    pub facets: Vec<FacetGroup>,
    pub groups: Vec<Group>,
    pub pagination: Pagination,
    pub version: Version,
}

impl Feed {
    fn link(&self, relation: &Relation) -> Option<&Link> {
        self.links.iter().find(|link| link.matches(relation))
    }

    #[must_use]
    pub fn next(&self) -> Option<&Link> {
        self.link(&Relation::Next)
    }

    #[must_use]
    pub fn previous(&self) -> Option<&Link> {
        self.link(&Relation::Previous)
    }

    #[must_use]
    pub fn first(&self) -> Option<&Link> {
        self.link(&Relation::First)
    }

    #[must_use]
    pub fn last(&self) -> Option<&Link> {
        self.link(&Relation::Last)
    }

    #[must_use]
    pub fn start(&self) -> Option<&Link> {
        self.link(&Relation::Start)
    }

    #[must_use]
    pub fn search(&self) -> Option<&Link> {
        self.link(&Relation::Search)
    }

    /// A feed with no publications is a navigation feed: somewhere to keep
    /// browsing, not something to shelve. This is the fallback OPDS 1.2
    /// §7.1.3 describes for when a feed does not state its own kind, and
    /// since every catalog in the fixtures either states it truthfully or
    /// not at all, it is the only rule this crate needs — a feed's
    /// [`Navigation`] entries carry their own stated `kind` for the feeds
    /// *they* point to, in [`Navigation::kind`].
    #[must_use]
    pub fn is_navigation(&self) -> bool {
        self.publications.is_empty()
    }

    #[must_use]
    pub fn is_acquisition(&self) -> bool {
        !self.publications.is_empty()
    }
}

/// A well-known relation kept as a [`Link`] rather than consumed into
/// something more specific (an acquisition, an image, a facet, a group).
/// Shared between [`atom`] (which matches this against Atom's
/// space-separated `rel` attribute tokens) and [`json`] (which matches it
/// against OPDS 2.0's `rel`, already normalised to a list by
/// `json::rel_tokens`), so the two versions can never quietly drift apart on
/// which relations are worth keeping.
pub(crate) fn kept_relation(token: &str) -> Option<Relation> {
    match token {
        "self" => Some(Relation::SelfLink),
        "start" => Some(Relation::Start),
        "next" => Some(Relation::Next),
        "previous" | "prev" => Some(Relation::Previous),
        "first" => Some(Relation::First),
        "last" => Some(Relation::Last),
        "alternate" => Some(Relation::Alternate),
        "related" => Some(Relation::Related),
        "search" => Some(Relation::Search),
        "subsection" => Some(Relation::Subsection),
        "http://opds-spec.org/featured" => Some(Relation::Featured),
        "http://opds-spec.org/recommended" => Some(Relation::Recommended),
        "http://opds-spec.org/crawlable" => Some(Relation::Crawlable),
        _ => token
            .strip_prefix("http://opds-spec.org/sort/")
            .map(|kind| Relation::Sort(kind.to_owned())),
    }
}

/// Percent-encodes everything outside RFC 3986's unreserved set.
///
/// Used everywhere a reader-supplied search term is spliced into a URL — an
/// `OpenSearch` `{searchTerms}` substitution in [`atom`], an RFC 6570
/// `{?query}` expansion in [`json::expand_search`] — so that a query
/// containing `&` or `=` cannot add a parameter of its own to the request.
pub(crate) fn percent_encode(text: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                // `write!` into the `String` directly rather than building a
                // throwaway formatted string per byte and appending it.
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Maps an acquisition relation — either OPDS 1.2's URI or OPDS 2.0's
/// simplified name (OPDS 2.0 §5.3) — to the [`AcquisitionKind`] it means.
///
/// Both vocabularies are accepted in both versions' readers: nothing stops a
/// 2.0 catalog from writing the 1.x URI (the specification's own examples do
/// this in places), and rejecting it on a technicality would be exactly the
/// kind of guess this crate exists not to make.
pub(crate) fn acquisition_kind(rel: &str) -> Option<AcquisitionKind> {
    match rel {
        "http://opds-spec.org/acquisition" | "acquisition" => Some(AcquisitionKind::Generic),
        "http://opds-spec.org/acquisition/open-access" | "download" => {
            Some(AcquisitionKind::OpenAccess)
        }
        "http://opds-spec.org/acquisition/borrow" | "borrow" => Some(AcquisitionKind::Borrow),
        "http://opds-spec.org/acquisition/buy" | "buy" => Some(AcquisitionKind::Buy),
        "http://opds-spec.org/acquisition/sample"
        | "http://opds-spec.org/acquisition/preview"
        | "preview" => Some(AcquisitionKind::Sample),
        "http://opds-spec.org/acquisition/subscribe" | "subscribe" => {
            Some(AcquisitionKind::Subscribe)
        }
        _ => None,
    }
}

/// Reads a catalog page, in whichever of the two OPDS versions it happens to
/// be written in.
///
/// `base` is the URL the bytes were fetched from; every relative href in the
/// document resolves against it, and both vendored conformance catalogs use
/// relative hrefs throughout, so nothing in a real catalog reads correctly
/// without it.
///
/// The version is decided by sniffing the body, never by trusting a
/// `Content-Type` the caller may have received: after skipping a byte-order
/// mark and leading whitespace, `{` means OPDS 2.0 and `<` means OPDS 1.2.
/// Servers ignore `Accept` and mislabel responses constantly; a client that
/// trusted the declared type would fail on catalogs that work fine.
///
/// # Errors
///
/// Returns [`Fault::NotAFeed`] when the body is neither, and
/// [`Fault::Json`] when it looks like JSON but is not well formed. A
/// malformed *entry* inside an otherwise valid feed is not an error at this
/// level — it is simply missing from the result.
pub fn parse(bytes: &[u8], base: &str) -> Result<Feed, Fault> {
    let text = String::from_utf8_lossy(bytes);
    let body = text.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r']);
    if body.starts_with('{') {
        json::parse(body, base).map_err(Fault::Json)
    } else if body.starts_with('<') {
        Ok(atom::parse(body, base))
    } else {
        Err(Fault::NotAFeed)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Fault, Version};

    const ATOM_FEED: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>T</title></feed>"#;
    const JSON_FEED: &str = r#"{"metadata":{"title":"T"}}"#;

    /// The entry point's whole job, pinned directly: a body is read by what
    /// its first non-whitespace byte is, never by what a caller believes the
    /// `Content-Type` said — both Standard Ebooks and Project Gutenberg's
    /// forthcoming JSON feed answer content negotiation inconsistently
    /// enough that trusting the header is how a client that "worked in
    /// testing" fails in the field.
    #[test]
    fn a_json_body_is_read_as_2_0_and_an_xml_body_as_1_x_whatever_the_server_said_the_type_was() {
        let json = parse(JSON_FEED.as_bytes(), "https://example.org/catalog").expect("json");
        assert_eq!(json.version, Version::Json);
        assert_eq!(json.title.as_deref(), Some("T"));

        let atom = parse(ATOM_FEED.as_bytes(), "https://example.org/catalog").expect("atom");
        assert_eq!(atom.version, Version::Atom);
        assert_eq!(atom.title.as_deref(), Some("T"));
    }

    #[test]
    fn a_leading_byte_order_mark_and_whitespace_do_not_defeat_sniffing() {
        let with_bom = format!("\u{feff}  \n\t{JSON_FEED}");
        let feed = parse(with_bom.as_bytes(), "https://example.org/catalog").expect("json");
        assert_eq!(feed.version, Version::Json);

        let with_bom = format!("\u{feff}  \n\t{ATOM_FEED}");
        let feed = parse(with_bom.as_bytes(), "https://example.org/catalog").expect("atom");
        assert_eq!(feed.version, Version::Atom);
    }

    #[test]
    fn bytes_that_are_neither_json_nor_xml_are_refused_as_not_a_feed() {
        let error = parse(b"just some text", "https://example.org/catalog").unwrap_err();
        assert_eq!(error, Fault::NotAFeed);
        let error = parse(b"", "https://example.org/catalog").unwrap_err();
        assert_eq!(error, Fault::NotAFeed);
    }

    #[test]
    fn json_that_is_not_well_formed_is_reported_rather_than_read_as_atom() {
        let error = parse(b"{ this is not json", "https://example.org/catalog").unwrap_err();
        assert!(matches!(error, Fault::Json(_)));
        assert!(error.to_string().contains("invalid OPDS 2.0 JSON"));
    }

    #[test]
    fn the_accept_header_lists_both_media_types_with_json_preferred() {
        assert!(super::ACCEPT.starts_with("application/opds+json"));
        assert!(super::ACCEPT.contains("application/atom+xml"));
    }
}
