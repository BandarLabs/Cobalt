//! Bounded catalog navigation. Account credentials stay on the configured origin.
use kobo_opds::{Feed, Publication};
use kobo_sdk::TaskError;

pub const LIMIT: usize = 256 * 1024;
pub const HISTORY: usize = 12;

pub fn endpoint(input: &str) -> Option<String> {
    let input = input.trim();
    let rest = input.strip_prefix("https://")?;
    if input.len() > 2048
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
        || input.contains(['\\', '#', '|'])
    {
        return None;
    }
    let authority = rest.split(['/', '?']).next()?;
    if authority.is_empty() || authority.contains(['@', '%']) {
        return None;
    }
    let (host, port) = if let Some(ip) = authority.strip_prefix('[') {
        let (ip, suffix) = ip.split_once(']')?;
        ip.parse::<std::net::Ipv6Addr>().ok()?;
        (
            ip,
            if suffix.is_empty() {
                None
            } else {
                Some(suffix.strip_prefix(':')?)
            },
        )
    } else {
        let (host, port) = authority
            .split_once(':')
            .map_or((authority, None), |(h, p)| (h, Some(p)));
        if host.is_empty()
            || !host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return None;
        }
        (host, port)
    };
    if host.len() > 253 || port.is_some_and(|p| p.parse::<u16>().map_or(true, |n| n == 0)) {
        return None;
    }
    let suffix = &rest[authority.len()..];
    Some(if suffix.is_empty() || suffix == "/" {
        format!("https://{authority}/opds")
    } else {
        input.to_owned()
    })
}

pub fn allowed(root: &str, target: &str) -> bool {
    endpoint(target).is_some() && kobo_opds::same_origin(root, target)
}

#[derive(Clone)]
pub struct Page {
    pub url: String,
    pub feed: Feed,
    pub offset: usize,
}
impl Page {
    pub fn parse(bytes: &[u8], url: &str) -> Result<Self, &'static str> {
        if bytes.len() > LIMIT {
            return Err("This catalog page is too large. Use a paginated OPDS catalog.");
        }
        let feed = kobo_opds::parse(bytes, url).map_err(|_| {
            "The server returned a web page or invalid catalog. Check the OPDS address."
        })?;
        Ok(Self {
            url: url.into(),
            feed,
            offset: 0,
        })
    }
    pub fn count(&self) -> usize {
        self.feed.navigation.len() + self.feed.publications.len()
    }
    pub fn book(&self, index: usize) -> Option<&Publication> {
        index
            .checked_sub(self.feed.navigation.len())
            .and_then(|i| self.feed.publications.get(i))
    }
}

/// What went wrong with a request to the library at `url`, in the owner's
/// terms. The library's origin is what decides the home-network hint, so a
/// reader away from home is told that rather than to check the address.
pub fn problem(url: &str, error: TaskError) -> kobo_sdk::failure::Problem {
    let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
    kobo_sdk::failure::Service::new("Your library")
        .at(&origin)
        .fixed_by("Tap Sign in again.")
        .explain(error)
}

pub fn failure(url: &str, error: TaskError) -> String {
    problem(url, error).message
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_keeps_explicit_paths_and_refuses_credentials_and_invalid_hosts() {
        assert_eq!(
            endpoint(" https://library.local/ ").as_deref(),
            Some("https://library.local/opds")
        );
        assert_eq!(
            endpoint("https://library.local/books/opds?page=2").as_deref(),
            Some("https://library.local/books/opds?page=2")
        );
        assert!(endpoint("https://[::1]:8443/opds").is_some());
        for input in [
            "http://library",
            "https://",
            "https://u:p@library/opds",
            "https://library\\evil",
            "https://library:0",
            "https://library:99999",
            "https://a..b",
            "https://library/#x",
        ] {
            assert!(endpoint(input).is_none(), "{input}");
        }
        assert!(allowed(
            "https://library/opds",
            "https://library:443/opds/books"
        ));
        assert!(!allowed("https://library/opds", "https://elsewhere/opds"));
        assert!(!allowed("https://library/opds", "https://u@library/opds"));
    }
    #[test]
    fn original_feed_resolves_sections_books_and_next_page() {
        let page = Page::parse(
            include_bytes!("../fixtures/root.xml"),
            "https://library.test/opds",
        )
        .unwrap();
        assert_eq!(page.count(), 3);
        assert_eq!(page.feed.navigation[0].title, "Authors");
        assert_eq!(
            page.feed.navigation[0].href,
            "https://library.test/opds/authors"
        );
        let book = page.book(2).unwrap();
        assert_eq!(book.title, "A Walk by the River");
        assert_eq!(book.authors, ["Cobalt sample"]);
        assert_eq!(
            book.best_acquisition().unwrap().href,
            "https://library.test/books/river.epub"
        );
        assert_eq!(
            page.feed.next().unwrap().href,
            "https://library.test/opds?page=2"
        );
        assert!(Page::parse(b"<html><body>Sign in</body></html>", &page.url).is_err());
        assert!(Page::parse(&vec![b' '; LIMIT + 1], &page.url).is_err());
    }
}
