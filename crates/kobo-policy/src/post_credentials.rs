//! Reviewed Post gateway routes for an account bound to its owner's server.
//! The gateway contract is documented in apps/post/README.md; no service
//! implementation code is used.
use kobo_json::Value;
use kobo_protocol::{Credential, CredentialUse, SecretHeader};

const MAX_BODY: usize = 48 * 1024;
const MAX_REPLY: usize = 32 * 1024;

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
        || credential.secret != "hermes-post"
        || credential.header != SecretHeader::Bearer
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
        CredentialUse::Fetch => body.is_none() && content_type.is_none() && letters_query(route),
        CredentialUse::Post if route == "/replies" => {
            content_type == Some("application/json") && body.is_some_and(reply_body)
        }
        _ => false,
    }
}

fn letters_query(route: &str) -> bool {
    let Some(query) = route.strip_prefix("/letters?") else {
        return false;
    };
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
            "page" => decimal(value, 1, 1_000_000).is_some(),
            "per_page" => decimal(value, 1, 50).is_some(),
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    seen.contains(&"page") && seen.contains(&"per_page")
}

fn decimal(text: &str, minimum: i64, maximum: i64) -> Option<i64> {
    if text.is_empty() || text.len() > 16 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>()
        .ok()
        .filter(|number| (minimum..=maximum).contains(number))
}

fn reply_body(body: &str) -> bool {
    if body.len() > MAX_BODY {
        return false;
    }
    let Ok(Value::Object(fields)) = kobo_json::parse(body) else {
        return false;
    };
    let mut seen = Vec::new();
    for (name, value) in &fields {
        if seen.contains(&name.as_str()) {
            return false;
        }
        seen.push(name.as_str());
        let valid = match name.as_str() {
            "letter_id" => value
                .as_str()
                .is_some_and(|id| !id.is_empty() && id.len() <= 128 && plain(id)),
            "body" => value
                .as_str()
                .is_some_and(|text| !text.is_empty() && text.len() <= MAX_REPLY),
            "reply_id" => value.as_str().is_some_and(|id| {
                id.len() == 64
                    && id
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            }),
            _ => false,
        };
        if !valid {
            return false;
        }
    }
    seen == ["letter_id", "body", "reply_id"]
}

fn plain(text: &str) -> bool {
    !text.chars().any(char::is_control) && !text.contains(['\\', '"'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::allowed_request_with_server;

    const SERVER: &str = "https://post.example:8443/hermes";
    fn request(route: &str, usage: CredentialUse, body: Option<&str>) -> bool {
        allowed_request_with_server(
            "post",
            &Credential::bearer("hermes-post"),
            &format!("{SERVER}{route}"),
            usage,
            body,
            body.map(|_| "application/json"),
            Some(SERVER),
        )
    }

    fn reply(letter: &str, body: &str, id: &str) -> String {
        format!(r#"{{"letter_id":"{letter}","body":"{body}","reply_id":"{id}"}}"#)
    }

    #[test]
    fn bound_account_reads_letters_with_bounded_pagination() {
        for route in ["/letters?page=1&per_page=10", "/letters?page=2&per_page=50"] {
            assert!(request(route, CredentialUse::Fetch, None), "{route}");
        }
        for route in [
            "/letters",
            "/letters?page=1",
            "/letters?per_page=10",
            "/letters?page=0&per_page=10",
            "/letters?page=1&per_page=51",
            "/letters?page=1&per_page=10&page=2",
            "/letters?page=1&per_page=10&unknown=true",
            "/letters?page=1&per_page=%31%30",
            "/letters/7",
            "/replies",
            "/letters/%2e%2e",
        ] {
            assert!(!request(route, CredentialUse::Fetch, None), "{route}");
        }
        assert!(!request(
            "/letters?page=1&per_page=10",
            CredentialUse::Fetch,
            Some("{}")
        ));
    }

    #[test]
    fn replies_carry_exactly_letter_body_and_idempotency_key() {
        let id = "a".repeat(64);
        assert!(request(
            "/replies",
            CredentialUse::Post,
            Some(&reply("morning-note", "Tea first.", &id))
        ));
        for body in [
            "{}",
            &reply("morning-note", "", &id),
            &reply("", "Tea first.", &id),
            &reply("morning-note", "Tea first.", &"a".repeat(63)),
            &reply("morning-note", "Tea first.", &"A".repeat(64)),
            &reply("mor\"ning", "Tea first.", &id),
            &reply("morning-note", "Tea first.", &format!("{id}{id}")),
        ] {
            assert!(
                !request("/replies", CredentialUse::Post, Some(body)),
                "{body}"
            );
        }
        let extra = r#"{"letter_id":"a","body":"b","reply_id":""#;
        let extra = format!("{}{}\",\"delete\":true}}", extra, "a".repeat(64));
        assert!(!request("/replies", CredentialUse::Post, Some(&extra)));
        assert!(!request("/replies", CredentialUse::Fetch, None));
        assert!(!request(
            "/replies",
            CredentialUse::Post,
            Some(&" ".repeat(MAX_BODY + 1))
        ));
    }

    #[test]
    fn token_never_escapes_its_saved_server_or_convention() {
        let token = Credential::bearer("hermes-post");
        for url in [
            "https://other.example:8443/hermes/letters?page=1&per_page=10",
            "http://post.example:8443/hermes/letters?page=1&per_page=10",
            "https://post.example:8443/letters?page=1&per_page=10",
            "https://post.example:8443/hermes-other/letters?page=1&per_page=10",
            "https://post.example:8443/hermes/letters?page=1&per_page=10#x",
        ] {
            assert!(
                !allowed(&token, SERVER, url, CredentialUse::Fetch, None, None),
                "{url}"
            );
        }
        for credential in [
            Credential::basic("hermes-post"),
            Credential::in_header("hermes-post", "X-Auth-Token"),
            Credential::bearer("other"),
        ] {
            assert!(!allowed(
                &credential,
                SERVER,
                &format!("{SERVER}/letters?page=1&per_page=10"),
                CredentialUse::Fetch,
                None,
                None
            ));
        }
        assert!(!allowed_request_with_server(
            "panels",
            &token,
            &format!("{SERVER}/letters?page=1&per_page=10"),
            CredentialUse::Fetch,
            None,
            None,
            Some(SERVER)
        ));
        // The legacy owner-managed rule (a plain `kobo secret set
        // hermes-post`) still answers unbound calls; the server-bound path is
        // the strict one and never leaks the token to another server.
    }
}
