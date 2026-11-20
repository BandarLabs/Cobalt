//! Who may ask this server for anything, decided from a request's head.
//!
//! The server listens on loopback, and loopback is not private. Any page in
//! any browser on this computer can aim a request at `127.0.0.1`, and a page
//! on the public internet can do the same through a hostname it re-points at
//! `127.0.0.1` after it has loaded. What this server does on request is
//! write to somebody's reader, so each of those has to be turned away, and
//! each by a rule of its own, because no one of them covers the others:
//!
//! * The exact `Host` is what stops the re-pointed hostname. Such a page's
//!   requests arrive here carrying its own name, which is never ours. A
//!   token or an `Origin` check alone does not stop it, since by then the
//!   browser considers the page and this server one origin.
//! * The cookie is what stops every other page and program that does not
//!   hold the session. It is `SameSite=Strict`, so the browser attaches it to
//!   nothing another site starts, and `HttpOnly`, so no script reads it.
//! * `Origin` and the request token are what stop a request that changes
//!   something from being started by anything but this page, even from a
//!   browser that holds the cookie.

use std::sync::Mutex;

/// The cookie that carries the session.
pub const COOKIE: &str = "kobo_ui";

/// The header a request that changes something carries its token in.
///
/// A header no form can set, so a request bearing it was made by script,
/// and a script on another origin has to ask permission first, which is
/// never given: this server answers no preflight.
pub const TOKEN_HEADER: &str = "x-kobo-token";

/// Whether `host` names this server and nothing else.
///
/// Exact, port included. A prefix or suffix match is how
/// `127.0.0.1.example.net` or `localhost.example.net` gets in.
#[must_use]
pub fn host_is_ours(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|host| {
        host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
    })
}

/// Whether `origin` is this server's own page.
///
/// A missing `Origin` is a refusal, not a pass. Every browser sends it on
/// the requests this guards, so its absence means the request did not come
/// from the page, and `null`, which a sandboxed frame or a redirect sends,
/// is not this origin either.
#[must_use]
pub fn origin_is_ours(origin: Option<&str>, port: u16) -> bool {
    origin.is_some_and(|origin| {
        origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
    })
}

/// Compares two secrets without stopping at the first difference.
///
/// A comparison that returns early answers a wrong guess faster the more of
/// it is right, and on loopback, where there is no network to blur the
/// timing, that is a way to learn a token a byte at a time.
#[must_use]
pub fn same(offered: &str, held: &str) -> bool {
    let (offered, held) = (offered.as_bytes(), held.as_bytes());
    let mut difference = offered.len() ^ held.len();
    // Walks the held secret's length whatever was offered, so the time
    // taken says nothing about how long the right answer is.
    for (index, held) in held.iter().enumerate() {
        let byte = offered.get(index).copied().unwrap_or(0);
        difference |= usize::from(byte ^ held);
    }
    difference == 0
}

/// One run's secrets.
///
/// Three, because they travel by different roads and one leaking must not
/// give away another. The launch token is the only one ever written where
/// other programs can see it, in the address handed to the browser, so it
/// works once and buys only the cookie. The cookie never reaches a script.
/// The request token is given to the page, which is the only thing that
/// should be asking for changes, and is worthless without the cookie.
#[derive(Debug)]
pub struct Session {
    /// The port the server is listening on, which every rule names.
    pub port: u16,
    launch: Mutex<Option<String>>,
    cookie: String,
    request: String,
}

/// What became of an attempt to trade the launch token for the cookie.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exchange {
    /// The token was right and has now been spent.
    Granted,
    /// The token was wrong, or was right once and has been used.
    Refused,
}

impl Session {
    /// Draws fresh secrets for a server on `port`.
    ///
    /// # Errors
    ///
    /// Only if the system cannot supply random bytes, in which case there is
    /// no safe session to offer and the command stops.
    pub fn new(port: u16) -> Result<Self, String> {
        Ok(Self {
            port,
            launch: Mutex::new(Some(secret()?)),
            cookie: secret()?,
            request: secret()?,
        })
    }

    /// The address that opens the page, with the launch token in it.
    ///
    /// `None` once the token has been spent, so a caller cannot print an
    /// address that no longer opens anything.
    #[must_use]
    pub fn launch_address(&self) -> Option<String> {
        let launch = self.launch.lock().ok()?;
        launch
            .as_ref()
            .map(|token| format!("http://127.0.0.1:{}/?token={token}", self.port))
    }

    /// Spends the launch token if `offered` is it.
    pub fn exchange(&self, offered: &str) -> Exchange {
        let Ok(mut launch) = self.launch.lock() else {
            return Exchange::Refused;
        };
        match launch.as_deref() {
            Some(held) if same(offered, held) => {
                *launch = None;
                Exchange::Granted
            }
            _ => Exchange::Refused,
        }
    }

    /// The `Set-Cookie` value that begins the session.
    ///
    /// No `Secure`, because the page is served over plain HTTP on loopback
    /// and a browser would discard a `Secure` cookie from it. No `Max-Age`,
    /// so it ends with the browser session; the server's own secrets end
    /// sooner, when it exits.
    #[must_use]
    pub fn set_cookie(&self) -> String {
        format!(
            "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict",
            self.cookie
        )
    }

    /// Whether `offered` is this session's cookie.
    #[must_use]
    pub fn holds_cookie(&self, offered: Option<&str>) -> bool {
        offered.is_some_and(|offered| same(offered, &self.cookie))
    }

    /// Whether `offered` is this session's request token.
    #[must_use]
    pub fn holds_token(&self, offered: Option<&str>) -> bool {
        offered.is_some_and(|offered| same(offered, &self.request))
    }

    /// The request token, for the page it is served inside and nothing else.
    #[must_use]
    pub fn request_token(&self) -> &str {
        &self.request
    }
}

/// Thirty-two random bytes as hex.
///
/// 256 bits, so guessing is not an attack worth describing, and hex, so the
/// value needs no escaping in an address, a cookie or a header.
fn secret() -> Result<String, String> {
    use ring::rand::SecureRandom as _;
    use std::fmt::Write as _;
    let mut bytes = [0_u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "this computer could not supply random bytes for a session".to_owned())?;
    let mut hex = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    Ok(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_loopback_names_with_this_port_are_this_host() {
        assert!(host_is_ours(Some("127.0.0.1:4100"), 4100));
        assert!(host_is_ours(Some("localhost:4100"), 4100));
        for other in [
            "127.0.0.1",
            "127.0.0.1:4101",
            "localhost",
            "LOCALHOST:4100",
            "127.0.0.1:4100.example.net",
            "localhost.example.net:4100",
            "example.net:4100",
            "127.0.0.1:04100",
            "[::1]:4100",
            "0.0.0.0:4100",
            " 127.0.0.1:4100",
            "",
        ] {
            assert!(!host_is_ours(Some(other), 4100), "{other:?}");
        }
        assert!(!host_is_ours(None, 4100));
    }

    #[test]
    fn only_this_servers_own_page_is_this_origin() {
        assert!(origin_is_ours(Some("http://127.0.0.1:4100"), 4100));
        assert!(origin_is_ours(Some("http://localhost:4100"), 4100));
        for other in [
            "null",
            "https://127.0.0.1:4100",
            "http://127.0.0.1:4101",
            "http://127.0.0.1",
            "http://127.0.0.1:4100/",
            "http://example.net",
            "http://127.0.0.1:4100.example.net",
            "",
        ] {
            assert!(!origin_is_ours(Some(other), 4100), "{other:?}");
        }
        assert!(!origin_is_ours(None, 4100));
    }

    #[test]
    fn secrets_compare_equal_only_when_every_byte_and_the_length_agree() {
        assert!(same("abc123", "abc123"));
        assert!(!same("abc124", "abc123"));
        assert!(!same("abc12", "abc123"));
        assert!(!same("abc1234", "abc123"));
        assert!(!same("", "abc123"));
        assert!(same("", ""));
    }

    #[test]
    fn the_launch_token_buys_the_cookie_once_and_never_again() {
        let session = Session::new(4100).expect("session");
        let address = session.launch_address().expect("an unspent token");
        let token = address
            .strip_prefix("http://127.0.0.1:4100/?token=")
            .expect("the address is on loopback and carries the token");
        assert_eq!(token.len(), 64);
        assert_eq!(session.exchange("not the token"), Exchange::Refused);
        assert_eq!(session.exchange(token), Exchange::Granted);
        assert_eq!(session.exchange(token), Exchange::Refused);
        assert_eq!(session.launch_address(), None);
    }

    #[test]
    fn the_three_secrets_are_different_and_none_opens_another_door() {
        let session = Session::new(4100).expect("session");
        let address = session.launch_address().expect("address");
        let launch = address.rsplit('=').next().expect("token");
        let cookie = session
            .set_cookie()
            .strip_prefix("kobo_ui=")
            .and_then(|rest| rest.split(';').next().map(str::to_owned))
            .expect("cookie value");
        let request = session.request_token().to_owned();
        assert_ne!(launch, cookie);
        assert_ne!(launch, request);
        assert_ne!(cookie, request);
        assert!(session.holds_cookie(Some(&cookie)));
        assert!(!session.holds_cookie(Some(launch)));
        assert!(!session.holds_cookie(Some(&request)));
        assert!(!session.holds_cookie(None));
        assert!(session.holds_token(Some(&request)));
        assert!(!session.holds_token(Some(&cookie)));
        assert!(!session.holds_token(Some(launch)));
        assert!(!session.holds_token(None));
    }

    #[test]
    fn the_cookie_is_kept_from_scripts_and_from_other_sites() {
        let cookie = Session::new(4100).expect("session").set_cookie();
        assert!(cookie.contains("; HttpOnly"));
        assert!(cookie.contains("; SameSite=Strict"));
        assert!(cookie.contains("; Path=/"));
        assert!(!cookie.contains("Domain="));
    }

    #[test]
    fn two_sessions_never_share_a_secret() {
        let first = Session::new(4100).expect("session");
        let second = Session::new(4100).expect("session");
        assert_ne!(first.launch_address(), second.launch_address());
        assert_ne!(first.set_cookie(), second.set_cookie());
        assert_ne!(first.request_token(), second.request_token());
    }
}
