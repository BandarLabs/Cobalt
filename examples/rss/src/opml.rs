//! Feeds reads subscription lists with the shared OPML reader.
//!
//! The one thing it adds is the name to show for an outline that carried none,
//! which is a question about this shelf rather than about the file.
use super::{pretty_host, Subscription};
pub use kobo_opml::LIMIT;

#[derive(Debug, Default)]
pub struct Import {
    pub feeds: Vec<Subscription>,
    pub skipped: usize,
}

pub fn parse(bytes: &[u8]) -> Result<Import, &'static str> {
    let read = kobo_opml::parse(bytes)?;
    Ok(Import {
        feeds: read
            .feeds
            .into_iter()
            .map(|feed| Subscription {
                title: if feed.title.is_empty() {
                    pretty_host(&feed.site, &feed.url)
                } else {
                    feed.title
                },
                url: feed.url,
                site: feed.site,
            })
            .collect(),
        skipped: read.skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untitled_outline_is_named_after_the_site_it_came_from() {
        let import = parse(
            br#"<opml><body><outline xmlUrl="https://example.com/feed" htmlUrl="https://example.com/"/></body></opml>"#,
        )
        .unwrap();
        assert_eq!(import.feeds[0].title, "example.com");
    }
}
