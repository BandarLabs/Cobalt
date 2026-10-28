//! Readeck requests bound to an owner-entered HTTPS server and API token.
//! Deliberately no unbound token, OAuth, annotation or arbitrary write access.
use kobo_protocol::{Credential, CredentialUse, SecretHeader};

pub(super) fn allowed(
    credential: &Credential,
    server: &str,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if credential.secret != "readeck"
        || credential.header != SecretHeader::Bearer
        || url.contains('#')
        || url.chars().any(char::is_control)
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
    match usage {
        CredentialUse::Fetch => body.is_none() && content_type.is_none() && read_route(route),
        CredentialUse::Patch => {
            content_type == Some("application/json")
                && route
                    .strip_prefix("/api/bookmarks/")
                    .is_some_and(bookmark_id)
                && body.is_some_and(|body| {
                    matches!(
                        body,
                        r#"{"read_progress":100}"#
                            | r#"{"is_archived":true}"#
                            | r#"{"is_marked":true}"#
                            | r#"{"is_marked":false}"#
                            | r#"{"is_deleted":true}"#
                    )
                })
        }
        _ => false,
    }
}

fn bookmark_id(id: &str) -> bool {
    (18..=22).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn read_route(route: &str) -> bool {
    if route == "/api/bookmarks?limit=50&offset=0&is_archived=false&read_status=unread&read_status=reading&type=article&type=photo&sort=-created" {
        return true;
    }
    if route
        .strip_prefix("/api/bookmarks/")
        .and_then(|path| path.strip_suffix("/article"))
        .is_some_and(bookmark_id)
    {
        return true;
    }
    search_route(route)
}

// Only the app's bounded, canonical UTF-8 search format. Escaped characters
// belong to one query value, never to a route or parameter name.
fn search_route(route: &str) -> bool {
    let Some(rest) = route.strip_prefix("/api/bookmarks?limit=50&offset=") else {
        return false;
    };
    let Some((offset, query)) = rest.split_once("&sort=-created&search=") else {
        return false;
    };
    let Ok(number) = offset.parse::<u32>() else {
        return false;
    };
    if number % 50 != 0 || number.to_string() != offset || query.is_empty() || query.len() > 768 {
        return false;
    }
    let unreserved =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
    let hex = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    };
    let mut decoded = Vec::new();
    let mut bytes = query.bytes();
    while let Some(byte) = bytes.next() {
        if unreserved(byte) {
            decoded.push(byte);
        } else if byte == b'%' {
            let Some(high) = bytes.next().and_then(hex) else {
                return false;
            };
            let Some(low) = bytes.next().and_then(hex) else {
                return false;
            };
            let value = high * 16 + low;
            if unreserved(value) {
                return false;
            }
            decoded.push(value);
        } else {
            return false;
        }
    }
    std::str::from_utf8(&decoded).is_ok_and(|query| {
        query.len() <= 256
            && !query.is_empty()
            && query.trim() == query
            && !query.chars().any(char::is_control)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::{allowed_request_with_server, servers};
    const SERVER: &str = "https://read.example:8443/library";
    const ARTICLE: &str = "/api/bookmarks/0123456789ABCDEFGH/article";
    const BOOKMARK: &str = "/api/bookmarks/0123456789ABCDEFGH";
    fn request(route: &str, usage: CredentialUse, body: Option<&str>, mime: Option<&str>) -> bool {
        allowed_request_with_server(
            "readeck",
            &Credential::bearer("readeck"),
            &format!("{SERVER}{route}"),
            usage,
            body,
            mime,
            Some(SERVER),
        )
    }
    #[test]
    fn readeck_account_is_explicit_and_server_bound() {
        assert!(servers::may_set("readeck", "readeck"));
        assert!(!servers::may_set("other", "readeck"));
        assert!(!servers::may_set("readeck", "other"));
        assert!(request(ARTICLE, CredentialUse::Fetch, None, None));
        for (app, credential, server, url) in [
            (
                "other",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("{SERVER}{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("other"),
                Some(SERVER),
                format!("{SERVER}{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::basic("readeck"),
                Some(SERVER),
                format!("{SERVER}{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                None,
                format!("{SERVER}{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("https://other.example:8443/library{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("https://read.example/library{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("https://read.example:8443/library-other{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("{SERVER}/../library{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("{SERVER}/%2e%2e/library{ARTICLE}"),
            ),
            (
                "readeck",
                Credential::bearer("readeck"),
                Some(SERVER),
                format!("{SERVER}{ARTICLE}#fragment"),
            ),
        ] {
            assert!(
                !allowed_request_with_server(
                    app,
                    &credential,
                    &url,
                    CredentialUse::Fetch,
                    None,
                    None,
                    server
                ),
                "{url}"
            );
        }
    }
    #[test]
    fn only_reviewed_reads_and_exact_mutations_are_permitted() {
        for body in [
            r#"{"read_progress":100}"#,
            r#"{"is_archived":true}"#,
            r#"{"is_marked":true}"#,
            r#"{"is_marked":false}"#,
            r#"{"is_deleted":true}"#,
        ] {
            assert!(request(
                BOOKMARK,
                CredentialUse::Patch,
                Some(body),
                Some("application/json")
            ));
            for usage in [
                CredentialUse::Fetch,
                CredentialUse::Post,
                CredentialUse::Put,
            ] {
                assert!(!request(
                    BOOKMARK,
                    usage,
                    Some(body),
                    Some("application/json")
                ));
            }
            assert!(!request(
                BOOKMARK,
                CredentialUse::Patch,
                Some(body),
                Some("text/plain")
            ));
        }
        for body in [
            "{}",
            r#"{"is_deleted":false}"#,
            r#"{"read_progress":99}"#,
            r#"{"is_marked":true,"title":"changed"}"#,
            r#"{"is_marked":true,"is_marked":false}"#,
        ] {
            assert!(!request(
                BOOKMARK,
                CredentialUse::Patch,
                Some(body),
                Some("application/json")
            ));
        }
        for route in [
            "/api/users",
            "/api/bookmarks",
            "/api/bookmarks/short/article",
            "/api/bookmarks/0123456789ABCDEFGH/article?extra=1",
            "/api/bookmarks/0123456789ABCDEFGH/annotations",
            "/api/bookmarks/0123456789ABCDEFGH/",
        ] {
            assert!(!request(route, CredentialUse::Fetch, None, None));
            assert!(!request(
                route,
                CredentialUse::Patch,
                Some(r#"{"is_deleted":true}"#),
                Some("application/json")
            ));
        }
        assert!(!request(ARTICLE, CredentialUse::Fetch, Some("body"), None));
    }
    #[test]
    fn account_record_lives_under_preserved_secrets_and_replacement_is_atomic() {
        let root = std::env::temp_dir().join(format!(
            "readeck-account-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let secrets = root.join("secrets");
        servers::install(&secrets, "readeck", "readeck", SERVER, "fixture-token").unwrap();
        let path = secrets.join("apps/readeck/servers/readeck");
        let initial = std::fs::read(&path).unwrap();
        assert!(servers::install(
            &secrets,
            "readeck",
            "readeck",
            "http://bad.example",
            "replacement"
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), initial);
        // The update transaction moves the secrets directory, not individual records.
        let next = root.join("next");
        std::fs::create_dir(&next).unwrap();
        std::fs::rename(&secrets, next.join("secrets")).unwrap();
        let preserved =
            std::fs::read_to_string(next.join("secrets/apps/readeck/servers/readeck")).unwrap();
        let record = servers::decode(&preserved).unwrap();
        assert_eq!(record.server, SERVER);
        assert_eq!(record.value, "fixture-token");
        let replacement = "https://second.example";
        servers::install(
            &next.join("secrets"),
            "readeck",
            "readeck",
            replacement,
            "second-token",
        )
        .unwrap();
        let current =
            std::fs::read_to_string(next.join("secrets/apps/readeck/servers/readeck")).unwrap();
        let record = servers::decode(&current).unwrap();
        assert!(!allowed(
            &Credential::bearer("readeck"),
            &record.server,
            &format!("{SERVER}{ARTICLE}"),
            CredentialUse::Fetch,
            None,
            None
        ));
        assert!(allowed(
            &Credential::bearer("readeck"),
            &record.server,
            &format!("{replacement}{ARTICLE}"),
            CredentialUse::Fetch,
            None,
            None
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_accepts_one_bounded_encoded_value_not_extra_parameters() {
        let prefix = "/api/bookmarks?limit=50&offset=50&sort=-created&search=";
        for query in ["rust", "caf%C3%A9", "foo%20bar", "foo%26limit%3D100", "%2F"] {
            assert!(request(
                &format!("{prefix}{query}"),
                CredentialUse::Fetch,
                None,
                None
            ));
        }
        for query in [
            "",
            "%",
            "%FF",
            "%00",
            "%0A",
            "%41",
            "a&limit=100",
            "a#fragment",
            "%20a",
            "a%20",
        ] {
            assert!(!request(
                &format!("{prefix}{query}"),
                CredentialUse::Fetch,
                None,
                None
            ));
        }
        for offset in ["01", "1", "-50", "4294967300"] {
            assert!(!request(
                &format!("/api/bookmarks?limit=50&offset={offset}&sort=-created&search=a"),
                CredentialUse::Fetch,
                None,
                None
            ));
        }
        assert!(!request(
            &format!("{prefix}{}", "a".repeat(257)),
            CredentialUse::Fetch,
            None,
            None
        ));
    }
}
