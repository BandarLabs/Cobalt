//! Account values and their permitted server are stored and read as one record.
use kobo_protocol::{Credential, CredentialUse, DeviceError, SecretHeader};
use std::path::{Path, PathBuf};

const MAGIC: &str = "cobalt-server-account-v1";

#[must_use]
pub fn may_set(app: &str, name: &str) -> bool {
    app == "panels" && name == "komga"
}

pub(crate) fn path(root: &Path, app: &str, name: &str) -> Option<PathBuf> {
    super::app_secret_path(root, app, name)?;
    Some(
        root.join(super::APP_SECRET_DIRECTORY)
            .join(app)
            .join("servers")
            .join(name),
    )
}

fn clean_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    !lower.contains(['\\', '\r', '\n'])
        && !["%2e", "%2f", "%5c", "%00", "%0a", "%0d"]
            .iter()
            .any(|part| lower.contains(part))
        && !path.split('/').any(|part| matches!(part, "." | ".."))
}

#[must_use]
pub fn valid_server(server: &str) -> bool {
    kobo_protocol::valid_secret_server(server)
        && kobo_net::parse(server).is_ok_and(|address| clean_path(&address.path))
}

/// Restrict account use to the configured HTTPS origin and base path.
#[must_use]
pub fn contains(server: &str, url: &str) -> bool {
    if !valid_server(server) {
        return false;
    }
    let (Ok(base), Ok(target)) = (kobo_net::parse(server), kobo_net::parse(url)) else {
        return false;
    };
    let base_path = base.path.trim_end_matches('/');
    let target_path = target.path.split('?').next().unwrap_or("");
    base.host.eq_ignore_ascii_case(&target.host)
        && base.port == target.port
        && clean_path(target_path)
        && (base_path.is_empty()
            || target_path == base_path
            || target_path
                .strip_prefix(base_path)
                .is_some_and(|tail| tail.starts_with('/')))
}

/// The reviewed provider policy; a saved server cannot grant another method,
/// credential name, header convention or application's account.
#[must_use]
pub fn allowed(
    app: &str,
    credential: &Credential,
    server: &str,
    url: &str,
    usage: CredentialUse,
) -> bool {
    may_set(app, &credential.secret)
        && credential.header == SecretHeader::Basic
        && usage == CredentialUse::Fetch
        && contains(server, url)
}

/// Install a complete private record before acknowledging the account save.
///
/// # Errors
/// Refuses unsupported app/name pairs, malformed servers and invalid values;
/// reports storage failure without exposing a partial record.
pub fn install(
    root: &Path,
    app: &str,
    name: &str,
    server: &str,
    value: &str,
) -> Result<(), DeviceError> {
    if !may_set(app, name) || !valid_server(server) || !super::valid_value(value) {
        return Err(DeviceError::InvalidInput);
    }
    let path = path(root, app, name).ok_or(DeviceError::InvalidInput)?;
    super::private_directory(root)?;
    let apps = root.join(super::APP_SECRET_DIRECTORY);
    super::private_directory(&apps)?;
    super::private_directory(&apps.join(app))?;
    let directory = path.parent().ok_or(DeviceError::InvalidInput)?;
    super::private_directory(directory)?;
    let record = format!("{MAGIC}\n{server}\n{value}");
    super::write_private_record(directory, name, record.as_bytes())
}

// No Debug implementation: this object contains the exact private value.
pub(crate) struct Record {
    pub server: String,
    pub value: String,
}

pub(crate) fn decode(bytes: &str) -> Option<Record> {
    let mut lines = bytes.split('\n');
    if lines.next()? != MAGIC {
        return None;
    }
    let server = lines.next()?;
    let value = lines.next()?;
    if lines.next().is_some() || !valid_server(server) || !super::valid_value(value) {
        return None;
    }
    Some(Record {
        server: server.into(),
        value: value.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn server_scope_rejects_origin_path_and_header_escape() {
        let base = "https://books.example:8443/library";
        let account = Credential::basic("komga");
        for url in [
            "https://books.example:8443/library",
            "https://BOOKS.example:8443/library/opds?page=2",
        ] {
            assert!(
                allowed("panels", &account, base, url, CredentialUse::Fetch),
                "{url}"
            );
        }
        for url in [
            "http://books.example:8443/library",
            "https://books.example/library",
            "https://books.example.attacker.invalid:8443/library",
            "https://books.example:8443/library-other",
            "https://books.example:8443/other",
            "https://books.example:8443/library/../other",
            "https://books.example:8443/library/%2e%2e/other",
            "https://books.example:8443/library/a%2fb",
            "https://user@books.example:8443/library",
            "https://books.example:8443/library/a%5cb",
        ] {
            assert!(
                !allowed("panels", &account, base, url, CredentialUse::Fetch),
                "{url}"
            );
        }
        assert!(!allowed(
            "other",
            &account,
            base,
            base,
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "panels",
            &Credential::bearer("komga"),
            base,
            base,
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "panels",
            &Credential::basic("other"),
            base,
            base,
            CredentialUse::Fetch
        ));
        assert!(!allowed(
            "panels",
            &account,
            base,
            base,
            CredentialUse::Post
        ));
        for base in [
            "http://books.example",
            "https://user@books.example",
            "https://books.example?key=x",
            "https://books.example/#a",
            "https://books.example/../a",
        ] {
            assert!(!valid_server(base), "{base}");
        }
    }

    #[test]
    fn private_record_replacement_keeps_scope_and_exact_value_together() {
        let root = std::env::temp_dir().join(format!("cobalt-bound-record-{}", std::process::id()));
        let _ignored = std::fs::remove_dir_all(&root);
        install(
            &root,
            "panels",
            "komga",
            "https://first.example/library",
            "reader:  password  ",
        )
        .unwrap();
        let file = path(&root, "panels", "komga").unwrap();
        let record = decode(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(record.server, "https://first.example/library");
        assert_eq!(record.value, "reader:  password  ");
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(file.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        install(
            &root,
            "panels",
            "komga",
            "https://second.example",
            "reader:replacement",
        )
        .unwrap();
        let complete = std::fs::read(&file).unwrap();
        assert!(install(&root, "panels", "komga", "http://invalid.example", "bad").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), complete);
        let record = decode(std::str::from_utf8(&complete).unwrap()).unwrap();
        assert_eq!(record.server, "https://second.example");
        assert_eq!(record.value, "reader:replacement");
        for invalid in [
            "",
            "cobalt-server-account-v2\nhttps://second.example\nx",
            "cobalt-server-account-v1\nhttps://second.example\nx\nextra",
        ] {
            assert!(decode(invalid).is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
