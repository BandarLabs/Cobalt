//! Platform-owned allowlists for attaching stored credentials to requests.
//!
//! [`kobo_net`] supplies generic HTTPS transport and URL primitives. This
//! module owns the shipped applications' identities and provider contracts so
//! an application cannot broaden the destinations, methods, or headers that a
//! stored secret may use.

use kobo_net::{has_origin, parse};
use kobo_protocol::{Credential, CredentialUse, SecretHeader};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// The directory below the owner secret root reserved for app-entered values.
pub const APP_SECRET_DIRECTORY: &str = "apps";

/// Returns the private namespace for one runtime-verified application.
///
/// The app identity is supplied by the runtime after the executable's path
/// and `Hello` identity agree. A secret name is only one path component, so
/// neither input can select another application's namespace.
#[must_use]
pub fn app_secret_path(root: &Path, app: &str, name: &str) -> Option<PathBuf> {
    if !kobo_protocol::valid_app_id(app) || !valid_secret_name(name) {
        return None;
    }
    Some(root.join(APP_SECRET_DIRECTORY).join(app).join(name))
}

/// Installs an app-entered credential in the verified caller's namespace.
///
/// Global files directly below `root` remain owner-managed CLI credentials.
/// They are never replaced by this path and are only a fallback at lookup.
///
/// # Errors
///
/// Returns [`kobo_protocol::DeviceError::InvalidInput`] for a caller, name, or
/// value outside policy, and [`kobo_protocol::DeviceError::Backend`] when the
/// private directory cannot be safely created or durably replaced.
pub fn install_app_secret(
    root: &Path,
    app: &str,
    name: &str,
    value: &str,
) -> Result<(), kobo_protocol::DeviceError> {
    if !may_set(app, name)
        || app_secret_path(root, app, name).is_none()
        || value.is_empty()
        || value.len() > kobo_protocol::MAX_APP_SECRET_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(kobo_protocol::DeviceError::InvalidInput);
    }
    private_directory(root)?;
    let apps = root.join(APP_SECRET_DIRECTORY);
    private_directory(&apps)?;
    let directory = apps.join(app);
    private_directory(&directory)?;

    let temporary = directory.join(format!(".{name}.new"));
    let destination = directory.join(name);
    if temporary.exists() {
        let kind = fs::symlink_metadata(&temporary)
            .map_err(|_| kobo_protocol::DeviceError::Backend)?
            .file_type();
        if !kind.is_file() && !kind.is_symlink() {
            return Err(kobo_protocol::DeviceError::Backend);
        }
        fs::remove_file(&temporary).map_err(|_| kobo_protocol::DeviceError::Backend)?;
    }
    let result: std::io::Result<()> = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &destination)?;
        fs::File::open(&directory)?.sync_all()
    })();
    if result.is_err() {
        let _ignored = fs::remove_file(&temporary);
    }
    result.map_err(|_| kobo_protocol::DeviceError::Backend)
}

fn private_directory(path: &Path) -> Result<(), kobo_protocol::DeviceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|_| kobo_protocol::DeviceError::Backend)?;
        }
        Ok(_) | Err(_) => return Err(kobo_protocol::DeviceError::Backend),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| kobo_protocol::DeviceError::Backend)
}

/// Whether a credential name is exactly one portable path component.
#[must_use]
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

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

/// Whether an application may install one runtime-owned credential.
///
/// This is deliberately narrower than filesystem access: an app may replace
/// only the exact secret names its reviewed network policy can consume.
#[must_use]
pub fn may_set(app: &str, name: &str) -> bool {
    match app {
        "audiobook" => matches!(name, "exa" | "openai" | "elevenlabs"),
        "chat" => matches!(name, "openai" | "anthropic" | "gemini"),
        "rss" => name == "miniflux",
        "zotero-reader" => name == "zotero",
        _ => false,
    }
}

/// Whether a shipped application may attach one named secret to this request.
///
/// The runtime calls this immediately before resolving the secret. Policies
/// are default-deny and bind a runtime-verified app ID to the credential name,
/// header convention, request kind, exact HTTPS origin, path, and query.
#[must_use]
pub fn allowed(app: &str, credential: &Credential, url: &str, usage: CredentialUse) -> bool {
    allowed_request(app, credential, url, usage, None, None)
}

/// The complete credential decision, including the body shape of writes.
///
/// The shorter [`allowed`] entry point remains for read-only callers and
/// tests. A state-changing API must come through this form so permission to
/// POST one route cannot be stretched into arbitrary parameters.
#[must_use]
pub fn allowed_request(
    app: &str,
    credential: &Credential,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if app == "lichess" {
        return lichess_credential_allowed(credential, url, usage, body, content_type);
    }
    if app == "zotero-reader" {
        return usage == CredentialUse::Fetch && zotero_credential_allowed(credential, url);
    }
    if app == "rss" {
        return miniflux_credential_allowed(credential, url, usage, body, content_type);
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
                    == "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.6-flash:generateContent"
                && has_origin(url, "generativelanguage.googleapis.com", 443)
        }
        _ => false,
    }
}

fn lichess_credential_allowed(
    credential: &Credential,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if credential.secret != "lichess"
        || credential.header != SecretHeader::Bearer
        || !has_origin(url, "lichess.org", 443)
    {
        return false;
    }
    let Ok(target) = parse(url) else {
        return false;
    };
    if target.path.contains(['%', '\\', '#'])
        || target.path.starts_with("//")
        || target
            .path
            .split('/')
            .any(|part| matches!(part, "." | ".."))
    {
        return false;
    }
    match usage {
        CredentialUse::Fetch => {
            body.is_none()
                && content_type.is_none()
                && (matches!(
                    target.path.as_str(),
                    "/api/account"
                        | "/api/account/playing"
                        | "/api/stream/event"
                        | "/api/puzzle/batch/mix?nb=32&difficulty=normal"
                ) || target
                    .path
                    .strip_prefix("/api/board/game/stream/")
                    .is_some_and(lichess_id))
        }
        CredentialUse::Post => {
            content_type == Some("application/x-www-form-urlencoded")
                && body.is_some_and(|body| lichess_post(&target.path, body))
        }
        CredentialUse::Put => false,
    }
}

/// Binds RSS to its dedicated Miniflux token and the small v1 API vocabulary
/// the application actually uses. The server is owner-configured, so the
/// static policy cannot pin its origin; `parse` still requires HTTPS and the
/// route guard preserves an intentional reverse-proxy prefix.
fn miniflux_credential_allowed(
    credential: &Credential,
    url: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if credential.secret != "miniflux"
        || !matches!(
            &credential.header,
            SecretHeader::Named(header) if header.eq_ignore_ascii_case("x-auth-token")
        )
    {
        return false;
    }
    let Ok(target) = parse(url) else {
        return false;
    };
    miniflux_v1_request_allowed(&target.path, usage, body, content_type)
}

fn miniflux_v1_request_allowed(
    path_and_query: &str,
    usage: CredentialUse,
    body: Option<&str>,
    content_type: Option<&str>,
) -> bool {
    if path_and_query.contains(['%', '\\', '#']) {
        return false;
    }
    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, None), |(path, query)| (path, Some(query)));
    let Some(path) = path.strip_prefix('/') else {
        return false;
    };
    let parts: Vec<&str> = path.split('/').collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return false;
    }
    match usage {
        CredentialUse::Fetch if parts.ends_with(&["v1", "entries"]) => {
            body.is_none()
                && content_type.is_none()
                && matches!(
                    query,
                    Some(
                        "status=unread&limit=100&order=published_at&direction=desc"
                            | "starred=true&limit=100&order=published_at&direction=desc"
                            | "status=read&limit=100&order=published_at&direction=desc"
                    )
                )
        }
        CredentialUse::Fetch
            if parts.len() >= 4
                && matches!(
                    &parts[parts.len() - 4..],
                    ["v1", "entries", id, "fetch-content"]
                        if !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())
                ) =>
        {
            body.is_none() && content_type.is_none() && query.is_none()
        }
        CredentialUse::Post
            if parts.ends_with(&["v1", "discover"]) || parts.ends_with(&["v1", "feeds"]) =>
        {
            query.is_none() && content_type == Some("application/json") && body.is_some()
        }
        CredentialUse::Put if parts.ends_with(&["v1", "entries"]) => {
            query.is_none()
                && content_type == Some("application/json")
                && body.is_some_and(miniflux_mutation_body)
        }
        _ => false,
    }
}

fn miniflux_mutation_body(body: &str) -> bool {
    let Some(ids) = body
        .strip_prefix(r#"{"entry_ids":["#)
        .and_then(|body| body.split_once("],"))
    else {
        return false;
    };
    let (id, tail) = ids;
    !id.is_empty()
        && id.bytes().all(|byte| byte.is_ascii_digit())
        && id.parse::<u64>().is_ok()
        && matches!(
            tail,
            r#""status":"read"}"# | r#""starred":true}"# | r#""starred":false}"#
        )
}

fn lichess_post(path: &str, body: &str) -> bool {
    if path == "/api/board/seek" {
        return [
            "rated=true&time=10&increment=0&variant=standard&color=random",
            "rated=true&time=10&increment=5&variant=standard&color=random",
            "rated=true&time=15&increment=10&variant=standard&color=random",
            "rated=true&time=30&increment=0&variant=standard&color=random",
            "rated=true&time=30&increment=20&variant=standard&color=random",
        ]
        .contains(&body);
    }
    let parts = path.trim_start_matches('/').split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        ["api", "board", "game", game, "move", movement] => {
            body.is_empty() && lichess_id(game) && uci_move(movement)
        }
        ["api", "board", "game", game, action]
            if matches!(*action, "resign" | "abort" | "claim-victory") =>
        {
            body.is_empty() && lichess_id(game)
        }
        ["api", "board", "game", game, "draw", answer] => {
            body.is_empty() && lichess_id(game) && matches!(*answer, "yes" | "no")
        }
        ["api", "challenge", challenge, action] if matches!(*action, "accept" | "decline") => {
            body.is_empty() && lichess_id(challenge)
        }
        _ => false,
    }
}

fn lichess_id(value: &str) -> bool {
    (8..=16).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn uci_move(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.len(), 4 | 5)
        && matches!(bytes[0], b'a'..=b'h')
        && matches!(bytes[1], b'1'..=b'8')
        && matches!(bytes[2], b'a'..=b'h')
        && matches!(bytes[3], b'1'..=b'8')
        && (bytes.len() == 4 || matches!(bytes[4], b'q' | b'r' | b'b' | b'n'))
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
    use super::{allowed, allowed_request, install_app_secret, may_set, AUDIOBOOK_VOICES};
    use kobo_protocol::{Credential, CredentialUse};

    #[test]
    fn apps_can_install_only_the_credentials_their_policy_consumes() {
        assert!(may_set("zotero-reader", "zotero"));
        assert!(may_set("chat", "anthropic"));
        assert!(may_set("audiobook", "elevenlabs"));
        assert!(may_set("rss", "miniflux"));
        assert!(!may_set("rss", "personal-miniflux"));
        assert!(!may_set("zotero-reader", "openai"));
        assert!(!may_set("lichess", "lichess"));
        assert!(!may_set("other", "zotero"));
    }

    #[test]
    fn rss_miniflux_token_is_exact_and_put_is_limited_to_entry_mutations() {
        let token = Credential::in_header("miniflux", "X-Auth-Token");
        let entries = "https://feeds.example/reader/v1/entries";
        let v1_prefix = "https://feeds.example/reader/v1/proxy";
        assert!(allowed_request(
            "rss",
            &token,
            entries,
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"starred":true}"#),
            Some("application/json"),
        ));
        assert!(allowed(
            "rss",
            &token,
            "https://feeds.example/reader/v1/entries?status=unread&limit=100&order=published_at&direction=desc",
            CredentialUse::Fetch,
        ));
        assert!(
            allowed(
                "rss",
                &token,
                &format!(
                    "{v1_prefix}/v1/entries?status=unread&limit=100&order=published_at&direction=desc"
                ),
                CredentialUse::Fetch,
            ),
            "a configured reverse-proxy prefix may itself contain v1"
        );
        assert!(allowed_request(
            "rss",
            &token,
            &format!("{v1_prefix}/v1/entries"),
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"starred":true}"#),
            Some("application/json"),
        ));
        assert!(allowed_request(
            "rss",
            &token,
            &format!("{v1_prefix}/v1/discover"),
            CredentialUse::Post,
            Some(r#"{"url":"https://example.org"}"#),
            Some("application/json"),
        ));
        for (credential, usage, body, content_type) in [
            (
                Credential::in_header("personal-miniflux", "X-Auth-Token"),
                CredentialUse::Put,
                Some(r#"{"entry_ids":[7],"status":"read"}"#),
                Some("application/json"),
            ),
            (
                Credential::in_header("miniflux", "X-Auth-Token"),
                CredentialUse::Post,
                Some(r#"{"entry_ids":[7],"status":"read"}"#),
                Some("application/json"),
            ),
            (
                Credential::in_header("miniflux", "X-Auth-Token"),
                CredentialUse::Put,
                Some(r#"{"entry_ids":[7],"status":"unread"}"#),
                Some("application/json"),
            ),
        ] {
            assert!(
                !allowed_request("rss", &credential, entries, usage, body, content_type),
                "{credential:?} {usage:?} unexpectedly authorized"
            );
        }
        for url in [
            "https://feeds.example/reader/v1/proxy/entries?status=unread&limit=100&order=published_at&direction=desc",
            "https://feeds.example/reader/v1/proxy/../v1/entries?status=unread&limit=100&order=published_at&direction=desc",
            "https://feeds.example/reader/%2e/v1/entries?status=unread&limit=100&order=published_at&direction=desc",
        ] {
            assert!(
                !allowed("rss", &token, url, CredentialUse::Fetch),
                "non-terminal route or traversing prefix unexpectedly authorized: {url}"
            );
        }
        assert!(!allowed_request(
            "rss",
            &token,
            "https://feeds.example/reader/v1/proxy/v1/entries/7",
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#),
            Some("application/json"),
        ));
        assert!(!allowed_request(
            "rss",
            &token,
            "http://feeds.example/v1/entries",
            CredentialUse::Put,
            Some(r#"{"entry_ids":[7],"status":"read"}"#),
            Some("application/json"),
        ));
    }

    #[test]
    fn app_entered_credentials_are_written_only_under_the_verified_app() {
        let root =
            std::env::temp_dir().join(format!("kobo-policy-app-secrets-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&root);
        install_app_secret(&root, "chat", "openai", "chat-key").expect("chat credential");
        install_app_secret(&root, "audiobook", "openai", "audio-key")
            .expect("audiobook credential");
        assert_eq!(
            std::fs::read(root.join("apps/chat/openai")).expect("chat value"),
            b"chat-key"
        );
        assert_eq!(
            std::fs::read(root.join("apps/audiobook/openai")).expect("audiobook value"),
            b"audio-key"
        );
        assert!(!root.join("openai").exists());
        let _ignored = std::fs::remove_dir_all(root);
    }

    #[test]
    fn app_identity_and_symlink_boundaries_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "kobo-policy-app-secret-links-{}",
            std::process::id()
        ));
        let outside = std::env::temp_dir().join(format!(
            "kobo-policy-app-secret-outside-{}",
            std::process::id()
        ));
        let _ignored = std::fs::remove_dir_all(&root);
        let _ignored = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(root.join("apps")).expect("apps");
        std::fs::create_dir_all(&outside).expect("outside");
        symlink(&outside, root.join("apps/chat")).expect("app link");
        assert!(
            install_app_secret(&root, "chat", "openai", "not-written").is_err(),
            "an app namespace symlink was followed"
        );
        assert!(
            install_app_secret(&root, "../audiobook", "openai", "not-written").is_err(),
            "a caller selected another namespace"
        );
        assert!(!outside.join("openai").exists());
        let _ignored = std::fs::remove_dir_all(root);
        let _ignored = std::fs::remove_dir_all(outside);
    }

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

    /// This is the second of two places that name Gemini's exact endpoint --
    /// `examples/chat/src/conversation.rs` is the other -- and the two went
    /// out of sync once already: the application moved to a current model
    /// after Google retired `gemini-2.0-flash`, this allowlist did not, and
    /// every Gemini request was refused as though no key were installed. This
    /// does not close the gap (this crate cannot depend on an application to
    /// compare against), but it does mean an edit to the URL on just one side
    /// fails a test instead of shipping silently.
    #[test]
    fn gemini_credentials_are_bound_to_the_current_model_endpoint() {
        let gemini = Credential::in_header("gemini", "x-goog-api-key");
        assert!(allowed(
            "chat",
            &gemini,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.6-flash:generateContent",
            CredentialUse::Fetch
        ));
        for url in [
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.0-flash:generateContent",
            "https://generativelanguage.googleapis.com.attacker.invalid/v1beta/models/gemini-3.6-flash:generateContent",
            "https://attacker.invalid/collect",
        ] {
            assert!(!allowed("chat", &gemini, url, CredentialUse::Fetch));
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
    fn lichess_token_is_bound_to_the_board_api_routes_the_app_uses() {
        let token = Credential::bearer("lichess");
        for url in [
            "https://lichess.org/api/account",
            "https://lichess.org/api/account/playing",
            "https://lichess.org/api/stream/event",
            "https://lichess.org/api/board/game/stream/abcdEF12",
            "https://lichess.org/api/puzzle/batch/mix?nb=32&difficulty=normal",
        ] {
            assert!(
                allowed("lichess", &token, url, CredentialUse::Fetch),
                "{url}"
            );
        }
        for body in [
            "rated=true&time=10&increment=0&variant=standard&color=random",
            "rated=true&time=10&increment=5&variant=standard&color=random",
            "rated=true&time=15&increment=10&variant=standard&color=random",
            "rated=true&time=30&increment=0&variant=standard&color=random",
            "rated=true&time=30&increment=20&variant=standard&color=random",
        ] {
            assert!(allowed_request(
                "lichess",
                &token,
                "https://lichess.org/api/board/seek",
                CredentialUse::Post,
                Some(body),
                Some("application/x-www-form-urlencoded"),
            ));
        }
        for (url, body) in [
            ("https://lichess.org/api/board/game/abcdEF12/move/e2e4", ""),
            ("https://lichess.org/api/board/game/abcdEF12/resign", ""),
            ("https://lichess.org/api/board/game/abcdEF12/abort", ""),
            (
                "https://lichess.org/api/board/game/abcdEF12/claim-victory",
                "",
            ),
            ("https://lichess.org/api/board/game/abcdEF12/draw/yes", ""),
            ("https://lichess.org/api/board/game/abcdEF12/draw/no", ""),
            ("https://lichess.org/api/challenge/abcdEF12/accept", ""),
            ("https://lichess.org/api/challenge/abcdEF12/decline", ""),
        ] {
            assert!(
                allowed_request(
                    "lichess",
                    &token,
                    url,
                    CredentialUse::Post,
                    Some(body),
                    Some("application/x-www-form-urlencoded"),
                ),
                "{url}"
            );
        }
    }

    #[test]
    fn lichess_policy_refuses_origin_path_method_and_body_expansion() {
        let token = Credential::bearer("lichess");
        for url in [
            "http://lichess.org/api/account",
            "https://lichess.org:8443/api/account",
            "https://lichess.org.attacker.invalid/api/account",
            "https://user@lichess.org/api/account",
            "https://lichess.org/api/token",
            "https://lichess.org/api/board/game/stream/short",
            "https://lichess.org/api/board/game/stream/abcdEF12?token=leak",
            "https://lichess.org/api/board/game/stream/%2e%2e",
        ] {
            assert!(
                !allowed("lichess", &token, url, CredentialUse::Fetch),
                "{url}"
            );
        }
        for (url, body, content_type) in [
            (
                "https://lichess.org/api/board/seek",
                "rated=false&time=10&increment=0&variant=standard&color=random",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/seek",
                "rated=true&time=10&increment=0&variant=standard&color=random&extra=1",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/seek",
                "rated=true&time=2&increment=1&variant=standard&color=random",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/seek",
                "rated=true&time=30&increment=20&variant=standard&color=white",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/game/abcdEF12/move/e2e4",
                "again=1",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/game/abcdEF12/move/e2e9",
                "",
                "application/x-www-form-urlencoded",
            ),
            (
                "https://lichess.org/api/board/game/abcdEF12/resign",
                "",
                "application/json",
            ),
        ] {
            assert!(!allowed_request(
                "lichess",
                &token,
                url,
                CredentialUse::Post,
                Some(body),
                Some(content_type),
            ));
        }
        assert!(!allowed_request(
            "other",
            &token,
            "https://lichess.org/api/account",
            CredentialUse::Fetch,
            None,
            None,
        ));
        assert!(!allowed_request(
            "lichess",
            &Credential::bearer("other"),
            "https://lichess.org/api/account",
            CredentialUse::Fetch,
            None,
            None,
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
