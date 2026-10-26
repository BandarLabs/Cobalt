//! The Wallabag OAuth session installed by `kobo readlater login`.
//!
//! The CLI exchanges the owner's password grant on a computer and delivers the
//! resulting client and tokens as one shelf file. The application imports that
//! file once, hands the access token to the runtime as a server-bound account
//! so it can never leave the saved host, and keeps only the refresh material in
//! its own store. When the host refuses an expired token, the application
//! refreshes through an uncredentialed token request and installs the
//! replacement the same way.

use kobo_json::{ObjectBuilder, Value};

/// The shelf file the companion CLI writes into this application's data.
pub const SHELF_FILE: &str = "session.v1";
/// The store key holding the imported session after the shelf file is gone.
pub const STORE_KEY: &str = "session";
/// Session files and stored bundles stay far under one article.
pub const LIMIT: usize = 16 * 1024;

/// The durable half of a session: everything a refresh needs, nothing more.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Session {
    pub server: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl Session {
    pub fn token_url(&self) -> String {
        format!("{}/oauth/v2/token", self.server.trim_end_matches('/'))
    }

    /// Wallabag answers form posts; the refresh grant carries every secret in
    /// the body because an uncredentialed task may not set headers.
    pub fn refresh_body(&self) -> String {
        let mut body = String::from("grant_type=refresh_token&client_id=");
        body.push_str(&form(&self.client_id));
        body.push_str("&client_secret=");
        body.push_str(&form(&self.client_secret));
        body.push_str("&refresh_token=");
        body.push_str(&form(&self.refresh_token));
        body
    }
}

/// One form value. Wallabag client and token alphabets are URL-safe, but the
/// encoder costs nothing and keeps a self-hosted secret honest.
fn form(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A freshly imported session: the durable half plus the access token that is
/// handed to the runtime and never kept by the application.
pub struct Import {
    pub session: Session,
    pub access_token: String,
}

/// The shelf file the CLI writes: version, server, OAuth client, and both
/// tokens of the grant it just made.
pub fn decode_import(bytes: &[u8]) -> Option<Import> {
    if bytes.len() > LIMIT {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if value.get("version")?.as_str()? != "1" {
        return None;
    }
    let text = |name: &str| {
        let found = value.get(name)?.as_str()?.trim();
        (!found.is_empty()).then(|| found.to_owned())
    };
    let server = text("server")?;
    if !server.starts_with("https://") {
        return None;
    }
    Some(Import {
        session: Session {
            server,
            client_id: text("client_id")?,
            client_secret: text("client_secret")?,
            refresh_token: text("refresh_token")?,
        },
        access_token: text("access_token")?,
    })
}

pub fn encode(session: &Session) -> Vec<u8> {
    ObjectBuilder::new()
        .set("version", "1")
        .set("server", session.server.clone())
        .set("client_id", session.client_id.clone())
        .set("client_secret", session.client_secret.clone())
        .set("refresh_token", session.refresh_token.clone())
        .build()
        .to_json()
        .into_bytes()
}

pub fn decode(bytes: &[u8]) -> Option<Session> {
    if bytes.len() > LIMIT {
        return None;
    }
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    if value.get("version")?.as_str()? != "1" {
        return None;
    }
    let text = |name: &str| {
        let found = value.get(name)?.as_str()?.trim();
        (!found.is_empty()).then(|| found.to_owned())
    };
    let server = text("server")?;
    if !server.starts_with("https://") {
        return None;
    }
    Some(Session {
        server,
        client_id: text("client_id")?,
        client_secret: text("client_secret")?,
        refresh_token: text("refresh_token")?,
    })
}

/// A token answer carries a replacement access token and, because Wallabag
/// rolls refresh tokens, the next refresh token too.
pub fn parse_token(bytes: &[u8]) -> Option<(String, Option<String>)> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let access = value.get("access_token")?.as_str()?.trim();
    if access.is_empty() {
        return None;
    }
    let refresh = value
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned);
    Some((access.to_owned(), refresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_requires_a_complete_https_session() {
        let file = br#"{"version":"1","server":"https://bag.example","client_id":"1_abc","client_secret":"s3cret","refresh_token":"ref","access_token":"acc"}"#;
        let import = decode_import(file).unwrap();
        assert_eq!(import.session.server, "https://bag.example");
        assert_eq!(import.access_token, "acc");
        assert!(decode_import(br#"{"version":"2","server":"https://bag.example"}"#).is_none());
        assert!(decode_import(
            br#"{"version":"1","server":"http://bag.example","client_id":"x","client_secret":"y","refresh_token":"r","access_token":"a"}"#
        )
        .is_none());
        assert!(decode_import(
            br#"{"version":"1","server":"https://bag.example","client_id":"x","client_secret":"y","refresh_token":"r"}"#
        )
        .is_none());
    }

    #[test]
    fn stored_session_round_trips_without_the_access_token() {
        let session = Session {
            server: "https://bag.example/".into(),
            client_id: "1_abc".into(),
            client_secret: "a secret with spaces".into(),
            refresh_token: "ref".into(),
        };
        assert_eq!(decode(&encode(&session)), Some(session.clone()));
        assert_eq!(session.token_url(), "https://bag.example/oauth/v2/token");
        assert_eq!(
            session.refresh_body(),
            "grant_type=refresh_token&client_id=1_abc&client_secret=a%20secret%20with%20spaces&refresh_token=ref"
        );
    }

    #[test]
    fn token_answers_roll_the_refresh_token() {
        let (access, refresh) =
            parse_token(br#"{"access_token":"new","refresh_token":"rolled","expires_in":3600}"#)
                .unwrap();
        assert_eq!(access, "new");
        assert_eq!(refresh.as_deref(), Some("rolled"));
        let (_, refresh) = parse_token(br#"{"access_token":"new"}"#).unwrap();
        assert_eq!(refresh, None);
        assert!(parse_token(br#"{"error":"invalid_grant"}"#).is_none());
    }
}
