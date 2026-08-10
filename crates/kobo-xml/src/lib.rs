//! A small, defensive XML pull scanner, shared by whatever in this workspace
//! reads XML that a stranger's server produced.
//!
//! It used to live inside the RSS feed reader, because that was the first
//! thing that needed it. An OPDS catalog reader needs the same scanner over
//! the same kind of adversarial input — a `<` with no `>`, a character
//! section that never closes, an element nested absurdly deep, a namespace
//! prefix chosen to collide with a name a reader cares about — and
//! duplicating a hand-rolled scanner for the second consumer would be how the
//! two quietly drift apart on what "malformed" means. So the scanner moved
//! here, and the feed reader became its first caller instead of its home.
//!
//! # Why this is not a general XML parser
//!
//! Because a general XML parser would be a liability for what calls this.
//! The documents this reads are written by strangers and served by machines
//! nobody here controls, and the interesting inputs are the malformed ones.
//! A parser that models XML faithfully has to have an answer for all of it:
//! DTDs, external entities, namespace validation, well-formedness errors. This
//! one has none of that. It walks a flat run of elements and text and treats
//! everything it cannot make sense of as a reason to stop, not a reason to
//! guess. Every malformed shape has the same defined outcome: scanning stops
//! early, and whatever was understood before that point is what the caller
//! gets. There is no input that makes it recurse and none that makes it
//! allocate a multiple of its input.

/// How deep the element stack may go before a document is treated as
/// malformed.
///
/// The documents this crate's callers read are shallow by nature — a
/// syndication feed is three levels deep, an OPDS catalog page a similar
/// handful — and inline markup adds only a few more levels on top of that.
/// Anything nested past this is either not really shaped like one of those
/// documents or is built to make a scanner grow without bound, and both
/// deserve the same answer: stop.
pub const MAX_DEPTH: usize = 64;

/// One step of an XML-shaped document, as [`scan`] walks it.
#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    Open {
        name: &'a str,
        attributes: &'a str,
    },
    /// Text that needed no decoding, borrowed from the document.
    Text(&'a str),
    /// Text that had entities in it, and so had to be built. The index is into
    /// the scratch the caller passed in.
    Owned(usize),
    Close {
        name: &'a str,
    },
}

impl<'a> Event<'a> {
    /// The element name this step carries, for an `Open` or a `Close`.
    ///
    /// Text steps have no name to give back, so this is the honest way to ask
    /// "what element is this" without every caller re-deriving the match.
    #[must_use]
    pub fn name(&self) -> Option<&'a str> {
        match self {
            Event::Open { name, .. } | Event::Close { name } => Some(name),
            Event::Text(_) | Event::Owned(_) => None,
        }
    }

    /// Reads a named attribute's value off an element-open event, with
    /// entities decoded.
    ///
    /// Only an `Open` event carries attributes, so every other step answers
    /// nothing rather than being a call site error.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<String> {
        match self {
            Event::Open { attributes, .. } => attribute(attributes, name),
            Event::Text(_) | Event::Owned(_) | Event::Close { .. } => None,
        }
    }
}

/// Walks the document, calling `visit` for each step.
///
/// Stops at the first thing it cannot make sense of rather than guessing,
/// because a document truncated by a proxy is far more common than one that
/// is subtly wrong, and half a document is a useful answer.
pub fn scan<'a>(input: &'a str, decoded: &mut Vec<String>, mut visit: impl FnMut(Event<'a>)) {
    let mut rest = input;
    let mut depth = 0usize;
    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            emit_text(rest, decoded, &mut visit);
            return;
        };
        if open > 0 {
            emit_text(&rest[..open], decoded, &mut visit);
            rest = &rest[open..];
            continue;
        }

        if let Some(tail) = rest.strip_prefix("<!--") {
            let Some(end) = tail.find("-->") else { return };
            rest = &tail[end + 3..];
        } else if let Some(tail) = rest.strip_prefix("<![CDATA[") {
            let Some(end) = tail.find("]]>") else { return };
            // Verbatim: the whole point of a character section is that what is
            // inside it was never escaped, and so must not be decoded again.
            visit(Event::Text(&tail[..end]));
            rest = &tail[end + 3..];
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            let Some(end) = rest.find('>') else { return };
            rest = &rest[end + 1..];
        } else {
            let Some(end) = rest.find('>') else { return };
            let inner = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some(name) = inner.strip_prefix('/') {
                depth = depth.saturating_sub(1);
                visit(Event::Close { name: name.trim() });
            } else {
                let closes_itself = inner.ends_with('/');
                let inner = inner.strip_suffix('/').unwrap_or(inner);
                let (name, attributes) = match inner.find(char::is_whitespace) {
                    Some(split) => (&inner[..split], &inner[split..]),
                    None => (inner, ""),
                };
                if name.is_empty() {
                    continue;
                }
                visit(Event::Open { name, attributes });
                if closes_itself {
                    visit(Event::Close { name });
                } else {
                    depth += 1;
                    if depth > MAX_DEPTH {
                        return;
                    }
                }
            }
        }
    }
}

/// Emits a run of text, decoding entities only when there are any.
fn emit_text<'a>(raw: &'a str, decoded: &mut Vec<String>, visit: &mut impl FnMut(Event<'a>)) {
    if raw.trim().is_empty() {
        return;
    }
    if raw.contains('&') {
        decoded.push(decode_entities(raw));
        visit(Event::Owned(decoded.len() - 1));
    } else {
        visit(Event::Text(raw));
    }
}

/// The five entities XML defines, plus numeric ones.
///
/// Named HTML entities beyond these are left alone deliberately. They are
/// only legal in a document that declared them itself, and a caller that goes
/// on to convert marked-up text to plain text has its own, larger entity
/// table for that job. Decoding them here too is how `&amp;lt;` becomes a tag.
#[must_use]
pub fn decode_entities(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        // An entity is short. Looking further than this for the semicolon
        // turns a bare ampersand in prose into a scan of the rest of the post.
        let Some(end) = tail
            .bytes()
            .take(12)
            .position(|byte| byte == b';')
            .filter(|end| *end > 1)
        else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        let replacement = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => numeric_entity(name),
        };
        match replacement {
            Some(character) => out.push(character),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `#38` and `#x26`, or nothing.
fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

/// Splits a raw attribute string and picks out one value by name, whichever
/// quote style the document used.
fn raw_attribute<'a>(attributes: &'a str, want: &str) -> Option<&'a str> {
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

/// Reads one attribute's value out of an element's raw attribute text, with
/// entities decoded.
///
/// An OPDS `<link>` carries most of what matters — `href`, `rel`, `type`,
/// `title` — as attributes rather than as element text, and unlike an Atom
/// `href` those attributes routinely hold prose: a `title` or `label` on a
/// facet link carries `&amp;` as often as any element's text does. Decoding
/// here, rather than leaving it to whichever caller remembers, is how a
/// catalog with an ampersand in a shelf name doesn't quietly show `&amp;`.
#[must_use]
pub fn attribute(attributes: &str, want: &str) -> Option<String> {
    let raw = raw_attribute(attributes, want)?;
    Some(if raw.contains('&') {
        decode_entities(raw)
    } else {
        raw.to_owned()
    })
}

/// Splits a qualified element name into its namespace prefix, if it has one,
/// and its local name.
///
/// OPDS documents lean on namespace prefixes in a way syndication feeds
/// mostly don't: `dcterms:language`, `opds:price`, `media:thumbnail`,
/// `thr:count`, `opensearch:totalResults`. A reader for those has to decide,
/// per element, whether the prefix is load-bearing or whether only the local
/// name is — `media:content` and `content` cannot be treated as the same
/// element, but `atom:title` and `title` usually should be — so this hands
/// back both parts and leaves the choice to the caller rather than making it
/// once for everyone.
#[must_use]
pub fn split_name(name: &str) -> (Option<&str>, &str) {
    match name.split_once(':') {
        Some((prefix, local)) => (Some(prefix), local),
        None => (None, name),
    }
}

#[cfg(test)]
mod tests {
    use super::{attribute, decode_entities, scan, split_name, Event};

    /// Runs `scan` and collects the element names and text it produced, in
    /// order, resolving `Owned` indices against the scratch so assertions can
    /// read like the document rather than like the scanner's internals.
    fn steps(input: &str) -> Vec<String> {
        let mut decoded = Vec::new();
        let mut out = Vec::new();
        scan(input, &mut decoded, |event| {
            out.push(match event {
                Event::Open { name, .. } => format!("open:{name}"),
                Event::Close { name } => format!("close:{name}"),
                Event::Text(text) => format!("text:{text}"),
                Event::Owned(_) => String::new(),
            });
        });
        // A second pass resolves `Owned` after the fact, since the closure
        // above cannot borrow `decoded` while `scan` still holds it mutably.
        let mut decoded_iter = decoded.into_iter();
        for entry in &mut out {
            if entry.is_empty() {
                *entry = format!("text:{}", decoded_iter.next().unwrap_or_default());
            }
        }
        out
    }

    #[test]
    fn a_prefixed_element_name_is_matched_by_its_local_name_and_by_its_qualified_name() {
        assert_eq!(split_name("dcterms:language"), (Some("dcterms"), "language"));
        let (prefix, local) = split_name("dcterms:language");
        assert_eq!(prefix, Some("dcterms"));
        assert_eq!(local, "language");
        // A caller matching on the qualified name entire, the way the feed
        // reader's own field-matching does, still has the whole string.
        assert_eq!(split_name("title"), (None, "title"));
    }

    #[test]
    fn a_named_attributes_value_comes_back_with_entities_decoded() {
        assert_eq!(
            attribute(r#"title="Fantasy &amp; Sci-Fi""#, "title"),
            Some("Fantasy & Sci-Fi".to_owned())
        );
    }

    #[test]
    fn an_attribute_that_is_absent_comes_back_as_nothing_rather_than_an_empty_string() {
        assert_eq!(attribute(r#"href="x""#, "missing"), None);
        assert_eq!(attribute("", "href"), None);
    }

    #[test]
    fn a_document_nested_past_the_depth_cap_stops_rather_than_growing() {
        let source = "<a>".repeat(10_000);
        let mut decoded = Vec::new();
        let mut opens = 0usize;
        scan(&source, &mut decoded, |event| {
            if matches!(event, Event::Open { .. }) {
                opens += 1;
            }
        });
        assert!(opens <= super::MAX_DEPTH + 1, "opens = {opens}");
    }

    #[test]
    fn an_unclosed_element_does_not_swallow_the_rest_of_the_document() {
        let events = steps("<a><b>one</b><c");
        assert_eq!(events, vec!["open:a", "open:b", "text:one", "close:b"]);
    }

    #[test]
    fn numeric_character_references_in_both_decimal_and_hexadecimal_form_decode() {
        assert_eq!(decode_entities("&#65;&#x42;"), "AB");
        assert_eq!(decode_entities("&#x2603;"), "\u{2603}");
    }

    #[test]
    fn an_element_open_event_answers_its_own_attributes() {
        let mut decoded = Vec::new();
        let mut seen = None;
        scan(r#"<link href="https://example.com/a &amp; b"/>"#, &mut decoded, |event| {
            if let Event::Open { .. } = event {
                seen = event.attribute("href");
            }
        });
        assert_eq!(seen, Some("https://example.com/a & b".to_owned()));
    }
}
