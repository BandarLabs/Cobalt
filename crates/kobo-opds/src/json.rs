//! OPDS 2.0: a JSON document shaped like a Readium Web Publication Manifest,
//! read with [`kobo_json`].
//!
//! The two versions diverge in more than syntax, and this module exists to
//! absorb every one of those divergences so [`crate::Feed`] never shows a
//! caller which version answered. Three shapes recur throughout OPDS 2.0's
//! own examples and are handled the same way everywhere they appear:
//!
//! - **A `rel` is a string or an array of strings** (`"rel": ["first",
//!   "previous"]` is in the specification's own examples), so [`rel_tokens`]
//!   always returns a list, never assuming which shape a given document used.
//! - **A contributor — `author`, and every other one — is a bare string, an
//!   object with a `name`, or an array of either.** [`contributors`] reads
//!   all three the same way, because Cobalt's own test catalog uses the
//!   object form and the specification's examples use the bare string form,
//!   and a reader that only handled one would fail on real data either way.
//! - **A localizable string (a title, most often) is a plain string or an
//!   object mapping language tag to string.** [`localized_text`] prefers
//!   `"en"` and otherwise takes whichever field arrived first, since this
//!   crate has no notion of the reader's locale to prefer instead.
//!
//! # Prices, kept exact
//!
//! `properties.price.value` is a JSON number, which [`kobo_json`] hands back
//! as an `f64`. Formatting that `f64` with a fixed number of decimal places
//! would turn `4.999` into a price nobody wrote; instead this uses `f64`'s
//! own `Display`, which — as [`kobo_json::Value::to_json`] documents — is the
//! shortest decimal that reads back as the same bits. For every price in the
//! fixtures, that shortest decimal is exactly the digits the catalog sent.

use crate::{
    acquisition_kind, kept_relation, Acquisition, Category, Facet, FacetGroup, Feed, Group, Image,
    ImageSource, Indirect, Link, Navigation, Pagination, Price, Publication, Relation, Series,
    Version, MAX_ENTRIES, MAX_PER_ENTRY,
};
use kobo_json::Value;

/// The most `child` levels read out of a nested `indirectAcquisition` array,
/// matching the sibling cap [`crate::atom`] applies to the same shape in
/// OPDS 1.2 — a hostile catalog gains nothing by nesting a thousand siblings
/// at one level instead of the handful any real indirect-acquisition chain
/// ever has.
const MAX_SIBLINGS: usize = 32;

pub(crate) fn parse(input: &str, base: &str) -> Result<Feed, kobo_json::ParseError> {
    let value = kobo_json::parse(input)?;
    let metadata = value.get("metadata");

    let mut feed = Feed {
        version: Version::Json,
        title: metadata
            .and_then(|m| m.get("title"))
            .and_then(localized_text),
        subtitle: metadata
            .and_then(|m| m.get("subtitle"))
            .and_then(localized_text),
        updated: metadata
            .and_then(|m| m.get("modified"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        pagination: Pagination {
            total: metadata.and_then(|m| as_u32(m.get("numberOfItems"))),
            per_page: metadata.and_then(|m| as_u32(m.get("itemsPerPage"))),
            start_index: None,
            current_page: metadata.and_then(|m| as_u32(m.get("currentPage"))),
        },
        ..Feed::default()
    };

    for link in array_field(&value, "links") {
        add_feed_link(&mut feed, base, link);
    }
    for item in array_field(&value, "navigation") {
        if let Some(navigation) = parse_navigation(base, item) {
            feed.navigation.push(navigation);
        }
    }
    for item in array_field(&value, "publications") {
        if let Some(publication) = parse_publication(base, item) {
            feed.publications.push(publication);
        }
    }
    for item in array_field(&value, "facets") {
        feed.facets.push(parse_facet_group(base, item));
    }
    for item in array_field(&value, "groups") {
        feed.groups.push(parse_group(base, item));
    }

    Ok(feed)
}

/// A top-level array field, capped and defaulting to empty rather than
/// making every caller repeat the same `and_then`/`unwrap_or_default` chain.
fn array_field<'a>(value: &'a Value, field: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(field)
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .take(MAX_ENTRIES)
}

fn as_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_i64)
        .and_then(|n| u32::try_from(n).ok())
}

/// `rel` as a list, whichever of a bare string or an array the document used.
fn rel_tokens(value: &Value) -> Vec<String> {
    match value.get("rel") {
        Some(Value::String(rel)) => vec![rel.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// A localizable string: a plain JSON string, or an object mapping a
/// language tag to a string, preferring `"en"` and otherwise taking
/// whichever field came first — this crate has no reader locale to prefer
/// instead, and *a* title is a better answer than none.
fn localized_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text.clone()),
        Value::Object(fields) => fields
            .iter()
            .find(|(tag, _)| tag == "en")
            .or_else(|| fields.first())
            .and_then(|(_, text)| text.as_str())
            .and_then(|text| non_empty(text.to_owned())),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
    }
}

fn non_empty(text: String) -> Option<String> {
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// A contributor: a bare string, an object with a `name`, or an array of
/// either — `author`, `translator`, `illustrator` and the rest of OPDS 2.0's
/// contributor roles all share this grammar (specification §3.1).
fn contributors(value: Option<&Value>) -> Vec<String> {
    fn one(value: &Value) -> Option<String> {
        match value {
            Value::String(name) => non_empty(name.clone()),
            Value::Object(_) => value.get("name").and_then(localized_text),
            _ => None,
        }
    }
    let Some(value) = value else {
        return Vec::new();
    };
    match value {
        Value::Array(items) => items.iter().filter_map(one).take(MAX_PER_ENTRY).collect(),
        other => one(other).into_iter().collect(),
    }
}

/// Resolves a link's `href` against `base`, accounting for OPDS 2.0's one
/// departure from plain URLs: a `templated: true` search link's href is an
/// RFC 6570 URI template such as `search{?query,title,author}`, and the `?`
/// and `{`/`}` inside that template are not URI-reference syntax — running
/// the whole string through ordinary reference resolution (which looks for a
/// literal `?` to mean "here starts the query string") would split
/// `search{?query,title,author}` into a path of `search{` and a query of
/// `query,title,author}`, silently producing nonsense. Splitting at the
/// first `{` first, resolving only the literal prefix, and reattaching the
/// template unresolved is what OPDS 2.0 §3.4.1 means by "resolve URI
/// Templates ... as per \[RFC6570\]": relative resolution operates on the
/// template's *fixed* part only.
fn resolve_href(base: &str, href: &str) -> Option<String> {
    let split_at = href.find('{').unwrap_or(href.len());
    let (prefix, template) = href.split_at(split_at);
    if prefix.is_empty() {
        // A template with nothing before its first expression, e.g. a bare
        // `{?query}` — there is no path segment to resolve, so the resolved
        // document itself stands in for it.
        return Some(format!("{base}{template}"));
    }
    let resolved_prefix = crate::url::safe_href(base, prefix)?;
    Some(format!("{resolved_prefix}{template}"))
}

fn build_image(base: &str, value: &Value) -> Option<Image> {
    let raw_href = value.get("href").and_then(Value::as_str)?;
    let media_type = value.get("type").and_then(Value::as_str).map(str::to_owned);
    let width = as_u32(value.get("width"));
    let height = as_u32(value.get("height"));
    let thumbnail = rel_tokens(value)
        .iter()
        .any(|rel| rel.contains("thumbnail"));

    if let Some((decoded_type, bytes)) = crate::url::decode_data_image(raw_href) {
        return Some(Image {
            href: ImageSource::Inline {
                media_type: decoded_type,
                bytes,
            },
            media_type,
            width,
            height,
            thumbnail,
        });
    }
    let href = crate::url::safe_href(base, raw_href)?;
    Some(Image {
        href: ImageSource::Url(href),
        media_type,
        width,
        height,
        thumbnail,
    })
}

fn categories(metadata: &Value) -> Vec<Category> {
    fn one(value: &Value) -> Option<Category> {
        match value {
            Value::String(text) => non_empty(text.clone()).map(|term| Category {
                term,
                label: None,
                scheme: None,
            }),
            Value::Object(_) => {
                let name = value.get("name").and_then(Value::as_str);
                let code = value.get("code").and_then(Value::as_str);
                let term = code.or(name)?.to_owned();
                Some(Category {
                    term,
                    label: name.map(str::to_owned),
                    scheme: value
                        .get("scheme")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
            }
            _ => None,
        }
    }
    match metadata.get("subject") {
        Some(Value::Array(items)) => items.iter().filter_map(one).take(MAX_PER_ENTRY).collect(),
        Some(other) => one(other).into_iter().collect(),
        None => Vec::new(),
    }
}

/// `metadata.belongsTo.series`: a string, or an object with a `name` and a
/// `position`. When a catalog lists several (`belongsTo.series` as an
/// array), the first is kept — the model holds one series, the same way a
/// physical shelf only has room to shows one, and picking arbitrarily among
/// several would be a guess this crate does not make on the application's
/// behalf.
fn series_of(metadata: &Value) -> Option<Series> {
    fn one(value: &Value) -> Option<Series> {
        match value {
            Value::String(name) => non_empty(name.clone()).map(|name| Series {
                name,
                position: None,
            }),
            Value::Object(_) => Some(Series {
                name: value.get("name").and_then(Value::as_str)?.to_owned(),
                position: value.get("position").and_then(Value::as_f64),
            }),
            _ => None,
        }
    }
    let series = metadata.get("belongsTo")?.get("series")?;
    match series {
        Value::Array(items) => items.first().and_then(one),
        other => one(other),
    }
}

fn price_of(properties: &Value) -> Option<Price> {
    let price = properties.get("price")?;
    let amount = price.get("value")?.as_f64()?;
    Some(Price {
        currency: price
            .get("currency")
            .and_then(Value::as_str)
            .map(str::to_owned),
        // The shortest decimal that reads back as this exact `f64` — see the
        // module documentation for why this, and not a fixed-precision
        // format, is what keeps `4.99` from becoming `4.9900000000000002` or
        // a rounded `5.00`.
        value: amount.to_string(),
    })
}

fn indirect_of(value: &Value) -> Indirect {
    let media_type = value.get("type").and_then(Value::as_str).map(str::to_owned);
    let indirect = value
        .get("child")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .take(MAX_SIBLINGS)
        .map(indirect_of)
        .collect();
    Indirect {
        media_type,
        indirect,
    }
}

/// True unless `properties.availability.state` says otherwise — the shape
/// the OPDS 2.0 test catalog's `Borrow` entry uses to mark every copy
/// checked out.
fn available(properties: &Value) -> bool {
    properties
        .get("availability")
        .and_then(|availability| availability.get("state"))
        .and_then(Value::as_str)
        .is_none_or(|state| !state.eq_ignore_ascii_case("unavailable"))
}

fn acquisitions(base: &str, value: &Value) -> Vec<Acquisition> {
    let mut out = Vec::new();
    for link in array_field(value, "links") {
        if out.len() >= MAX_PER_ENTRY {
            break;
        }
        let tokens = rel_tokens(link);
        let Some(kind) = tokens.iter().find_map(|rel| acquisition_kind(rel)) else {
            continue;
        };
        let Some(raw_href) = link.get("href").and_then(Value::as_str) else {
            continue;
        };
        let Some(href) = resolve_href(base, raw_href) else {
            continue;
        };
        if !crate::url::is_https(&href) {
            continue;
        }
        let properties = link.get("properties");
        let indirect = properties
            .and_then(|p| p.get("indirectAcquisition"))
            .and_then(Value::as_array)
            .unwrap_or(&[])
            .iter()
            .take(MAX_SIBLINGS)
            .map(indirect_of)
            .collect();
        out.push(Acquisition {
            kind,
            href,
            media_type: link.get("type").and_then(Value::as_str).map(str::to_owned),
            title: link.get("title").and_then(Value::as_str).map(str::to_owned),
            // OPDS 1.2 puts the byte length on the link's own `length`
            // attribute; OPDS 2.0 puts the same fact inside `properties`
            // (the parity fixtures pin this exactly). Reading `link.length`
            // directly, the way a first guess at the 2.0 shape would, always
            // finds nothing — the field is never there — so every acquisition
            // read from a 2.0 catalog would silently lose its size.
            length: properties
                .and_then(|p| as_u32(p.get("length")))
                .map(u64::from),
            price: properties.and_then(price_of),
            indirect,
            available: properties.is_none_or(available),
        });
    }
    out
}

/// A well-known feed-level relation, matching [`crate::atom`]'s
/// `kept_relation` table but reached through OPDS 2.0's `rel` grammar
/// (string or array) instead of Atom's space-separated attribute.
fn add_feed_link(feed: &mut Feed, base: &str, link: &Value) {
    let Some(raw_href) = link.get("href").and_then(Value::as_str) else {
        return;
    };
    let Some(href) = resolve_href(base, raw_href) else {
        return;
    };
    if !crate::url::is_https(&href) {
        return;
    }
    let matched: Vec<Relation> = rel_tokens(link)
        .iter()
        .filter_map(|t| kept_relation(t))
        .collect();
    if matched.is_empty() {
        return;
    }
    if feed.links.len() >= MAX_PER_ENTRY {
        return;
    }
    feed.links.push(Link {
        rel: matched,
        href,
        media_type: link.get("type").and_then(Value::as_str).map(str::to_owned),
        title: link.get("title").and_then(Value::as_str).map(str::to_owned),
    });
}

fn parse_navigation(base: &str, value: &Value) -> Option<Navigation> {
    let title = value.get("title").and_then(localized_text)?;
    let raw_href = value.get("href").and_then(Value::as_str)?;
    let href = resolve_href(base, raw_href)?;
    if !crate::url::is_https(&href) {
        return None;
    }
    let rel = rel_tokens(value).iter().find_map(|t| kept_relation(t));
    Some(Navigation {
        title,
        href,
        summary: None,
        // OPDS 2.0 has no equivalent of 1.2's `kind=navigation|acquisition`
        // type parameter — a 2.0 navigation object points at whatever it
        // points at, and the application finds out by fetching it.
        kind: None,
        rel,
        thumbnail: None,
    })
}

fn parse_publication(base: &str, value: &Value) -> Option<Publication> {
    let metadata = value.get("metadata")?;
    let title = metadata.get("title").and_then(localized_text)?;
    let images = array_field(value, "images")
        .filter_map(|image| build_image(base, image))
        .take(MAX_PER_ENTRY)
        .collect();
    Some(Publication {
        title,
        identifier: metadata
            .get("identifier")
            .and_then(Value::as_str)
            .map(str::to_owned),
        authors: contributors(metadata.get("author")),
        summary: metadata.get("description").and_then(localized_text),
        language: match metadata.get("language") {
            Some(Value::String(language)) => Some(language.clone()),
            Some(Value::Array(items)) => items.first().and_then(Value::as_str).map(str::to_owned),
            _ => None,
        },
        // `metadata.published` is OPDS 2.0's one publication-date field, and
        // it means what OPDS 1.2 splits `published`/`dcterms:issued` in two
        // to say: `dcterms:issued` is the work's original year, which is
        // what `published` carries here (the parity fixtures pin this: 1.2
        // writes `<dcterms:issued>1864</dcterms:issued>`, 2.0 writes
        // `"published": "1864"`, and both must land in `issued` or an
        // application would draw a different byline depending on which wire
        // format answered). OPDS 2.0 has nothing corresponding to Atom's own
        // `<published>` (when present, the date an entry was added to the
        // *catalog*, distinct from the work's own publication year), so
        // `published` is always `None` coming from this reader.
        issued: metadata
            .get("published")
            .and_then(Value::as_str)
            .map(str::to_owned),
        published: None,
        updated: metadata
            .get("modified")
            .and_then(Value::as_str)
            .map(str::to_owned),
        publisher: metadata.get("publisher").and_then(localized_text),
        rights: metadata.get("rights").and_then(localized_text),
        extent: None,
        categories: categories(metadata),
        series: series_of(metadata),
        images,
        acquisition: acquisitions(base, value),
        links: Vec::new(),
    })
}

fn parse_facet_group(base: &str, value: &Value) -> FacetGroup {
    let title = value
        .get("metadata")
        .and_then(|m| m.get("title"))
        .and_then(localized_text)
        .unwrap_or_default();
    let facets = array_field(value, "links")
        .filter_map(|link| {
            let raw_href = link.get("href").and_then(Value::as_str)?;
            let href = resolve_href(base, raw_href)?;
            if !crate::url::is_https(&href) {
                return None;
            }
            // OPDS 2.0's convention for "this is the facet currently
            // applied": the facet link carries `rel: "self"`, the same
            // relation a feed uses to name its own address.
            let active = rel_tokens(link).iter().any(|rel| rel == "self");
            let count = link
                .get("properties")
                .and_then(|properties| as_u32(properties.get("numberOfItems")));
            Some(Facet {
                title: link
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                href,
                active,
                count,
            })
        })
        .take(MAX_PER_ENTRY)
        .collect();
    FacetGroup { title, facets }
}

fn parse_group(base: &str, value: &Value) -> Group {
    Group {
        title: value
            .get("metadata")
            .and_then(|m| m.get("title"))
            .and_then(localized_text)
            .unwrap_or_default(),
        href: array_field(value, "links").find_map(|link| {
            if rel_tokens(link).iter().any(|rel| rel == "self") {
                link.get("href")
                    .and_then(Value::as_str)
                    .and_then(|href| resolve_href(base, href))
                    .filter(|href| crate::url::is_https(href))
            } else {
                None
            }
        }),
        navigation: array_field(value, "navigation")
            .filter_map(|item| parse_navigation(base, item))
            .collect(),
        publications: array_field(value, "publications")
            .filter_map(|item| parse_publication(base, item))
            .collect(),
    }
}

/// One piece of a parsed RFC 6570 template: literal text to copy verbatim, a
/// bare `{name}` (simple string expansion, §3.2.2), or a `{?a,b,c}` form
/// expansion (§3.2.8) naming several variables at once.
enum Piece<'a> {
    Literal(&'a str),
    Simple(&'a str),
    Form(Vec<&'a str>),
}

/// Parses the OPDS-relevant subset of RFC 6570: bare `{query}` and
/// `{?query,title,author}`-shaped form expansions. Anything using an
/// operator this does not recognise (`{+var}`, `{/var}`, `{;var}`, `{#var}`)
/// is refused outright — a client that "did its best" with an operator it
/// half understands produces a URL that looks plausible and is wrong, which
/// is worse than a search box that says a catalog cannot be searched.
fn parse_template(template: &str) -> Option<Vec<Piece<'_>>> {
    let mut pieces = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        if start > 0 {
            pieces.push(Piece::Literal(&rest[..start]));
        }
        let tail = &rest[start + 1..];
        let end = tail.find('}')?;
        let expression = &tail[..end];
        if let Some(names) = expression.strip_prefix('?') {
            pieces.push(Piece::Form(names.split(',').collect()));
        } else if !expression.is_empty()
            && expression
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            pieces.push(Piece::Simple(expression));
        } else {
            return None;
        }
        rest = &tail[end + 1..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Literal(rest));
    }
    Some(pieces)
}

/// Expands a search [`Link`]'s href with `query` filling in the template's
/// `query` variable, percent-encoded so the query cannot inject a parameter
/// of its own. Any other named variable (`title`, `author`) is left
/// undefined and — per RFC 6570's own rule for undefined variables — omitted
/// rather than written out empty.
///
/// Returns `None` for a template using an operator [`parse_template`] does
/// not implement, and simply returns the href unchanged when it was not
/// templated at all.
#[must_use]
pub fn expand_search(link: &Link, query: &str) -> Option<String> {
    if !link.href.contains('{') {
        return Some(link.href.clone());
    }
    let pieces = parse_template(&link.href)?;
    let encoded = crate::percent_encode(query);
    let mut out = String::new();
    for piece in pieces {
        match piece {
            Piece::Literal(text) => out.push_str(text),
            Piece::Simple("query") => out.push_str(&encoded),
            Piece::Simple(_) => {}
            Piece::Form(names) => {
                let mut first = true;
                for name in names {
                    if name == "query" {
                        out.push(if first { '?' } else { '&' });
                        out.push_str("query=");
                        out.push_str(&encoded);
                        first = false;
                    }
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AcquisitionKind, ImageSource};

    const HOME: &str = include_str!("../tests/fixtures/opds2/home.json");
    const NAVIGATION: &str = include_str!("../tests/fixtures/opds2/navigation.json");
    const PUBLICATIONS: &str = include_str!("../tests/fixtures/opds2/publications.json");
    const SPEC_FACETS_PAGINATION: &str =
        include_str!("../tests/fixtures/opds2/spec-facets-pagination.json");

    const HOME_BASE: &str = "https://test.opds.io/2.0/home.json";
    const NAVIGATION_BASE: &str = "https://test.opds.io/2.0/navigation.json";
    const PUBLICATIONS_BASE: &str = "https://test.opds.io/2.0/publications.json";

    fn parsed(source: &str, base: &str) -> Feed {
        parse(source, base).expect("the fixture is well formed JSON")
    }

    #[test]
    fn a_home_feed_yields_navigation_groups_and_publications_at_once() {
        let feed = parsed(HOME, HOME_BASE);
        assert_eq!(feed.title.as_deref(), Some("OPDS 2.0 Test Catalog"));
        assert!(!feed.navigation.is_empty());
        assert!(!feed.groups.is_empty());
        assert!(!feed.publications.is_empty());

        let french_classics = feed
            .groups
            .iter()
            .find(|g| g.title == "French Classics")
            .expect("a French Classics group");
        assert_eq!(french_classics.publications.len(), 5);

        let more_navigation = feed
            .groups
            .iter()
            .find(|g| g.title == "More Navigation")
            .expect("a group of navigation rather than publications");
        assert_eq!(more_navigation.navigation.len(), 2);
        assert!(more_navigation.publications.is_empty());

        // Moby-Dick is both in the "English Classics" group and in the
        // feed's own flat publications list — matching what home.json
        // itself does, on purpose (see `Group`'s doc comment).
        assert!(feed
            .groups
            .iter()
            .find(|g| g.title == "English Classics")
            .expect("an English Classics group")
            .publications
            .iter()
            .any(|p| p.title == "Moby-Dick"));
        assert!(feed.publications.iter().any(|p| p.title == "Moby-Dick"));
    }

    #[test]
    fn a_navigation_entrys_relative_and_absolute_hrefs_both_resolve() {
        let feed = parsed(NAVIGATION, NAVIGATION_BASE);
        let absolute = feed
            .navigation
            .iter()
            .find(|n| n.title.contains("Absolute URI"))
            .expect("the absolute entry");
        assert_eq!(absolute.href, "https://test.opds.io/2.0/home.json");

        let relative = feed
            .navigation
            .iter()
            .find(|n| n.title.contains("Relative URI"))
            .expect("the relative entry");
        assert_eq!(relative.href, "https://test.opds.io/2.0/home.json");

        let root_relative = feed
            .navigation
            .iter()
            .find(|n| n.title.contains("Also Relative"))
            .expect("the root-relative entry");
        assert_eq!(root_relative.href, "https://test.opds.io/2.0/home.json");
    }

    #[test]
    fn a_rel_written_as_an_array_is_matched_on_every_one_of_its_values() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        // `"rel": ["first", "previous"]` — a single link answering to both.
        let first = feed.first().expect("first, from the array rel");
        let previous = feed.previous().expect("previous, from the same array rel");
        assert_eq!(first.href, previous.href);
        assert_eq!(first.href, "https://example.com/?page=1");

        // `"rel": "preview"` on the same fixture's sample link is the single
        // string form; publications.json's own array-rel case,
        // `["http://opds-spec.org/acquisition/sample", "preview"]`, is the
        // other shape the same relation is spelled in the wild.
        let feed = parsed(PUBLICATIONS, PUBLICATIONS_BASE);
        let sample_publication = feed
            .publications
            .iter()
            .find(|p| p.title == "Sample")
            .expect("the Sample entry");
        assert!(sample_publication
            .acquisition
            .iter()
            .any(|a| a.kind == AcquisitionKind::Sample));
    }

    #[test]
    fn an_author_written_as_a_string_an_object_and_an_array_all_read_the_same() {
        let feed = parsed(PUBLICATIONS, PUBLICATIONS_BASE);
        let bare_string = feed
            .publications
            .iter()
            .find(|p| p.title == "Voyage au centre de la Terre")
            .expect("bare string author");
        assert_eq!(bare_string.authors, vec!["Jules Verne".to_owned()]);

        let object = feed
            .publications
            .iter()
            .find(|p| p.title == "Author Using an Object")
            .expect("object author");
        assert_eq!(object.authors, vec!["Jules Verne".to_owned()]);

        let array = feed
            .publications
            .iter()
            .find(|p| p.title == "Multiple Authors")
            .expect("array of strings");
        assert_eq!(
            array.authors,
            vec!["Jules Verne".to_owned(), "Second Author".to_owned()]
        );

        let array_of_objects = feed
            .publications
            .iter()
            .find(|p| p.title == "Multiple Authors Using Objects")
            .expect("array of objects");
        assert_eq!(
            array_of_objects.authors,
            vec!["Jules Verne".to_owned(), "Second Author".to_owned()]
        );
    }

    #[test]
    fn a_facet_carries_its_count_and_says_which_one_is_active() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        let language = &feed.facets[0];
        assert_eq!(language.title, "Language");
        let german = language
            .facets
            .iter()
            .find(|f| f.title == "German")
            .expect("the German facet");
        assert!(german.active, "German carries rel: self");
        assert_eq!(german.count, Some(6));
        let french = language
            .facets
            .iter()
            .find(|f| f.title == "French")
            .expect("the French facet");
        assert!(!french.active);
        assert_eq!(french.count, Some(10));
    }

    #[test]
    fn full_pagination_is_read_from_the_feeds_own_metadata() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        assert_eq!(feed.pagination.total, Some(5678));
        assert_eq!(feed.pagination.per_page, Some(50));
        assert_eq!(feed.pagination.current_page, Some(2));
        assert_eq!(
            feed.next().expect("next").href,
            "https://example.com/?page=3"
        );
        assert_eq!(
            feed.last().expect("last").href,
            "https://example.com/?page=114"
        );
    }

    #[test]
    fn a_templated_search_link_expands_with_the_query_percent_encoded() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        let search = feed.search().expect("a templated search link");
        assert!(search.href.contains('{'), "the href is still a template");
        let expanded = expand_search(search, "pride & prejudice").expect("expands");
        assert_eq!(
            expanded,
            "https://example.com/search?query=pride%20%26%20prejudice"
        );
        assert!(!expanded.contains('{'));
    }

    #[test]
    fn a_search_link_that_is_not_templated_expands_to_itself() {
        let link = Link {
            rel: vec![Relation::Search],
            href: "https://example.com/search".to_owned(),
            media_type: None,
            title: None,
        };
        assert_eq!(
            expand_search(&link, "anything").as_deref(),
            Some("https://example.com/search")
        );
    }

    #[test]
    fn a_price_keeps_the_digits_it_was_written_with_rather_than_being_rounded() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        let publication = &feed.publications[0];
        let buy = publication
            .acquisition
            .iter()
            .find(|a| a.kind == AcquisitionKind::Buy)
            .expect("a buy acquisition");
        let price = buy.price.as_ref().expect("a price");
        assert_eq!(price.currency.as_deref(), Some("USD"));
        assert_eq!(price.value, "4.99");
    }

    #[test]
    fn indirect_acquisition_is_read_from_the_properties_object() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        let publication = &feed.publications[0];
        let buy = publication
            .acquisition
            .iter()
            .find(|a| a.kind == AcquisitionKind::Buy)
            .expect("a buy acquisition");
        assert_eq!(buy.indirect.len(), 1);
        assert_eq!(
            buy.indirect[0].media_type.as_deref(),
            Some("application/epub+zip")
        );
    }

    #[test]
    fn contributor_as_object_carries_the_belongs_to_series_too() {
        let feed = parsed(SPEC_FACETS_PAGINATION, "https://example.com/?page=2");
        let publication = &feed.publications[0];
        assert_eq!(publication.authors, vec!["Jules Verne".to_owned()]);
        let series = publication.series.as_ref().expect("a series");
        assert_eq!(series.name, "The Extraordinary Voyages");
        assert_eq!(series.position, Some(3.0));
    }

    #[test]
    fn a_borrowed_book_with_every_copy_checked_out_is_unavailable() {
        let feed = parsed(PUBLICATIONS, PUBLICATIONS_BASE);
        let borrow_publication = feed
            .publications
            .iter()
            .find(|p| p.title == "Borrow")
            .expect("the Borrow entry");
        let borrow = &borrow_publication.acquisition[0];
        assert!(!borrow.available);
        assert_eq!(borrow_publication.best_acquisition(), None);
    }

    #[test]
    fn a_link_that_is_not_https_never_becomes_something_to_fetch() {
        let source = r#"{
            "metadata": {"title": "Hostile"},
            "publications": [{
                "metadata": {"title": "Insecure Download"},
                "links": [
                    {"rel": "http://opds-spec.org/acquisition", "href": "http://example.com/plain.epub", "type": "application/epub+zip"}
                ],
                "images": [
                    {"href": "javascript:alert(1)", "type": "image/jpeg"}
                ]
            }]
        }"#;
        let feed = parsed(source, "https://example.com/catalog.json");
        let publication = &feed.publications[0];
        assert!(publication.acquisition.is_empty());
        assert!(publication.images.is_empty());
    }

    #[test]
    fn one_malformed_publication_does_not_discard_the_entries_around_it() {
        let source = r#"{
            "metadata": {"title": "Mostly Fine"},
            "publications": [
                {"metadata": {"title": "Before"}, "links": []},
                {"metadata": {}, "links": []},
                "not even an object",
                {"metadata": {"title": "After"}, "links": []}
            ]
        }"#;
        let feed = parsed(source, "https://example.com/catalog.json");
        let titles: Vec<&str> = feed.publications.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["Before", "After"]);
    }

    #[test]
    fn a_json_body_is_read_as_2_0_whatever_the_server_said_the_type_was() {
        let feed = parsed(HOME, HOME_BASE);
        assert_eq!(feed.version, Version::Json);
    }

    #[test]
    fn a_data_uri_image_in_a_2_0_catalog_is_decoded_rather_than_fetched() {
        let source = r#"{
            "metadata": {"title": "Inline"},
            "publications": [{
                "metadata": {"title": "One"},
                "links": [{"rel": "http://opds-spec.org/acquisition/open-access", "href": "https://example.com/book.epub", "type": "application/epub+zip"}],
                "images": [{"href": "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7", "type": "image/gif"}]
            }]
        }"#;
        let feed = parsed(source, "https://example.com/catalog.json");
        let cover = feed.publications[0].cover().expect("a cover");
        match &cover.href {
            ImageSource::Inline { media_type, bytes } => {
                assert_eq!(media_type, "image/gif");
                assert!(!bytes.is_empty());
            }
            ImageSource::Url(_) => panic!("expected an inline image"),
        }
    }
}
