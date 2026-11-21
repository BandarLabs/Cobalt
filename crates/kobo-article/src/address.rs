//! Web addresses: resolving the ones a page writes relative to itself.
//!
//! A page names its pictures however its authors liked: absolute, relative to
//! the page, relative to the site, or with the scheme left off. Every one of
//! them has to become a full address before it can be fetched, and the
//! transport only accepts the plain ASCII form, so this also escapes what a
//! browser would have escaped for us.

/// The scheme of `address`, lowercased, when it has one.
fn scheme_of(address: &str) -> Option<String> {
    let (scheme, _) = address.split_once(':')?;
    let mut characters = scheme.chars();
    let first = characters.next()?;
    let valid = first.is_ascii_alphabetic()
        && characters.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// The parts of an absolute address that resolution needs.
struct Parts<'a> {
    scheme: &'a str,
    authority: &'a str,
    path: &'a str,
    query: Option<&'a str>,
}

fn split(address: &str) -> Option<Parts<'_>> {
    let address = address.split('#').next().unwrap_or(address);
    let (scheme, rest) = address.split_once("://")?;
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_end);
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    Some(Parts {
        scheme,
        authority,
        path,
        query,
    })
}

/// Removes `.` and `..` segments the way RFC 3986 says to.
fn remove_dots(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    let trailing = path.ends_with("/.") || path.ends_with("/..") || path.ends_with('/');
    for segment in path.split('/') {
        match segment {
            "." | "" => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let mut out = String::from("/");
    out.push_str(&segments.join("/"));
    if trailing && !segments.is_empty() {
        out.push('/');
    }
    out
}

/// Resolves `reference` against the page at `base`.
///
/// Answers only `http` and `https` addresses, because nothing else is worth
/// fetching for a book: `data:` pictures are almost always the placeholder a
/// lazy page shows before the real one loads, and `javascript:` is never a
/// picture. The fragment is dropped, since a server never sees it.
#[must_use]
pub fn resolve(base: &str, reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    let reference = reference.split('#').next().unwrap_or(reference);
    let resolved = if let Some(scheme) = scheme_of(reference) {
        if scheme != "http" && scheme != "https" {
            return None;
        }
        reference.to_owned()
    } else {
        let base = split(base)?;
        if let Some(rest) = reference.strip_prefix("//") {
            format!("{}://{rest}", base.scheme)
        } else if reference.starts_with('/') {
            let (path, query) = match reference.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (reference, None),
            };
            with_query(
                &format!("{}://{}{}", base.scheme, base.authority, remove_dots(path)),
                query,
            )
        } else if let Some(query) = reference.strip_prefix('?') {
            let path = if base.path.is_empty() { "/" } else { base.path };
            format!("{}://{}{path}?{query}", base.scheme, base.authority)
        } else if reference.is_empty() {
            let path = if base.path.is_empty() { "/" } else { base.path };
            with_query(
                &format!("{}://{}{path}", base.scheme, base.authority),
                base.query,
            )
        } else {
            let directory = base.path.rfind('/').map_or("/", |at| &base.path[..=at]);
            let (path, query) = match reference.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (reference, None),
            };
            let merged = format!("{directory}{path}");
            with_query(
                &format!(
                    "{}://{}{}",
                    base.scheme,
                    base.authority,
                    remove_dots(&merged)
                ),
                query,
            )
        }
    };
    let scheme = scheme_of(&resolved)?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = split(&resolved)?.authority;
    if host.is_empty() {
        return None;
    }
    Some(escape(&resolved))
}

fn with_query(address: &str, query: Option<&str>) -> String {
    match query {
        Some(query) => format!("{address}?{query}"),
        None => address.to_owned(),
    }
}

/// Percent-escapes what a browser would escape before sending the address.
///
/// Pages write spaces and accented letters into addresses all the time and
/// browsers quietly fix them; the transport here parses strictly and would
/// refuse the picture instead.
fn escape(address: &str) -> String {
    let mut out = String::with_capacity(address.len());
    for byte in address.bytes() {
        if byte.is_ascii_graphic()
            && !matches!(
                byte,
                b'"' | b'<' | b'>' | b'\\' | b'^' | b'`' | b'{' | b'|' | b'}'
            )
        {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The address the transport can fetch: it speaks TLS only, so a plain
/// `http` address is asked for over `https`, which nearly every site serves.
#[must_use]
pub fn fetchable(address: &str) -> String {
    match address.strip_prefix("http://") {
        Some(rest) => format!("https://{rest}"),
        None => address.to_owned(),
    }
}

/// Where the whole paper is served as a web page, when `address` is a
/// paper's abstract page on arXiv.
///
/// arXiv gives every paper an abstract page under `/abs/` and, for the
/// papers it has been able to render, the paper itself under `/html/` at the
/// same identifier. Someone who sends an abstract page to their reader wants
/// the paper, and got a book of two hundred words. The arXiv app on the
/// reader goes from one to the other by the same rule.
///
/// Only the address is worked out here. Whether a rendering exists is found
/// by asking for it, and an identifier that does not look like one is left
/// alone rather than turned into a request.
#[must_use]
pub fn full_text(address: &str) -> Option<String> {
    if !is_web_address(address) || host_of(address)? != "arxiv.org" {
        return None;
    }
    let paper = split(address)?
        .path
        .strip_prefix("/abs/")?
        .trim_end_matches('/');
    // New identifiers are one part, `2406.01234v2`; old ones carry their
    // archive in front, `hep-th/9901001`.
    let parts: Vec<&str> = paper.split('/').collect();
    let plain = |part: &&str| {
        !part.is_empty()
            && part.len() <= 32
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    };
    (parts.len() <= 2 && parts.iter().all(plain)).then(|| format!("https://arxiv.org/html/{paper}"))
}

/// The host of an address without a leading `www.`, for naming a site that
/// does not name itself.
#[must_use]
pub fn host_of(address: &str) -> Option<String> {
    let parts = split(address)?;
    let host = parts
        .authority
        .rsplit('@')
        .next()
        .unwrap_or(parts.authority);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Whether `address` is an absolute `http` or `https` address with a host.
#[must_use]
pub fn is_web_address(address: &str) -> bool {
    matches!(scheme_of(address).as_deref(), Some("http" | "https"))
        && split(address).is_some_and(|parts| !parts.authority.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://news.example.org/2024/05/story.html?ref=home#top";

    #[test]
    fn a_relative_picture_is_found_beside_the_page() {
        assert_eq!(
            resolve(PAGE, "images/a.jpg").as_deref(),
            Some("https://news.example.org/2024/05/images/a.jpg")
        );
        assert_eq!(
            resolve(PAGE, "../../static/a.jpg?w=800").as_deref(),
            Some("https://news.example.org/static/a.jpg?w=800")
        );
    }

    #[test]
    fn a_site_relative_or_scheme_relative_picture_keeps_the_page_host_or_scheme() {
        assert_eq!(
            resolve(PAGE, "/media/a.png").as_deref(),
            Some("https://news.example.org/media/a.png")
        );
        assert_eq!(
            resolve(PAGE, "//cdn.example.net/a.png").as_deref(),
            Some("https://cdn.example.net/a.png")
        );
    }

    #[test]
    fn an_absolute_address_is_kept_and_its_fragment_dropped() {
        assert_eq!(
            resolve(PAGE, "http://other.example/a.gif#x").as_deref(),
            Some("http://other.example/a.gif")
        );
    }

    #[test]
    fn data_and_script_addresses_are_not_pictures_to_fetch() {
        assert_eq!(resolve(PAGE, "data:image/gif;base64,R0lGOD"), None);
        assert_eq!(resolve(PAGE, "javascript:void(0)"), None);
        assert_eq!(resolve(PAGE, "  "), None);
    }

    #[test]
    fn spaces_and_letters_outside_ascii_are_escaped_as_a_browser_would() {
        assert_eq!(
            resolve(PAGE, "/a photo/café.jpg").as_deref(),
            Some("https://news.example.org/a%20photo/caf%C3%A9.jpg")
        );
    }

    #[test]
    fn a_plain_http_address_is_fetched_over_tls() {
        assert_eq!(fetchable("http://a.example/x"), "https://a.example/x");
        assert_eq!(fetchable("https://a.example/x"), "https://a.example/x");
    }

    #[test]
    fn an_abstract_page_names_where_its_paper_is_served_in_full() {
        for (page, paper) in [
            (
                "https://arxiv.org/abs/2406.01234",
                Some("https://arxiv.org/html/2406.01234"),
            ),
            (
                "http://www.arxiv.org/abs/2406.01234v2?context=cs#top",
                Some("https://arxiv.org/html/2406.01234v2"),
            ),
            (
                "https://arxiv.org/abs/hep-th/9901001/",
                Some("https://arxiv.org/html/hep-th/9901001"),
            ),
            // Already the paper, a listing, another site, and identifiers
            // that are not ones.
            ("https://arxiv.org/html/2406.01234", None),
            ("https://arxiv.org/list/cs.CL/recent", None),
            ("https://example.org/abs/2406.01234", None),
            ("https://arxiv.org.example.net/abs/2406.01234", None),
            ("https://arxiv.org/abs/", None),
            ("https://arxiv.org/abs/a/b/c", None),
            ("https://arxiv.org/abs/2406.01234%2F..%2Fx", None),
            ("arxiv.org/abs/2406.01234", None),
        ] {
            assert_eq!(full_text(page).as_deref(), paper, "{page}");
        }
    }

    #[test]
    fn a_site_without_a_name_is_named_by_its_host() {
        assert_eq!(host_of(PAGE).as_deref(), Some("news.example.org"));
        assert_eq!(
            host_of("https://www.Example.com:8443/x").as_deref(),
            Some("example.com")
        );
        assert!(is_web_address(PAGE));
        assert!(!is_web_address("file:///tmp/page.html"));
        assert!(!is_web_address("story.html"));
    }
}
