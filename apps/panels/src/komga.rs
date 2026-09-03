//! OPDS plumbing shared by Komga's root, series, and volume feeds.

use kobo_opds::{Feed, Publication};
use kobo_sdk::{Credential, Task};

pub const CATALOG: &str = "https://komga.local/opds/v1.2/catalog";
pub const MAX_FEED_BYTES: u32 = 256 * 1024;

pub fn fetch(url: String) -> Task {
    Task::Fetch {
        url,
        offset: 0,
        max_bytes: MAX_FEED_BYTES,
        credential: Some(Credential::basic("komga")),
        headers: Vec::new(),
    }
}

pub fn cbz(publication: &Publication) -> Option<String> {
    publication
        .acquisition
        .iter()
        .find(|item| {
            item.available
                && (std::path::Path::new(&item.href)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("cbz"))
                    || item
                        .media_type
                        .as_deref()
                        .is_some_and(|kind| kind.contains("zip")))
        })
        .map(|item| item.href.clone())
}

pub fn parse(bytes: &[u8], base: &str) -> Result<Feed, String> {
    kobo_opds::parse(bytes, base).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{fetch, parse, CATALOG};
    use kobo_sdk::{Credential, Task};

    #[test]
    fn every_komga_request_uses_the_runtime_basic_secret() {
        let Task::Fetch {
            credential: Some(Credential { secret, .. }),
            ..
        } = fetch(CATALOG.to_owned())
        else {
            panic!("Komga fetch");
        };
        assert_eq!(secret, "komga");
    }

    #[test]
    fn atom_series_feed_is_parsed_by_the_shared_opds_reader() {
        let feed = parse(
            br#"<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Series</title><entry><title>Volume 1</title><link rel="http://opds-spec.org/acquisition/open-access" href="one.cbz" type="application/x-cbz"/></entry></feed>"#,
            CATALOG,
        )
        .expect("feed");
        assert_eq!(feed.publications[0].title, "Volume 1");
    }
}
