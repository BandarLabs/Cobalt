//! Reviewed Miniflux routes, for an account bound to its owner's server and
//! for an owner-wide token.
//! No service implementation code is used; request contracts are documented at
//! <https://miniflux.app/docs/api.html>.
use kobo_json::Value;
use kobo_protocol::{Credential, CredentialUse, SecretHeader};

const MAX_BODY: usize = 8192;
const MAX_EXACT_ID: i64 = 9_007_199_254_740_991;

/// The requests Digest makes with an owner-wide token, and nothing more.
///
/// A token set with plain `kobo secret set miniflux` is bound to no server:
/// the owner types the address into Digest on the reader, so the destination
/// cannot be pinned the way a bound account's is, and the route has to carry
/// the whole limit instead. Each route here is one Digest sends, down to its
/// query keys and their values. The token can fetch the three lists a sync
/// collects, open an article, file a feed and mark what was read. It cannot
/// reach the account, its API keys, or any route Digest does not use.
///
/// Allowing only the unread list, as this once did, failed every sync: the
/// starred and read lists are part of each one, and a sync with a refused part
/// is abandoned whole.
pub(super) fn allowed_unbound(
    credential: &Credential,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if url.contains('#')
        || url.chars().any(char::is_control)
        || credential.secret != "miniflux"
        || !matches!(&credential.header, SecretHeader::Named(name) if name.eq_ignore_ascii_case("X-Auth-Token"))
    {
        return false;
    }
    let Ok(target) = kobo_net::parse(url) else {
        return false;
    };
    // Whatever precedes the API is the base path the owner typed, such as a
    // Miniflux served under `/reader`. It may not hide a query or a step back.
    let Some(start) = target.path.find("/v1/") else {
        return false;
    };
    let (base, route) = target.path.split_at(start);
    if base.contains(['%', '\\', '?'])
        || base.split('/').any(|part| matches!(part, "." | ".."))
        || route.contains(['%', '\\', '#'])
    {
        return false;
    }
    match usage {
        CredentialUse::Fetch => body.is_none() && content_type.is_none() && digest_read(route),
        CredentialUse::Put if route == "/v1/entries" => {
            content_type == Some("application/json") && body.is_some_and(entry_update)
        }
        CredentialUse::Post if route == "/v1/feeds" => {
            content_type == Some("application/json") && body.is_some_and(create_feed)
        }
        _ => false,
    }
}

/// The reads Digest makes: its lists, one article's full text, and the
/// categories a new feed can be filed under.
fn digest_read(route: &str) -> bool {
    if route == "/v1/categories" {
        return true;
    }
    if let Some(query) = route.strip_prefix("/v1/entries?") {
        return digest_list(query);
    }
    route
        .strip_prefix("/v1/entries/")
        .and_then(|entry| entry.strip_suffix("/fetch-content?update_content=false"))
        .is_some_and(|id| decimal(id, 1, MAX_EXACT_ID).is_some())
}

/// One of the three lists a sync collects: a depth, exactly one of unread,
/// starred or read, newest first.
fn digest_list(query: &str) -> bool {
    let mut seen = Vec::new();
    let mut lists = 0;
    for field in query.split('&') {
        let Some((name, value)) = field.split_once('=') else {
            return false;
        };
        if seen.contains(&name) {
            return false;
        }
        seen.push(name);
        let valid = match name {
            "limit" => decimal(value, 1, 100).is_some(),
            "status" => {
                lists += 1;
                matches!(value, "unread" | "read")
            }
            "starred" => {
                lists += 1;
                value == "true"
            }
            "order" => value == "published_at",
            "direction" => value == "desc",
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    lists == 1
        && ["limit", "order", "direction"]
            .iter()
            .all(|key| seen.contains(key))
}

pub(super) fn allowed(
    credential: &Credential,
    server: &str,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if url.contains('#')
        || url.chars().any(char::is_control)
        || credential.secret != "miniflux"
        || !matches!(&credential.header, SecretHeader::Named(name) if name.eq_ignore_ascii_case("X-Auth-Token"))
        || !super::servers::contains(server, url)
    {
        return false;
    }
    let (Ok(base), Ok(target)) = (kobo_net::parse(server), kobo_net::parse(url)) else {
        return false;
    };
    let Some(route) = target.path.strip_prefix(base.path.trim_end_matches('/')) else {
        return false;
    };
    if route.contains(['%', '\\', '#']) {
        return false;
    }
    match usage {
        CredentialUse::Fetch => body.is_none() && content_type.is_none() && read_route(route),
        CredentialUse::Put if route == "/v1/entries" => {
            content_type == Some("application/json") && body.is_some_and(entry_update)
        }
        CredentialUse::Post if route == "/v1/feeds" => {
            content_type == Some("application/json") && body.is_some_and(create_feed)
        }
        _ => false,
    }
}

fn read_route(route: &str) -> bool {
    if matches!(
        route,
        "/v1/me" | "/v1/version" | "/v1/feeds" | "/v1/categories"
    ) {
        return true;
    }
    if let Some(query) = route.strip_prefix("/v1/entries?") {
        return entry_query(query);
    }
    let Some(entry) = route.strip_prefix("/v1/entries/") else {
        return false;
    };
    let id = entry
        .strip_suffix("/fetch-content?update_content=false")
        .unwrap_or(entry);
    decimal(id, 1, MAX_EXACT_ID).is_some()
}

fn decimal(text: &str, minimum: i64, maximum: i64) -> Option<i64> {
    if text.is_empty() || text.len() > 16 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>()
        .ok()
        .filter(|number| (minimum..=maximum).contains(number))
}

fn entry_query(query: &str) -> bool {
    let mut seen = Vec::new();
    for field in query.split('&') {
        let Some((name, value)) = field.split_once('=') else {
            return false;
        };
        if seen.contains(&name) {
            return false;
        }
        seen.push(name);
        let valid = match name {
            "limit" => decimal(value, 1, 100).is_some(),
            "offset" => decimal(value, 0, 1_000_000).is_some(),
            "status" => matches!(value, "unread" | "read" | "removed"),
            "starred" => matches!(value, "true" | "false"),
            "order" => matches!(value, "id" | "published_at"),
            "direction" => matches!(value, "asc" | "desc"),
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    seen.contains(&"limit")
}

fn object(body: &str, names: &[&str]) -> Option<Vec<(String, Value)>> {
    if body.len() > MAX_BODY {
        return None;
    }
    let Value::Object(fields) = kobo_json::parse(body).ok()? else {
        return None;
    };
    let mut seen = Vec::new();
    for (name, _) in &fields {
        if !names.contains(&name.as_str()) || seen.contains(&name.as_str()) {
            return None;
        }
        seen.push(name.as_str());
    }
    Some(fields)
}

fn positive_id(value: &Value) -> Option<i64> {
    value.as_i64().filter(|id| (1..=MAX_EXACT_ID).contains(id))
}

fn entry_update(body: &str) -> bool {
    let Some(fields) = object(body, &["entry_ids", "status", "starred"]) else {
        return false;
    };
    let mut has_ids = false;
    let mut changes = 0;
    for (name, value) in fields {
        match name.as_str() {
            "entry_ids" => {
                let Some(ids) = value.as_array() else {
                    return false;
                };
                if ids.is_empty() || ids.len() > 100 {
                    return false;
                }
                let mut unique = Vec::new();
                for value in ids {
                    let Some(id) = positive_id(value) else {
                        return false;
                    };
                    if unique.contains(&id) {
                        return false;
                    }
                    unique.push(id);
                }
                has_ids = true;
            }
            "status" => {
                if !matches!(value.as_str(), Some("read" | "unread" | "removed")) {
                    return false;
                }
                changes += 1;
            }
            "starred" => {
                if value.as_bool().is_none() {
                    return false;
                }
                changes += 1;
            }
            _ => return false,
        }
    }
    has_ids && changes > 0
}

fn create_feed(body: &str) -> bool {
    let Some(fields) = object(body, &["feed_url", "category_id"]) else {
        return false;
    };
    let mut has_url = false;
    for (name, value) in fields {
        match name.as_str() {
            "feed_url" => {
                let Some(url) = value.as_str() else {
                    return false;
                };
                if url.len() > 4096
                    || url.contains('#')
                    || url.chars().any(char::is_control)
                    || kobo_net::parse(url).is_err()
                {
                    return false;
                }
                has_url = true;
            }
            "category_id" => {
                if positive_id(&value).is_none() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    has_url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::allowed_request_with_server;

    const SERVER: &str = "https://flux.example:8443/reader";
    fn request(route: &str, usage: CredentialUse, body: Option<&str>) -> bool {
        allowed_request_with_server(
            "rss-miniflux",
            &Credential::in_header("miniflux", "X-Auth-Token"),
            &format!("{SERVER}{route}"),
            usage,
            body,
            body.map(|_| "application/json"),
            Some(SERVER),
        )
    }

    #[test]
    fn bound_account_reads_only_reviewed_routes_and_bounded_queries() {
        for route in [
            "/v1/me",
            "/v1/version",
            "/v1/feeds",
            "/v1/categories",
            "/v1/entries/7",
            "/v1/entries/7/fetch-content?update_content=false",
            "/v1/entries?status=unread&limit=100&order=published_at&direction=desc",
            "/v1/entries?limit=20&starred=true&offset=20",
            "/v1/entries?limit=50&status=read&order=id&direction=asc",
        ] {
            assert!(request(route, CredentialUse::Fetch, None), "{route}");
        }
        for route in [
            "/v1/api-keys",
            "/v1/users",
            "/v1/me?extra=1",
            "/v1/entries",
            "/v1/entries?status=unread",
            "/v1/entries?limit=101",
            "/v1/entries?limit=0",
            "/v1/entries?limit=-1",
            "/v1/entries?limit=1&limit=2",
            "/v1/entries?limit=1&status=read&status=unread",
            "/v1/entries?limit=1&offset=1000001",
            "/v1/entries?limit=1&unknown=true",
            "/v1/entries?limit=1&status=%72ead",
            "/v1/entries/0",
            "/v1/entries/9007199254740992",
            "/v1/entries/7?foo=bar",
            "/v1/entries/7/fetch-content?update_content=true",
            "/v1/entries/7/fetch-content",
            "/v1/entries/7/bookmark",
            "/v1/entries/7/../me",
            "/v1/entries/%37",
        ] {
            assert!(!request(route, CredentialUse::Fetch, None), "{route}");
        }
        assert!(!request("/v1/me", CredentialUse::Fetch, Some("{}")));
    }

    #[test]
    fn updates_require_explicit_bounded_desired_values() {
        for body in [
            r#"{"entry_ids":[7],"status":"read"}"#,
            r#"{"entry_ids":[7,8],"starred":false}"#,
            r#"{"entry_ids":[7],"status":"unread","starred":true}"#,
            r#"{"entry_ids":[7],"status":"removed"}"#,
        ] {
            assert!(
                request("/v1/entries", CredentialUse::Put, Some(body)),
                "{body}"
            );
            assert!(!request("/v1/entries", CredentialUse::Post, Some(body)));
            assert!(!request("/v1/entries", CredentialUse::Patch, Some(body)));
            assert!(!request(
                "/v1/entries/7/bookmark",
                CredentialUse::Put,
                Some(body)
            ));
        }
        for body in [
            "{}",
            "[]",
            r#"{"entry_ids":[7]}"#,
            r#"{"status":"read"}"#,
            r#"{"entry_ids":[],"status":"read"}"#,
            r#"{"entry_ids":[0],"status":"read"}"#,
            r#"{"entry_ids":[-1],"status":"read"}"#,
            r#"{"entry_ids":[1.5],"status":"read"}"#,
            r#"{"entry_ids":[9007199254740993],"status":"read"}"#,
            r#"{"entry_ids":[7,7],"status":"read"}"#,
            r#"{"entry_ids":["7"],"status":"read"}"#,
            r#"{"entry_ids":[7],"status":"read","status":"removed"}"#,
            r#"{"entry_ids":[7],"status":"read","\u0073tatus":"removed"}"#,
            r#"{"entry_ids":[7],"starred":"true"}"#,
            r#"{"entry_ids":[7],"status":"archived"}"#,
            r#"{"entry_ids":[7],"starred":true,"title":"replace"}"#,
        ] {
            assert!(
                !request("/v1/entries", CredentialUse::Put, Some(body)),
                "{body}"
            );
        }
        let ids = (1..=101)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        assert!(!request(
            "/v1/entries",
            CredentialUse::Put,
            Some(&format!(r#"{{"entry_ids":[{ids}],"status":"read"}}"#))
        ));
        assert!(!request(
            "/v1/entries",
            CredentialUse::Put,
            Some(&" ".repeat(MAX_BODY + 1))
        ));
    }

    #[test]
    fn feed_creation_cannot_add_hidden_options_or_other_mutations() {
        for body in [
            r#"{"feed_url":"https://example.org/feed.xml"}"#,
            r#"{"feed_url":"https://example.org/feed.xml","category_id":7}"#,
        ] {
            assert!(request("/v1/feeds", CredentialUse::Post, Some(body)));
            assert!(!request("/v1/feeds", CredentialUse::Put, Some(body)));
            assert!(!request("/v1/feeds/7", CredentialUse::Post, Some(body)));
        }
        for body in [
            "{}",
            r#"{"feed_url":"http://example.org/feed"}"#,
            r#"{"feed_url":"https://example.org/feed#fragment"}"#,
            r#"{"feed_url":"https://user:password@example.org/feed"}"#,
            r#"{"feed_url":"https://example.org/feed","category_id":0}"#,
            r#"{"feed_url":"https://example.org/feed","username":"secret"}"#,
            r#"{"feed_url":"https://example.org/feed","feed_url":"https://other.org/feed"}"#,
            r#"{"feed_url":"https://example.org/feed","fetch_via_proxy":true}"#,
        ] {
            assert!(
                !request("/v1/feeds", CredentialUse::Post, Some(body)),
                "{body}"
            );
        }
    }

    #[test]
    fn token_never_escapes_its_saved_server_app_or_header_convention() {
        let token = Credential::in_header("miniflux", "X-Auth-Token");
        for url in [
            "https://other.example:8443/reader/v1/me",
            "https://flux.example/reader/v1/me",
            "http://flux.example:8443/reader/v1/me",
            "https://flux.example:8443/v1/me",
            "https://flux.example:8443/reader-other/v1/me",
            "https://flux.example:8443/reader/../v1/me",
            "https://flux.example:8443/reader/%2e%2e/v1/me",
            "https://flux.example:8443/reader/v1/me#fragment",
        ] {
            assert!(
                !allowed(&token, SERVER, url, CredentialUse::Fetch, None, None),
                "{url}"
            );
        }
        for credential in [
            Credential::basic("miniflux"),
            Credential::bearer("miniflux"),
            Credential::in_header("other", "X-Auth-Token"),
        ] {
            assert!(!allowed(
                &credential,
                SERVER,
                &format!("{SERVER}/v1/me"),
                CredentialUse::Fetch,
                None,
                None
            ));
        }
        assert!(!allowed_request_with_server(
            "panels",
            &token,
            &format!("{SERVER}/v1/me"),
            CredentialUse::Fetch,
            None,
            None,
            Some(SERVER)
        ));
        assert!(!allowed(
            &token,
            SERVER,
            &format!("{SERVER}/v1/entries"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#),
            Some("text/plain")
        ));
        // Without a bound server the same write is held to its body and type.
        assert!(!allowed_request_with_server(
            "rss-miniflux",
            &token,
            &format!("{SERVER}/v1/entries"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#),
            Some("text/plain"),
            None
        ));
        assert!(!allowed_request_with_server(
            "rss-miniflux",
            &token,
            &format!("{SERVER}/v1/entries"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read","title":"overwrite"}"#),
            Some("application/json"),
            None
        ));
    }

    /// The request as Digest sends it with a token set by plain
    /// `kobo secret set miniflux`, which binds no server.
    fn unbound(url: &str, usage: CredentialUse, body: Option<&str>) -> bool {
        allowed_request_with_server(
            "rss-miniflux",
            &Credential::in_header("miniflux", "X-Auth-Token"),
            url,
            usage,
            body,
            body.map(|_| "application/json"),
            None,
        )
    }

    #[test]
    fn every_request_digest_makes_with_an_owner_wide_token_is_allowed() {
        // Spelled exactly as `apps/rss-miniflux/src/miniflux.rs` builds them,
        // against a server at the root and one under a base path.
        for server in ["https://flux.example", SERVER] {
            for (depth, list) in [
                (60, "status=unread"),
                (20, "starred=true"),
                (20, "status=read"),
            ] {
                let url = format!(
                    "{server}/v1/entries?limit={depth}&{list}&order=published_at&direction=desc"
                );
                assert!(unbound(&url, CredentialUse::Fetch, None), "{url}");
            }
            for route in [
                "/v1/entries/7/fetch-content?update_content=false",
                "/v1/categories",
            ] {
                let url = format!("{server}{route}");
                assert!(unbound(&url, CredentialUse::Fetch, None), "{url}");
            }
            let feeds = format!("{server}/v1/feeds");
            assert!(unbound(
                &feeds,
                CredentialUse::Post,
                Some(r#"{"feed_url":"https://example.org/feed.xml","category_id":3}"#)
            ));
            let entries = format!("{server}/v1/entries");
            for body in [
                r#"{"entry_ids":[7],"status":"read"}"#,
                r#"{"entry_ids":[7],"status":"unread"}"#,
                r#"{"entry_ids":[7],"status":"removed"}"#,
                r#"{"entry_ids":[7],"starred":true}"#,
                r#"{"entry_ids":[7],"starred":false}"#,
            ] {
                assert!(unbound(&entries, CredentialUse::Put, Some(body)), "{body}");
            }
        }
    }

    #[test]
    fn an_owner_wide_token_cannot_reach_any_route_digest_does_not_use() {
        for route in [
            "/v1/me",
            "/v1/version",
            "/v1/users",
            "/v1/api-keys",
            "/v1/feeds",
            "/v1/entries",
            "/v1/entries/7",
            "/v1/entries?status=unread",
            "/v1/entries?limit=60&status=unread",
            "/v1/entries?limit=60&order=published_at&direction=desc",
            "/v1/entries?limit=60&status=unread&starred=true&order=published_at&direction=desc",
            "/v1/entries?limit=60&status=removed&order=published_at&direction=desc",
            "/v1/entries?limit=60&starred=false&order=published_at&direction=desc",
            "/v1/entries?limit=101&status=unread&order=published_at&direction=desc",
            "/v1/entries?limit=60&status=unread&order=id&direction=desc",
            "/v1/entries?limit=60&status=unread&order=published_at&direction=asc",
            "/v1/entries?limit=60&status=unread&order=published_at&direction=desc&offset=60",
            "/v1/entries?limit=60&limit=1&status=unread&order=published_at&direction=desc",
            "/v1/entries?limit=60&status=%75nread&order=published_at&direction=desc",
            "/v1/entries/7/fetch-content?update_content=true",
            "/v1/entries/7/fetch-content",
            "/v1/entries/0/fetch-content?update_content=false",
            "/v1/entries/7/bookmark",
            "/v1/categories/3/entries",
        ] {
            let url = format!("{SERVER}{route}");
            assert!(!unbound(&url, CredentialUse::Fetch, None), "{url}");
        }
        for url in [
            "https://flux.example/../v1/categories",
            "https://flux.example/%2e%2e/v1/categories",
            "https://flux.example/reader?x=/v1/categories",
            "http://flux.example/v1/categories",
            "https://flux.example/v1/categories#me",
        ] {
            assert!(!unbound(url, CredentialUse::Fetch, None), "{url}");
        }
        let entries = format!("{SERVER}/v1/entries");
        let read = r#"{"entry_ids":[7],"status":"read"}"#;
        assert!(!unbound(&entries, CredentialUse::Post, Some(read)));
        assert!(!unbound(&entries, CredentialUse::Patch, Some(read)));
        assert!(!unbound(
            &format!("{SERVER}/v1/entries/7/bookmark"),
            CredentialUse::Put,
            Some(read)
        ));
        assert!(!unbound(
            &format!("{SERVER}/v1/feeds"),
            CredentialUse::Post,
            Some(r#"{"feed_url":"https://example.org/feed","username":"secret"}"#)
        ));
        assert!(!unbound(
            &format!("{SERVER}/v1/me"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#)
        ));
        // The same routes with any other credential name or header are refused.
        for credential in [
            Credential::in_header("other", "X-Auth-Token"),
            Credential::bearer("miniflux"),
        ] {
            assert!(!allowed_request_with_server(
                "rss-miniflux",
                &credential,
                &format!("{SERVER}/v1/categories"),
                CredentialUse::Fetch,
                None,
                None,
                None
            ));
        }
    }
}
