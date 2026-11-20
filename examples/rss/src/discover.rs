//! Finding a site's feed from the address of one of its pages.
//!
//! An owner who wants to follow a newsletter has the address of its site, not
//! of its feed, and most do not know a feed exists. Pasting that address used
//! to end at "This address did not return a feed", which was true and no help.
//!
//! A site says where its feed is in two ways. Nearly every publishing system
//! writes a `<link rel="alternate">` into the head of each page naming it,
//! which is what a browser's own feed button once read. Where that is
//! missing, the feed still sits at one of a handful of paths the same systems
//! have used for twenty years. Both are tried here, the site's own word first.
//!
//! Everything this proposes is fetched from the site itself, or from wherever
//! the site's page pointed. A pasted address is still never sent to the
//! search service: the promise was that a private feed address stays between
//! the reader and its publisher, and looking harder must not break it.

/// The most addresses tried for one page.
///
/// Each is a request the owner waits through, a second or more apiece on the
/// reader. A page that names its feed is answered by the first.
const MAX_CANDIDATES: usize = 8;

/// The kinds a page gives the feeds it links to.
const FEED_TYPES: [&str; 4] = [
    "application/rss+xml",
    "application/atom+xml",
    "application/feed+json",
    "application/rdf+xml",
];

/// Where publishing systems put a feed when the page does not say.
///
/// In the order they are most often right: the first two are what the common
/// weblog and newsletter systems use, and the rest belong to the generators
/// that write a site out as files.
const USUAL_PATHS: [&str; 6] = [
    "feed",
    "rss",
    "feed.xml",
    "atom.xml",
    "rss.xml",
    "index.xml",
];

/// Every address worth asking for the feed of the page at `address`, best
/// first: the feeds the page itself links to, then the usual places.
///
/// Nothing when the address is not one the runtime will fetch.
#[must_use]
pub fn candidates(page: &[u8], address: &str) -> Vec<String> {
    let Ok(home) = kobo_net::parse(address) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for link in feed_links(&String::from_utf8_lossy(page), &home) {
        push(&mut found, address, link);
    }
    for usual in usual_places(&home) {
        push(&mut found, address, usual);
    }
    found.truncate(MAX_CANDIDATES);
    found
}

fn push(found: &mut Vec<String>, asked: &str, candidate: String) {
    // The page that was just read is not asked for again as its own feed.
    if candidate != asked && !found.contains(&candidate) {
        found.push(candidate);
    }
}

/// The feeds a page links to from its own markup, in the order it gives them.
///
/// A feed of the comments is left out. It is listed beside the feed of the
/// articles on most weblogs, and it is never what somebody pasting a site's
/// address meant.
fn feed_links(html: &str, home: &kobo_net::Address) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        let tail = &rest[open + 1..];
        let Some(close) = tail.find('>') else {
            break;
        };
        let inside = &tail[..close];
        rest = &tail[close + 1..];
        if kobo_html::element_name(inside) != "link" {
            continue;
        }
        let alternate = kobo_html::attribute(inside, "rel").is_some_and(|rel| {
            rel.split_whitespace()
                .any(|word| word.eq_ignore_ascii_case("alternate"))
        });
        let feed = kobo_html::attribute(inside, "type").is_some_and(|kind| {
            FEED_TYPES
                .iter()
                .any(|known| kind.trim().eq_ignore_ascii_case(known))
        });
        if !alternate || !feed {
            continue;
        }
        let Some(href) = kobo_html::attribute(inside, "href") else {
            continue;
        };
        let href = kobo_html::decode_entities(href.trim());
        let comments = [Some(href.as_str()), kobo_html::attribute(inside, "title")]
            .into_iter()
            .flatten()
            .any(|text| text.to_ascii_lowercase().contains("comment"));
        if comments {
            continue;
        }
        if let Ok(resolved) = kobo_net::resolve_redirect(home, &href) {
            if kobo_net::parse(&resolved).is_ok() {
                links.push(resolved);
            }
        }
    }
    links
}

/// The usual places for a feed, under the page's own path and then at the
/// root of its site.
///
/// Under the path first, because a publication on a shared host lives at a
/// path and its feed lives beside it, while the root there is the host's own
/// front page.
fn usual_places(home: &kobo_net::Address) -> Vec<String> {
    let origin = if home.port == 443 {
        format!("https://{}", home.host)
    } else {
        format!("https://{}:{}", home.host, home.port)
    };
    let path = home.path.split(['?', '#']).next().unwrap_or_default();
    let under = path.trim_end_matches('/');
    let mut places = Vec::new();
    if !under.is_empty() {
        places.extend(
            USUAL_PATHS[..2]
                .iter()
                .map(|usual| format!("{origin}{under}/{usual}")),
        );
    }
    places.extend(USUAL_PATHS.iter().map(|usual| format!("{origin}/{usual}")));
    places
}

/// The address of the site a typed name stands for, when the name is a host
/// and nothing else.
///
/// A name goes to the search service, which knows the feeds of a great many
/// sites. When it knows none for this one, the site itself can still be
/// asked, which is what the owner would have done next by hand.
#[must_use]
pub fn site_address(typed: &str) -> Option<String> {
    let typed = typed.trim();
    if typed.is_empty() || typed.contains("://") || typed.contains(char::is_whitespace) {
        return None;
    }
    let host = typed.split('/').next().unwrap_or_default();
    if !host.contains('.') {
        return None;
    }
    let address = format!("https://{typed}");
    kobo_net::parse(&address).ok().map(|_| address)
}

#[cfg(test)]
mod tests {
    use super::{candidates, site_address};

    #[test]
    fn the_feed_a_page_names_is_tried_before_the_usual_places() {
        let page = br#"<html><head><title>The Tide Letter</title>
            <link rel="stylesheet" href="/style.css">
            <link rel="alternate" type="application/rss+xml" href="/feed" title="The Tide Letter"/>
            </head><body><p>A letter about the sea.</p></body></html>"#;
        let found = candidates(page, "https://tide.example/");
        assert_eq!(found[0], "https://tide.example/feed");
        assert_eq!(found[1], "https://tide.example/rss");
        assert!(found.len() <= super::MAX_CANDIDATES);
    }

    #[test]
    fn a_feed_on_another_host_is_followed_because_the_page_itself_named_it() {
        let page = br#"<link rel="alternate" type="application/rss+xml"
            title="The Tide Letter Feed" href="https://feeds.example.net/tide?a=1&amp;b=2">"#;
        let found = candidates(page, "https://www.tide.example/about");
        assert_eq!(found[0], "https://feeds.example.net/tide?a=1&b=2");
    }

    #[test]
    fn the_feed_of_the_comments_is_never_offered_as_the_sites_feed() {
        let page = br#"
            <link rel="alternate" type="application/rss+xml" title="Tide &raquo; Comments Feed" href="https://tide.example/comments/feed/">
            <link rel="alternate" type="application/atom+xml" title="Tide" href="atom.xml">"#;
        let found = candidates(page, "https://tide.example/");
        assert_eq!(found[0], "https://tide.example/atom.xml");
        assert!(!found.iter().any(|address| address.contains("comments")));
    }

    #[test]
    fn links_that_are_not_feeds_or_not_fetchable_are_passed_over() {
        let page = br#"
            <link rel="alternate" hreflang="fr" href="https://tide.example/fr/">
            <link rel="alternate" type="application/rss+xml" href="ftp://tide.example/feed">
            <link rel="icon" type="application/rss+xml" href="/icon">
            <a rel="alternate" type="application/rss+xml" href="/anchor">Feed</a>"#;
        let found = candidates(page, "https://tide.example/");
        assert_eq!(found[0], "https://tide.example/feed", "{found:?}");
    }

    #[test]
    fn a_publication_at_a_path_is_looked_for_beside_itself_before_the_hosts_root() {
        let found = candidates(b"<html></html>", "https://letters.example/tide");
        assert_eq!(
            found,
            [
                "https://letters.example/tide/feed",
                "https://letters.example/tide/rss",
                "https://letters.example/feed",
                "https://letters.example/rss",
                "https://letters.example/feed.xml",
                "https://letters.example/atom.xml",
                "https://letters.example/rss.xml",
                "https://letters.example/index.xml",
            ]
        );
    }

    #[test]
    fn the_page_that_was_asked_for_is_not_proposed_as_its_own_feed() {
        let page = br#"<link rel="alternate" type="application/rss+xml" href="/feed">"#;
        let found = candidates(page, "https://tide.example/feed");
        assert!(!found.contains(&"https://tide.example/feed".to_owned()));
    }

    #[test]
    fn an_address_the_runtime_will_not_fetch_proposes_nothing() {
        assert!(candidates(b"", "http://tide.example/").is_empty());
        assert!(candidates(b"", "tide.example").is_empty());
    }

    #[test]
    fn only_a_bare_host_stands_for_a_site_to_ask_directly() {
        assert_eq!(
            site_address(" tide.example "),
            Some("https://tide.example".to_owned())
        );
        assert_eq!(
            site_address("letters.example/tide"),
            Some("https://letters.example/tide".to_owned())
        );
        for typed in ["tide", "the tide letter", "https://tide.example", ""] {
            assert_eq!(site_address(typed), None, "{typed}");
        }
    }
}
