//! Reviewed Miniflux routes for an account bound to its owner's server.
//! No service implementation code is used; request contracts are documented at
//! <https://miniflux.app/docs/api.html>.
use kobo_json::Value;
use kobo_protocol::{Credential, CredentialUse, SecretHeader};

const MAX_BODY: usize = 8192;
const MAX_EXACT_ID: i64 = 9_007_199_254_740_991;

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
        assert!(!allowed_request_with_server(
            "rss-miniflux",
            &token,
            &format!("{SERVER}/v1/entries"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#),
            Some("application/json"),
            None
        ));
    }
}
