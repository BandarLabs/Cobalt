//! Bounded OPML subscription lists, read the same way on both machines.
//!
//! Reading an outline never fetches anything: an OPML file is a list of
//! addresses, and a parser that visited them would turn opening a file into a
//! few hundred requests nobody asked for.
//!
//! The reader and the computer share this because they have to agree about
//! what is importable. A file the reader would refuse should be refused on the
//! computer, where there is a keyboard and a full screen to say why, rather
//! than after it has been carried across and listed on a panel.

use kobo_xml::{scan, Event};

/// The largest subscription list this accepts.
///
/// A quarter of a megabyte is a few thousand feeds. Past that it is not a
/// reading list, and the device has 500 MB of memory for everything it does.
pub const LIMIT: usize = 256 * 1024;

/// The most outline elements one file may contain, nested or not.
const MAX_OUTLINES: usize = 2000;

/// The longest title kept from an outline.
const MAX_TITLE: usize = 300;

/// One subscription named by an outline.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Feed {
    /// The feed address, which is always HTTPS and carries no credentials.
    pub url: String,
    /// What the outline called it, or empty when it said nothing. Callers
    /// decide what to show in its place: the reader uses the host.
    pub title: String,
    /// The site the outline pointed at, if any.
    pub site: String,
}

/// What one file offers, and how much of it could not be used.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Import {
    pub feeds: Vec<Feed>,
    /// Outlines naming a duplicate, a plain HTTP address, an address carrying
    /// credentials, or something that is not an address at all. Counted rather
    /// than dropped silently, so a preview can say "12 of 14".
    pub skipped: usize,
}

/// Reads a subscription list.
///
/// # Errors
///
/// Returns a sentence fit to put on a panel when the file is too large, is not
/// UTF-8, is not an OPML document, or ends in the middle of one. A file that
/// parses but names nothing usable is not an error: it is an import of zero
/// feeds with everything counted as skipped.
pub fn parse(bytes: &[u8]) -> Result<Import, &'static str> {
    if bytes.len() > LIMIT {
        return Err("This OPML file is too large.");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "This OPML file is not valid UTF-8.")?;
    // A document that stops inside a tag, a comment or a character section is
    // a half-copied file, and the scanner is built to stop there quietly so a
    // feed can still be read from what did arrive. A subscription list is the
    // one thing that must not be read that way: the reader would be told it
    // imported nothing, which is what an empty list looks like too, and would
    // copy the file again for nothing. The last angle bracket with no closing
    // one after it is exactly where the scanner gave up.
    if text
        .rsplit_once('<')
        .is_some_and(|(_, tail)| !tail.contains('>'))
    {
        return Err("This OPML file is incomplete or damaged.");
    }
    let mut scratch = Vec::new();
    let mut events = Vec::new();
    scan(text, &mut scratch, |event| events.push(event));
    let mut stack = Vec::new();
    let mut closed = false;
    let mut root = false;
    let mut outlines = 0;
    let mut import = Import::default();
    for event in events {
        match event {
            Event::Open { name, .. } => {
                if closed || (stack.is_empty() && !name.eq_ignore_ascii_case("opml")) {
                    return Err("This file is not an OPML subscription list.");
                }
                root = true;
                if name.eq_ignore_ascii_case("outline") {
                    outlines += 1;
                    if outlines > MAX_OUTLINES {
                        return Err("This OPML file contains too many entries.");
                    }
                    if let Some(url) = event.attribute("xmlUrl") {
                        let url = url.trim();
                        if url.len() > 4096
                            || kobo_net::parse(url).is_err()
                            || import.feeds.iter().any(|feed| feed.url == url)
                        {
                            import.skipped += 1;
                        } else {
                            let title = event
                                .attribute("title")
                                .or_else(|| event.attribute("text"))
                                .unwrap_or_default();
                            import.feeds.push(Feed {
                                url: url.to_owned(),
                                title: title.trim().chars().take(MAX_TITLE).collect(),
                                site: event.attribute("htmlUrl").unwrap_or_default(),
                            });
                        }
                    }
                }
                stack.push(name);
            }
            Event::Close { name } => {
                if stack.pop() != Some(name) {
                    return Err("This OPML file is incomplete or damaged.");
                }
                if stack.is_empty() {
                    closed = true;
                }
            }
            Event::Text(text) if stack.is_empty() && !text.trim().is_empty() => {
                return Err("This OPML file contains unexpected text.");
            }
            Event::Owned(index)
                if stack.is_empty()
                    && scratch
                        .get(index)
                        .is_some_and(|text| !text.trim().is_empty()) =>
            {
                return Err("This OPML file contains unexpected text.");
            }
            Event::Text(_) | Event::Owned(_) => {}
        }
    }
    if !root || !closed || !stack.is_empty() {
        return Err("This OPML file is incomplete or damaged.");
    }
    Ok(import)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_that_stops_inside_a_tag_is_damaged_rather_than_empty() {
        // Each of these parsed as a successful import of whatever had arrived
        // before the truncation, which tells a reader their subscription list
        // held nothing when it in fact held an unknown amount more.
        for truncated in [
            &b"<opml></opml><"[..],
            &br#"<opml><body><outline xmlUrl="https://a.test/f"/></body></opml><out"#[..],
            &br#"<opml><body><outline xmlUrl="https://a.test/f"/></body></opml><!-- "#[..],
            &b"<opml><body></body></opml><![CDATA[x"[..],
        ] {
            assert_eq!(
                parse(truncated),
                Err("This OPML file is incomplete or damaged."),
                "{}",
                std::str::from_utf8(truncated).expect("probe text")
            );
        }
    }

    #[test]
    fn an_angle_bracket_inside_a_character_section_is_not_a_truncation() {
        let import = parse(
            br#"<opml><body><outline text="a &lt; b" xmlUrl="https://a.test/f"/><note><![CDATA[a < b]]></note></body></opml>"#,
        )
        .expect("a complete document");
        assert_eq!(import.feeds.len(), 1);
    }

    #[test]
    fn nested_outlines_preserve_titles_and_decode_urls() {
        let import = parse(br#"<?xml version="1.0"?><opml version="2.0"><body><outline text="News"><outline text="A &amp; B" xmlUrl="https://example.com/feed?a=1&amp;b=2"/></outline></body></opml>"#).unwrap();
        assert_eq!(import.feeds.len(), 1);
        assert_eq!(import.feeds[0].title, "A & B");
        assert_eq!(import.feeds[0].url, "https://example.com/feed?a=1&b=2");
    }

    #[test]
    fn duplicates_and_unusable_urls_are_counted_for_the_preview() {
        let import = parse(br#"<opml><body><outline xmlUrl="https://example.com/feed"/><outline xmlUrl="https://example.com/feed"/><outline xmlUrl="http://example.com/feed"/><outline xmlUrl="https://user:pass@example.com/feed"/></body></opml>"#).unwrap();
        assert_eq!(import.feeds.len(), 1);
        assert_eq!(import.skipped, 3);
    }

    #[test]
    fn an_outline_without_a_title_leaves_one_for_the_caller_to_supply() {
        let import =
            parse(br#"<opml><body><outline xmlUrl="https://example.com/feed" htmlUrl="https://example.com/"/></body></opml>"#)
                .unwrap();
        assert_eq!(import.feeds[0].title, "");
        assert_eq!(import.feeds[0].site, "https://example.com/");
    }

    #[test]
    fn incomplete_and_non_opml_documents_do_not_become_partial_imports() {
        for bytes in [
            b"<html></html>".as_slice(),
            b"<opml><body><outline xmlUrl='https://example.com/feed'/>",
            b"<opml><body></opml>",
            b"<opml></opml><opml></opml>",
            &[0xff],
        ] {
            assert!(parse(bytes).is_err(), "{bytes:?}");
        }
        assert!(parse(&vec![b' '; LIMIT + 1]).is_err());
    }
}
