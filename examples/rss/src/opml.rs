//! Bounded OPML parsing. Reading an outline never fetches its feed URLs.
use super::Subscription;
use kobo_xml::{scan, Event};

pub const LIMIT: usize = 256 * 1024;
const MAX_OUTLINES: usize = 2000;

#[derive(Debug, Default)]
pub struct Import {
    pub feeds: Vec<Subscription>,
    pub skipped: usize,
}

pub fn parse(bytes: &[u8]) -> Result<Import, &'static str> {
    if bytes.len() > LIMIT {
        return Err("This OPML file is too large.");
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "This OPML file is not valid UTF-8.")?;
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
                            let site = event.attribute("htmlUrl").unwrap_or_default();
                            import.feeds.push(Subscription {
                                url: url.to_owned(),
                                title: if title.trim().is_empty() {
                                    super::pretty_host(&site, url)
                                } else {
                                    title.chars().take(300).collect()
                                },
                                site,
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
