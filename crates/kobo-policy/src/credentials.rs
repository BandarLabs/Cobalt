//! Platform-owned allowlists for attaching stored credentials to requests.
//!
//! [`kobo_net`] supplies generic HTTPS transport and URL primitives. This
//! module owns the shipped applications' identities and provider contracts so
//! an application cannot broaden the destinations, methods, or headers that a
//! stored secret may use.

use kobo_net::{has_origin, parse};
use kobo_protocol::{Credential, CredentialUse, SecretHeader};

/// The narration voices the audiobook application may spend its `ElevenLabs`
/// key on: one per offered language, native accents. The application holds
/// the same list in its pipeline; a voice added there must be added here.
const AUDIOBOOK_VOICES: [&str; 6] = [
    "JBFqnCBsd6RMkjVDRZzb", // George, English
    "1qEiC6qsybMkmnNdVMbK", // Monika Sogam, Hindi
    "l1zE9xgNpUTaQCZzpNJa", // Alberto Rodríguez, Spanish
    "aQROLel5sQbj1vuIVi6B", // Nicolas, French
    "7eVMgwCnXydb3CikjV7a", // Lea, German
    "4VZIsMPtgggwNg7OXbPY", // James Gao, Chinese
];

/// Whether a shipped application may attach one named secret to this request.
///
/// The runtime calls this immediately before resolving the secret. Policies
/// are default-deny and bind a runtime-verified app ID to the credential name,
/// header convention, request kind, exact HTTPS origin, path, and query.
#[must_use]
pub fn allowed(app: &str, credential: &Credential, url: &str, usage: CredentialUse) -> bool {
    if app == "zotero-reader" {
        return usage == CredentialUse::Fetch && zotero_credential_allowed(credential, url);
    }
    if let Some(allowed) = store_app_credential_allowed(app, credential, url, usage) {
        return allowed;
    }
    if app == "audiobook" {
        return match (&*credential.secret, &credential.header) {
            ("exa", SecretHeader::Named(header)) => {
                header.eq_ignore_ascii_case("x-api-key")
                    && url == "https://api.exa.ai/agent/runs"
                    && has_origin(url, "api.exa.ai", 443)
            }
            ("openai", SecretHeader::Bearer) => {
                url == "https://api.openai.com/v1/responses"
                    && has_origin(url, "api.openai.com", 443)
            }
            ("elevenlabs", SecretHeader::Named(header)) => {
                header.eq_ignore_ascii_case("xi-api-key")
                    && AUDIOBOOK_VOICES.iter().any(|voice| {
                        url == format!(
                            "https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_128"
                        )
                    })
                    && has_origin(url, "api.elevenlabs.io", 443)
            }
            _ => false,
        };
    }
    if app != "chat" {
        return false;
    }
    match (&*credential.secret, &credential.header) {
        ("openai", SecretHeader::Bearer) => {
            url == "https://api.openai.com/v1/chat/completions"
                && has_origin(url, "api.openai.com", 443)
        }
        ("anthropic", SecretHeader::Named(header)) => {
            header.eq_ignore_ascii_case("x-api-key")
                && url == "https://api.anthropic.com/v1/messages"
                && has_origin(url, "api.anthropic.com", 443)
        }
        ("gemini", SecretHeader::Named(header)) => {
            header.eq_ignore_ascii_case("x-goog-api-key")
                && url
                    == "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent"
                && has_origin(url, "generativelanguage.googleapis.com", 443)
        }
        _ => false,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one explicit table keeps each Store app's credential boundary visible"
)]
fn store_app_credential_allowed(
    app: &str,
    credential: &Credential,
    url: &str,
    usage: CredentialUse,
) -> Option<bool> {
    let allowed = match app {
        "calibre-web" => {
            matches!(credential.header, SecretHeader::Basic)
                && usage == CredentialUse::Fetch
                && parsed_path(url).is_some_and(|path| clean_path(&path).ends_with("/opds"))
        }
        "habits" => {
            credential.secret == "habitica"
                && matches!(
                    &credential.header,
                    SecretHeader::Named(header) if header.eq_ignore_ascii_case("x-api-key")
                )
                && usage == CredentialUse::Fetch
                && url == "https://habitica.com/api/v3/tasks/user"
                && has_origin(url, "habitica.com", 443)
        }
        "homepanel" => {
            credential.secret == "homeassistant"
                && credential.header == SecretHeader::Bearer
                && parsed_path(url).is_some_and(|path| {
                    let path = clean_path(&path);
                    match usage {
                        CredentialUse::Fetch => path.ends_with("/api/"),
                        CredentialUse::Post => {
                            path.ends_with("/api/template") || path.contains("/api/services/")
                        }
                    }
                })
        }
        "kitchencard" => {
            credential.secret == "mealie"
                && credential.header == SecretHeader::Bearer
                && usage == CredentialUse::Fetch
                && url == "https://mealie.local/api/recipes?perPage=20"
                && has_origin(url, "mealie.local", 443)
        }
        "lichess" => {
            credential.secret == "lichess"
                && credential.header == SecretHeader::Bearer
                && has_origin(url, "lichess.org", 443)
                && match usage {
                    CredentialUse::Fetch => matches!(
                        parsed_path(url).as_deref(),
                        Some(
                            "/api/puzzle/batch/mix?nb=32&difficulty=normal"
                                | "/api/account/playing"
                        )
                    ),
                    CredentialUse::Post => parsed_path(url).as_deref() == Some("/api/board/seek"),
                }
        }
        "needles" => {
            credential.secret == "ravelry"
                && matches!(credential.header, SecretHeader::Basic)
                && usage == CredentialUse::Fetch
                && matches!(
                    url,
                    "https://api.ravelry.com/people/me/library/list.json"
                        | "https://api.ravelry.com/people/me/queue/list.json"
                        | "https://api.ravelry.com/people/me/favorites/list.json"
                )
                && has_origin(url, "api.ravelry.com", 443)
        }
        "panels" => {
            credential.secret == "komga"
                && matches!(credential.header, SecretHeader::Basic)
                && usage == CredentialUse::Fetch
                && url == "https://komga.local/opds/v1.2/catalog"
                && has_origin(url, "komga.local", 443)
        }
        "post" => {
            credential.secret == "hermes-post"
                && credential.header == SecretHeader::Bearer
                && parsed_path(url).is_some_and(|path| match usage {
                    CredentialUse::Fetch => clean_path(&path).ends_with("/letters"),
                    CredentialUse::Post => clean_path(&path).ends_with("/replies"),
                })
        }
        "readlater" => {
            credential.secret == "wallabag"
                && credential.header == SecretHeader::Bearer
                && parsed_path(url).is_some_and(|path| match usage {
                    CredentialUse::Fetch => {
                        (clean_path(&path).ends_with("/api/entries.json")
                            && path.contains("detail=metadata"))
                            || wallabag_entry_document(&path)
                    }
                    CredentialUse::Post => wallabag_entry_document(&path),
                })
        }
        "rss-miniflux" => {
            credential.secret == "miniflux"
                && matches!(
                    &credential.header,
                    SecretHeader::Named(header) if header.eq_ignore_ascii_case("x-auth-token")
                )
                && parsed_path(url).is_some_and(|path| match usage {
                    CredentialUse::Fetch => {
                        clean_path(&path).ends_with("/v1/entries") && path.contains("status=unread")
                    }
                    CredentialUse::Post => clean_path(&path).ends_with("/v1/entries"),
                })
        }
        _ => return None,
    };
    Some(allowed)
}

fn wallabag_entry_document(path: &str) -> bool {
    let path = clean_path(path);
    path.contains("/api/entries/")
        && path
            .strip_suffix(".json")
            .and_then(|prefix| prefix.rsplit('/').next())
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
}

fn parsed_path(url: &str) -> Option<String> {
    parse(url).ok().map(|target| target.path)
}

fn clean_path(path_and_query: &str) -> &str {
    path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path)
}

/// Binds a dedicated Zotero key to the exact read endpoints used by Zotero
/// Reader. The app cannot send it to group libraries, key-management routes,
/// file downloads, arbitrary queries, or a lookalike origin.
fn zotero_credential_allowed(credential: &Credential, url: &str) -> bool {
    if credential.secret != "zotero" || credential.header != SecretHeader::Bearer {
        return false;
    }
    parse(url).is_ok_and(|target| {
        target.host.eq_ignore_ascii_case("api.zotero.org")
            && target.port == 443
            && zotero_read_api_path(&target.path)
    })
}

fn zotero_read_api_path(path_and_query: &str) -> bool {
    if path_and_query.contains(['%', '\\']) {
        return false;
    }
    let Some(path_and_query) = path_and_query.strip_prefix('/') else {
        return false;
    };
    if path_and_query.starts_with('/') {
        return false;
    }
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 3
        || parts[0] != "users"
        || parts[1].is_empty()
        || parts[1].len() > 20
        || !parts[1].bytes().all(|byte| byte.is_ascii_digit())
        || parts.iter().any(|part| matches!(*part, "." | ".."))
    {
        return false;
    }
    match parts.as_slice() {
        ["users", _, "collections"] => {
            query == Some("format=json&limit=100&sort=title&direction=asc")
        }
        ["users", _, "collections", collection, "items", "top"] if zotero_key(collection) => {
            let Some(query) = query else {
                return false;
            };
            let fields: Vec<&str> = query.split('&').collect();
            if fields.len() != 6
                || fields[0] != "format=json"
                || fields[1] != "itemType=-attachment"
                || fields[4] != "sort=dateAdded"
                || fields[5] != "direction=desc"
            {
                return false;
            }
            let Some(limit) = fields[2].strip_prefix("limit=") else {
                return false;
            };
            let Some(start) = fields[3].strip_prefix("start=") else {
                return false;
            };
            let Ok(start) = start.parse::<usize>() else {
                return false;
            };
            (limit == "25" && start < 500 && start % 25 == 0) || (limit == "1" && start == 500)
        }
        ["users", _, "items", item] if zotero_key(item) => query == Some("format=json"),
        ["users", _, "items", item, "children"] if zotero_key(item) => {
            query == Some("format=json&itemType=attachment&limit=100")
        }
        ["users", _, "items", item, "fulltext"] if zotero_key(item) => query.is_none(),
        _ => false,
    }
}

fn zotero_key(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| matches!(byte, b'2'..=b'9' | b'A'..=b'N' | b'P'..=b'Z'))
}

#[cfg(test)]
mod tests {
    use super::{allowed, AUDIOBOOK_VOICES};
    use kobo_protocol::{Credential, CredentialUse};

    #[test]
    fn chat_credentials_are_bound_to_their_exact_service() {
        let openai = Credential::bearer("openai");
        assert!(allowed(
            "chat",
            &openai,
            "https://api.openai.com/v1/chat/completions",
            CredentialUse::Fetch
        ));
        for (app, url) in [
            ("other", "https://api.openai.com/v1/chat/completions"),
            (
                "chat",
                "https://api.openai.com.attacker.invalid/v1/chat/completions",
            ),
            ("chat", "https://attacker.invalid/collect"),
        ] {
            assert!(!allowed(app, &openai, url, CredentialUse::Fetch));
        }
    }

    #[test]
    fn audiobook_credentials_are_bound_to_exact_provider_requests() {
        let requests = [
            (
                Credential::in_header("exa", "x-api-key"),
                "https://api.exa.ai/agent/runs".to_owned(),
            ),
            (
                Credential::bearer("openai"),
                "https://api.openai.com/v1/responses".to_owned(),
            ),
        ];
        let voices = AUDIOBOOK_VOICES.map(|voice| {
            (
                Credential::in_header("elevenlabs", "xi-api-key"),
                format!(
                    "https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_128"
                ),
            )
        });
        for (credential, url) in requests.into_iter().chain(voices) {
            assert!(allowed(
                "audiobook",
                &credential,
                &url,
                CredentialUse::Fetch
            ));
            assert!(!allowed("chat", &credential, &url, CredentialUse::Fetch));
            assert!(!allowed(
                "audiobook",
                &credential,
                "https://attacker.invalid/collect",
                CredentialUse::Fetch
            ));
        }
        let elevenlabs = Credential::in_header("elevenlabs", "xi-api-key");
        for url in [
            "https://api.elevenlabs.io/v1/text-to-speech/AAAAAAAAAAAAAAAAAAAA?output_format=mp3_44100_128",
            "https://api.elevenlabs.io/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb?output_format=mp3_22050_32",
            "https://api.elevenlabs.io.attacker.invalid/v1/text-to-speech/JBFqnCBsd6RMkjVDRZzb?output_format=mp3_44100_128",
        ] {
            assert!(!allowed(
                "audiobook",
                &elevenlabs,
                url,
                CredentialUse::Fetch
            ));
        }
    }

    #[test]
    fn store_app_credentials_are_bound_to_their_request_shapes() {
        let requests = [
            (
                "calibre-web",
                Credential::basic("calibre"),
                "https://books.example/opds",
                CredentialUse::Fetch,
            ),
            (
                "habits",
                Credential::in_header("habitica", "X-Api-Key"),
                "https://habitica.com/api/v3/tasks/user",
                CredentialUse::Fetch,
            ),
            (
                "homepanel",
                Credential::bearer("homeassistant"),
                "https://home.example/api/template",
                CredentialUse::Post,
            ),
            (
                "kitchencard",
                Credential::bearer("mealie"),
                "https://mealie.local/api/recipes?perPage=20",
                CredentialUse::Fetch,
            ),
            (
                "lichess",
                Credential::bearer("lichess"),
                "https://lichess.org/api/account/playing",
                CredentialUse::Fetch,
            ),
            (
                "lichess",
                Credential::bearer("lichess"),
                "https://lichess.org/api/board/seek",
                CredentialUse::Post,
            ),
            (
                "needles",
                Credential::basic("ravelry"),
                "https://api.ravelry.com/people/me/library/list.json",
                CredentialUse::Fetch,
            ),
            (
                "panels",
                Credential::basic("komga"),
                "https://komga.local/opds/v1.2/catalog",
                CredentialUse::Fetch,
            ),
            (
                "post",
                Credential::bearer("hermes-post"),
                "https://letters.example/replies",
                CredentialUse::Post,
            ),
            (
                "readlater",
                Credential::bearer("wallabag"),
                "https://read.example/api/entries.json?detail=metadata&perPage=50&page=1&archive=0",
                CredentialUse::Fetch,
            ),
            (
                "readlater",
                Credential::bearer("wallabag"),
                "https://read.example/api/entries/7.json",
                CredentialUse::Fetch,
            ),
            (
                "rss-miniflux",
                Credential::in_header("miniflux", "X-Auth-Token"),
                "https://feeds.example/v1/entries?status=unread&limit=100&order=published_at&direction=desc",
                CredentialUse::Fetch,
            ),
        ];
        for (app, credential, url, usage) in requests {
            assert!(allowed(app, &credential, url, usage), "{app}: {url}");
            assert!(
                !allowed("other", &credential, url, usage),
                "another app used {app}'s credential"
            );
        }
    }

    #[test]
    fn store_app_credentials_reject_wrong_headers_methods_and_paths() {
        assert!(!allowed(
            "post",
            &Credential::bearer("hermes-post"),
            "http://letters.example/letters",
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "post",
            &Credential::basic("hermes-post"),
            "https://letters.example/letters",
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "readlater",
            &Credential::bearer("wallabag"),
            "https://read.example/api/users",
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "rss-miniflux",
            &Credential::in_header("miniflux", "Authorization"),
            "https://feeds.example/v1/entries?status=unread",
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "lichess",
            &Credential::bearer("lichess"),
            "https://lichess.org/api/token",
            CredentialUse::Fetch
        ));
    }

    #[test]
    fn zotero_key_is_bound_to_exact_read_routes() {
        let key = Credential::bearer("zotero");
        for url in [
            "https://api.zotero.org/users/12345/collections?format=json&limit=100&sort=title&direction=asc",
            "https://api.zotero.org/users/12345/collections/ABCD2345/items/top?format=json&itemType=-attachment&limit=25&start=475&sort=dateAdded&direction=desc",
            "https://api.zotero.org/users/12345/collections/ABCD2345/items/top?format=json&itemType=-attachment&limit=1&start=500&sort=dateAdded&direction=desc",
            "https://api.zotero.org/users/12345/items/EFGH6789?format=json",
            "https://api.zotero.org/users/12345/items/EFGH6789/children?format=json&itemType=attachment&limit=100",
            "https://api.zotero.org/users/12345/items/JKLM2345/fulltext",
        ] {
            assert!(allowed(
                "zotero-reader",
                &key,
                url,
                CredentialUse::Fetch
            ));
            assert!(!allowed(
                "zotero-reader",
                &key,
                url,
                CredentialUse::Post
            ));
        }
    }

    #[test]
    fn zotero_key_refuses_other_apps_credentials_and_destinations() {
        let key = Credential::bearer("zotero");
        let item = "https://api.zotero.org/users/12345/items/PAPER001?format=json";
        assert!(!allowed("other", &key, item, CredentialUse::Fetch));
        assert!(!allowed(
            "zotero-reader",
            &Credential::bearer("other"),
            item,
            CredentialUse::Fetch
        ));
        for url in [
            "http://api.zotero.org/users/12345/items/PAPER001?format=json",
            "https://api.zotero.org:8443/users/12345/items/PAPER001?format=json",
            "https://user@api.zotero.org/users/12345/items/PAPER001?format=json",
            "https://api.zotero.org.attacker.invalid/users/12345/items/PAPER001?format=json",
            "https://api.zotero.org/groups/12345/items/PAPER001?format=json",
            "https://api.zotero.org/users/name/items/PAPER001?format=json",
            "https://api.zotero.org/users/12345/items",
            "https://api.zotero.org/users/12345/items/paper001?format=json",
            "https://api.zotero.org/users/12345/items/PAPER001/file",
            "https://api.zotero.org/users/12345/items/PAPER001?format=json&key=leak",
            "https://api.zotero.org/users/12345/items/%2e%2e/fulltext",
            "https://api.zotero.org/users/12345/collections/COLL1234/items/top?format=json&itemType=-attachment&limit=100&start=0&sort=dateAdded&direction=desc",
            "https://api.zotero.org/users/12345/items/ABCD0EFG?format=json",
            "https://api.zotero.org/users/12345/items/ABCD1EFG?format=json",
            "https://api.zotero.org/users/12345/items/ABCDOEFG?format=json",
            "https://api.zotero.org//users/12345/items/ABCD2345?format=json",
        ] {
            assert!(!allowed(
                "zotero-reader",
                &key,
                url,
                CredentialUse::Fetch
            ), "accepted {url}");
        }
    }
}
