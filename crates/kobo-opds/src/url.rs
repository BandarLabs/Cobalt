//! The part of this crate that decides whether a URL a catalog wrote down is
//! allowed to become a request.
//!
//! A catalog is a stranger. Every href in a feed — an acquisition link, a
//! cover image, a `next` page — is a value the server chose, and OPDS's own
//! test catalogs write most of them as relative references (`../larger.jpg`,
//! `main.xml`) that only mean something resolved against the address the feed
//! was fetched from. So this module does two things that have to happen
//! together: it resolves a reference the way RFC 3986 says to, and it refuses
//! to hand back anything that resolution produces which is not `https`. A
//! parser that resolved URLs but trusted whatever scheme came out the other
//! end would cheerfully hand the runtime `javascript:alert(1)` or
//! `file:///etc/passwd`, because both of those are perfectly well-formed
//! absolute references — [`hostile.xml`] in the fixtures carries exactly
//! those two, plus `http:` and a `data:` URI standing in for an image.
//!
//! [`hostile.xml`]: ../../tests/fixtures/opds1/hostile.xml

/// Resolves `href` against `base` per RFC 3986 §5, and returns nothing unless
/// the result is `https`.
///
/// This is the one function nearly every href in this crate passes through.
/// Combining resolution with the scheme check in a single call is
/// deliberate: a caller that resolved first and remembered to check the
/// scheme second would only need to forget the second step once for a
/// catalog to walk the reader off of `https`.
#[must_use]
pub fn safe_href(base: &str, href: &str) -> Option<String> {
    let resolved = resolve(base, href)?;
    is_https(&resolved).then_some(resolved)
}

/// True when `url` starts with the `https:` scheme, case-insensitively.
///
/// `http:`, `javascript:`, `file:` and every other scheme are refused for the
/// same reason: a link that is not `https` is a link this runtime will not
/// fetch, so returning it as though it were fetchable would cost the reader a
/// tap that was always going to fail — or, for `javascript:` and `file:`,
/// would fail in a way worth not attempting at all.
#[must_use]
pub fn is_https(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

/// True when `a` and `b` share a scheme, host and port.
///
/// Exported so the application can decide whether to follow a `next` link:
/// nothing in this crate enforces that a paginated catalog stays on the host
/// the reader pointed it at, because refusing a page turn is a product
/// decision, not a parsing one. What this crate owns is giving a correct
/// answer to "are these the same place," which is easy to get subtly wrong by
/// comparing whole strings (`http://x` and `https://x` are not the same
/// origin; `example.com:443` and `example.com` might not be, depending on the
/// scheme's default port, which is why this compares the parsed parts rather
/// than prefixes of the raw text).
#[must_use]
pub fn same_origin(a: &str, b: &str) -> bool {
    match (split(a), split(b)) {
        (Some(a), Some(b)) => {
            a.scheme.eq_ignore_ascii_case(b.scheme)
                && without_default_port(a.scheme, a.authority)
                    .eq_ignore_ascii_case(&without_default_port(b.scheme, b.authority))
        }
        _ => false,
    }
}

/// An authority with the scheme's own default port taken off.
///
/// `https://example.com` and `https://example.com:443` are the same origin,
/// and a catalog that writes one in its feed and the other in a paging link is
/// writing about itself both times. Comparing the authorities as written made
/// that catalog look like it was sending the reader somewhere else, which ends
/// the shelf.
fn without_default_port<'a>(scheme: &str, authority: &'a str) -> std::borrow::Cow<'a, str> {
    let default = if scheme.eq_ignore_ascii_case("https") {
        ":443"
    } else if scheme.eq_ignore_ascii_case("http") {
        ":80"
    } else {
        return std::borrow::Cow::Borrowed(authority);
    };
    authority
        .strip_suffix(default)
        .map_or(std::borrow::Cow::Borrowed(authority), |host| {
            std::borrow::Cow::Borrowed(host)
        })
}

/// The generic-syntax pieces of an absolute URL that resolution needs.
///
/// Nothing here percent-decodes or understands userinfo; none of the
/// catalogs this crate reads use either, and a component this crate does not
/// need is a component that cannot be gotten wrong.
struct Parts<'a> {
    scheme: &'a str,
    authority: &'a str,
    path: &'a str,
    query: Option<&'a str>,
}

fn split(url: &str) -> Option<Parts<'_>> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || !scheme.as_bytes()[0].is_ascii_alphabetic() {
        return None;
    }
    let before_fragment = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (authority_and_path, query) = match before_fragment.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (before_fragment, None),
    };
    let (authority, path) = match authority_and_path.find('/') {
        Some(index) => (&authority_and_path[..index], &authority_and_path[index..]),
        None => (authority_and_path, ""),
    };
    Some(Parts {
        scheme,
        authority,
        path,
        query,
    })
}

/// True when `text` opens with an RFC 3986 scheme (`ALPHA *( ALPHA / DIGIT /
/// "+" / "-" / "." ) ":"`), which is how an absolute reference is told apart
/// from a relative one.
///
/// This is what catches `javascript:alert(1)` and `file:///etc/passwd`: both
/// have a scheme, both are therefore absolute references that get returned
/// as-is rather than resolved against `base`, and both then fail the
/// `https`-only check in [`safe_href`]. A resolver that only recognised
/// `http`/`https` explicitly would need updating every time a new scheme
/// showed up in a hostile feed; testing the grammar instead needs no updates.
fn has_scheme(text: &str) -> bool {
    let mut chars = text.char_indices();
    let Some((_, first)) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    for (index, character) in chars {
        match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '+' | '-' | '.' => {}
            ':' => return index > 0,
            _ => return false,
        }
    }
    false
}

/// Resolves `href` against `base`, following RFC 3986 §5.3's reference
/// transformation closely enough for the shapes OPDS catalogs actually write:
/// an absolute URL, a scheme-relative `//host/path`, a root-relative `/path`,
/// a query-only `?q=1`, a fragment-only `#f`, or a same-directory relative
/// path possibly carrying `..` segments.
///
/// Returns `None` when `base` is not itself an absolute URL this can parse,
/// which should never happen in practice since `base` is always the address
/// the feed was fetched from, but a parser that panicked on a caller's typo
/// would be a worse failure than one that says nothing.
#[must_use]
fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    if has_scheme(href) {
        return Some(href.to_owned());
    }
    let base = split(base)?;

    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("{}://{rest}", base.scheme));
    }
    if let Some(rest) = href.strip_prefix('/') {
        return Some(format!("{}://{}/{rest}", base.scheme, base.authority));
    }
    if let Some(rest) = href.strip_prefix('?') {
        let path = if base.path.is_empty() { "/" } else { base.path };
        return Some(format!("{}://{}{path}?{rest}", base.scheme, base.authority));
    }
    if let Some(rest) = href.strip_prefix('#') {
        let path = if base.path.is_empty() { "/" } else { base.path };
        let query = base.query.map(|q| format!("?{q}")).unwrap_or_default();
        return Some(format!(
            "{}://{}{path}{query}#{rest}",
            base.scheme, base.authority
        ));
    }

    let (href_path, href_rest) = match href.split_once('#') {
        Some((before, fragment)) => (before, format!("#{fragment}")),
        None => (href, String::new()),
    };
    let (href_path, href_query) = match href_path.split_once('?') {
        Some((before, query)) => (before, format!("?{query}")),
        None => (href_path, String::new()),
    };

    let directory = match base.path.rfind('/') {
        Some(index) => &base.path[..=index],
        None => "/",
    };
    let merged = format!("{directory}{href_path}");
    let normalized = remove_dot_segments(&merged);
    Some(format!(
        "{}://{}{normalized}{href_query}{href_rest}",
        base.scheme, base.authority
    ))
}

/// RFC 3986 §5.2.4: collapses `.` and `..` segments out of a merged path.
///
/// The two test catalogs lean on this throughout — `acquisition/../larger.jpg`
/// has to become `larger.jpg` sitting next to `acquisition/`, not a literal
/// path segment called `..` that no server will answer.
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." => {}
            ".." => {
                // `output[0]` is always the empty string standing for the
                // leading `/` (every path this function is given starts with
                // one), and popping it would turn an absolute path relative,
                // which is not what an excess of `..` is meant to do — it is
                // meant to do nothing further, the way a shell's `cd ..` at
                // `/` stays at `/`.
                if output.len() > 1 {
                    output.pop();
                }
            }
            segment => output.push(segment),
        }
    }
    if output.is_empty() {
        "/".to_owned()
    } else {
        output.join("/")
    }
}

/// The most bytes accepted out of a `data:` URI's base64 payload before
/// decoding.
///
/// OPDS 1.2 blesses inline images (§5.2.2) and Gutenberg uses them for real —
/// its navigation thumbnails are 22×22 icons a few hundred bytes long — but
/// nothing stops a hostile feed from inlining a multi-megabyte image into
/// every one of a thousand entries instead of linking it, trading a bounded
/// number of small HTTP responses this crate never has to fetch for one
/// unbounded document it does. This is generous enough for a real cover image
/// (a few hundred KB) and small enough that even [`MAX_ENTRIES`](crate::MAX_ENTRIES)
/// copies of it stay well short of exhausting a 512 MiB device.
pub const MAX_INLINE_IMAGE_BASE64_CHARS: usize = 300_000;

/// The `data:` image types this crate will decode.
///
/// Matches what `kobo-image` can turn into pixels. `data:text/html` — the
/// shape [`hostile.xml`] tries — is rejected here for the same reason a
/// non-`https` link is rejected in [`safe_href`]: decoding it would hand the
/// runtime a document to interpret as markup, which is precisely what a
/// cover thumbnail must never be.
///
/// [`hostile.xml`]: ../../tests/fixtures/opds1/hostile.xml
const INLINE_IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// Decodes a `data:` URI into its media type and bytes, or refuses it.
///
/// Refused, rather than decoded: anything that is not `;base64`, any media
/// type outside [`INLINE_IMAGE_TYPES`], anything over
/// [`MAX_INLINE_IMAGE_BASE64_CHARS`], and anything whose payload is not valid
/// base64. This is the only place in the crate that ever looks at a `data:`
/// URI's payload — everywhere else, `data:` is just another scheme that isn't
/// `https` and so isn't a link.
#[must_use]
pub fn decode_data_image(href: &str) -> Option<(String, Vec<u8>)> {
    let rest = href.strip_prefix("data:")?;
    let (meta, data) = rest.split_once(',')?;
    let media_type = meta.strip_suffix(";base64")?;
    if !INLINE_IMAGE_TYPES.contains(&media_type) {
        return None;
    }
    if data.len() > MAX_INLINE_IMAGE_BASE64_CHARS {
        return None;
    }
    let bytes = base64_decode(data)?;
    Some((media_type.to_owned(), bytes))
}

/// Standard (not URL-safe) base64, the only alphabet a `data:` URI uses.
///
/// Written by hand because the workspace's crates.io dependencies live in
/// `kobo-net` alone (see `kobo-json`'s and `kobo-xml`'s own `Cargo.toml`), and
/// decoding a few kilobytes of base64 is a short enough function that a crate
/// for it would cost more in supply chain than it saves in code.
fn base64_decode(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let text = text.trim_end_matches('=');
    let bytes = text.as_bytes();
    // A final group of one character encodes six bits, which is not a byte.
    // RFC 4648 has no such encoding, so a payload ending that way was built by
    // something that did not know what it was doing -- and this is the one
    // place a `data:` URI's own bytes are believed, so it is refused rather
    // than half-decoded.
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4 + 3);
    for chunk in bytes.chunks(4) {
        let mut values = [0u8; 4];
        for (slot, byte) in values.iter_mut().zip(chunk) {
            *slot = value(*byte)?;
        }
        let count = chunk.len();
        out.push((values[0] << 2) | (values[1] >> 4));
        if count > 2 {
            out.push((values[1] << 4) | (values[2] >> 2));
        }
        if count > 3 {
            out.push((values[2] << 6) | values[3]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{decode_data_image, is_https, resolve, safe_href, same_origin};

    #[test]
    fn a_relative_href_resolves_against_the_page_it_was_written_on() {
        let base = "https://example.org/opds1/acquisition/main.xml";
        assert_eq!(
            resolve(base, "../larger.jpg").as_deref(),
            Some("https://example.org/opds1/larger.jpg")
        );
        assert_eq!(
            resolve(base, "page2.xml").as_deref(),
            Some("https://example.org/opds1/acquisition/page2.xml")
        );
        assert_eq!(
            resolve(base, "/root.xml").as_deref(),
            Some("https://example.org/root.xml")
        );
        assert_eq!(
            resolve(base, "?start_index=26").as_deref(),
            Some("https://example.org/opds1/acquisition/main.xml?start_index=26")
        );
        assert_eq!(
            resolve(base, "#fragment").as_deref(),
            Some("https://example.org/opds1/acquisition/main.xml#fragment")
        );
        assert_eq!(
            resolve(base, "https://elsewhere.example/x").as_deref(),
            Some("https://elsewhere.example/x")
        );
    }

    #[test]
    fn a_link_that_is_not_https_never_becomes_something_to_fetch() {
        let base = "https://catalog.example/hostile.xml";
        assert_eq!(safe_href(base, "http://catalog.example/plain.epub"), None);
        assert_eq!(safe_href(base, "javascript:alert(1)"), None);
        assert_eq!(safe_href(base, "file:///etc/passwd"), None);
        assert_eq!(
            safe_href(base, "https://catalog.example/ok.epub").as_deref(),
            Some("https://catalog.example/ok.epub")
        );
    }

    #[test]
    fn is_https_only_accepts_the_https_scheme() {
        assert!(is_https("https://example.com/"));
        assert!(!is_https("http://example.com/"));
        assert!(is_https("HTTPS://example.com/"));
        assert!(!is_https("data:image/png;base64,AAAA"));
    }

    #[test]
    fn same_origin_compares_scheme_and_authority_not_whole_strings() {
        assert!(same_origin(
            "https://example.com/a",
            "https://example.com/b?x=1"
        ));
        assert!(!same_origin(
            "https://example.com/a",
            "http://example.com/a"
        ));
        assert!(!same_origin(
            "https://example.com/a",
            "https://elsewhere.example/a"
        ));
    }

    #[test]
    fn a_default_port_written_out_is_the_same_origin_as_one_left_off() {
        // A catalog that writes itself one way in its feed and the other way
        // in a paging link is writing about itself both times, and comparing
        // the authorities as written ended the shelf at page one.
        assert!(same_origin(
            "https://example.com/catalog",
            "https://example.com:443/catalog?page=2"
        ));
        assert!(same_origin(
            "http://example.com/a",
            "http://example.com:80/b"
        ));
        // A port that is not the scheme's own is a different origin, and the
        // https default is not the http one.
        assert!(!same_origin(
            "https://example.com/a",
            "https://example.com:8443/b"
        ));
        assert!(!same_origin(
            "https://example.com/a",
            "https://example.com:80/b"
        ));
    }

    #[test]
    fn a_base64_payload_ending_in_a_lone_character_is_refused() {
        // Six bits is not a byte, and RFC 4648 has no encoding that ends that
        // way. Decoding it anyway made a malformed `data:` URI look valid.
        assert!(decode_data_image("data:image/png;base64,QQ==").is_some());
        assert!(decode_data_image("data:image/png;base64,Q").is_none());
        // Six characters is two whole bytes and a valid tail; five is one
        // byte and a stray six bits, which is the shape being refused.
        assert!(decode_data_image("data:image/png;base64,QUJJRA==").is_some());
        assert!(decode_data_image("data:image/png;base64,QUJJR").is_none());
    }

    #[test]
    fn a_data_uri_that_is_not_an_image_the_device_decodes_is_dropped() {
        assert_eq!(
            decode_data_image("data:text/html;base64,PHNjcmlwdD4="),
            None
        );
        assert_eq!(decode_data_image("data:image/png,notbase64"), None);
        assert_eq!(
            decode_data_image(&format!(
                "data:image/png;base64,{}",
                "A".repeat(super::MAX_INLINE_IMAGE_BASE64_CHARS + 1)
            )),
            None
        );
    }

    #[test]
    fn a_data_uri_that_is_an_accepted_image_type_decodes_to_its_bytes() {
        // "hi" in base64.
        let (media_type, bytes) = decode_data_image("data:image/gif;base64,aGk=").expect("decodes");
        assert_eq!(media_type, "image/gif");
        assert_eq!(bytes, b"hi");
    }
}
