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
    if app == "zotero-reader" {
        return usage == CredentialUse::Fetch && zotero_credential_allowed(credential, url);
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
    use super::{allowed, install_app_secret, may_set, AUDIOBOOK_VOICES};
    use kobo_protocol::{Credential, CredentialUse};

    #[test]
    fn apps_can_install_only_the_credentials_their_policy_consumes() {
        assert!(may_set("zotero-reader", "zotero"));
        assert!(may_set("chat", "anthropic"));
        assert!(may_set("audiobook", "elevenlabs"));
        assert!(!may_set("zotero-reader", "openai"));
        assert!(!may_set("other", "zotero"));
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
