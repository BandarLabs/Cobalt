use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use kobo_json::{ObjectBuilder, Value};
use kobo_protocol::{AppLinkState, DeviceError, DeviceResult, RemoteInstallOutcome};
use p256::ecdh::diffie_hellman;
use p256::ecdsa::signature::Verifier as _;
use p256::pkcs8::{DecodePublicKey, EncodePublicKey};
use p256::{PublicKey, SecretKey};
use ring::{aead, digest, hkdf, hmac};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const RELAY_URL: &str = "https://cobalt-install-relay.anandabhishek.workers.dev";
const STATE_DIRECTORY: &str = "app-link";
const PRIVATE_KEY_FILE: &str = "device-key";
const CREDENTIAL_FILE: &str = "credential.json";
const PENDING_FILE: &str = "pending.json";
const COMPLETED_FILE: &str = "completed";
const BROWSERS_FILE: &str = "browsers";
const MAX_RELAY_BODY: u32 = 16 * 1024;
const MAX_HTTP_RESPONSE: u64 = 48 * 1024;
const COMMAND_TTL_SECONDS: u64 = 72 * 60 * 60;
const INSTALL_COMPLETION_TTL_SECONDS: u64 = 15 * 60;
const CLOCK_SKEW_SECONDS: u64 = 5 * 60;
const COMPLETED_LIMIT: usize = 64;
const PINNED_BROWSER_LIMIT: usize = 8;
const HKDF_INFO: &[u8] = b"cobalt-app-install-v1";
const PAIR_PROOF_CONTEXT: &str = "cobalt-pair-proof-v1";
const INSTALL_SIGNATURE_CONTEXT: &str = "cobalt-install-v2";
static RELAY_TLS_CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();

pub fn read(root: &Path) -> Result<DeviceResult, DeviceError> {
    let mut relay = HttpsRelay::new()?;
    read_with(root, &mut relay, now())
}

pub fn begin(root: &Path) -> Result<DeviceResult, DeviceError> {
    let mut relay = HttpsRelay::new()?;
    begin_with(root, &mut relay, now(), &device_name())
}

pub fn poll(root: &Path) -> Result<DeviceResult, DeviceError> {
    let mut relay = HttpsRelay::new()?;
    let channel = crate::autoupdate::preferences(root).channel;
    poll_with(
        root,
        &mut relay,
        now(),
        |root, id| crate::app_store::prepare_remote_install(root, id, channel),
        |root, id| crate::app_store::install(root, id, channel),
    )
}

pub fn disconnect(root: &Path) -> Result<DeviceResult, DeviceError> {
    let mut relay = HttpsRelay::new()?;
    disconnect_with(root, &mut relay)
}

pub fn maintenance(root: &Path, action: &str) -> Result<String, DeviceError> {
    let result = match action {
        "status" => read(root)?,
        "unpair" => disconnect(root)?,
        _ => return Err(DeviceError::InvalidInput),
    };
    match result {
        DeviceResult::AppLink(AppLinkState::Unpaired) => Ok("unpaired".to_owned()),
        DeviceResult::AppLink(AppLinkState::Pairing {
            code, expires_in, ..
        }) => Ok(format!("pairing {code}, expires in {expires_in}s")),
        DeviceResult::AppLink(AppLinkState::Paired { browsers }) => {
            Ok(format!("paired with {browsers} browser(s)"))
        }
        _ => Err(DeviceError::Backend),
    }
}

fn device_name() -> String {
    "Kobo reader".to_owned()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

trait Relay {
    fn send(
        &mut self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> Result<Vec<u8>, DeviceError>;
}

struct HttpsRelay {
    base: String,
}

impl HttpsRelay {
    fn new() -> Result<Self, DeviceError> {
        let base = std::env::var("KOBO_INSTALL_RELAY_URL").unwrap_or_else(|_| RELAY_URL.to_owned());
        if !base.starts_with("https://") || base.contains(['\r', '\n']) {
            return Err(DeviceError::InvalidInput);
        }
        Ok(Self {
            base: base.trim_end_matches('/').to_owned(),
        })
    }
}

impl Relay for HttpsRelay {
    fn send(
        &mut self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> Result<Vec<u8>, DeviceError> {
        if !matches!(method, "GET" | "POST" | "DELETE")
            || !path.starts_with('/')
            || path.contains(['\r', '\n'])
            || token.is_some_and(|token| !valid_token(token))
        {
            return Err(DeviceError::InvalidInput);
        }
        let address = kobo_net::parse(&format!("{}{}", self.base, path)).map_err(network_error)?;
        let config = relay_tls_config()?;
        let server_name = address
            .host
            .clone()
            .try_into()
            .map_err(|_| DeviceError::InvalidInput)?;
        let mut addresses = (address.host.as_str(), address.port)
            .to_socket_addrs()
            .map_err(|_| DeviceError::Unreachable)?;
        let socket = addresses
            .find_map(|address| TcpStream::connect_timeout(&address, Duration::from_secs(30)).ok())
            .ok_or(DeviceError::Unreachable)?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .and_then(|()| socket.set_write_timeout(Some(Duration::from_secs(60))))
            .map_err(|_| DeviceError::Unreachable)?;
        let connection = rustls::ClientConnection::new(config, server_name)
            .map_err(|_| DeviceError::Unreachable)?;
        let mut stream = rustls::StreamOwned::new(connection, socket);
        let body = body.unwrap_or_default().as_bytes();
        let host = if address.port == 443 {
            address.host.clone()
        } else {
            format!("{}:{}", address.host, address.port)
        };
        let mut head = format!(
            "{method} {} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nAccept-Encoding: identity\r\nUser-Agent: kobo-runtime\r\n",
            address.path
        );
        if let Some(token) = token {
            head.push_str("Authorization: Bearer ");
            head.push_str(token);
            head.push_str("\r\n");
        }

        if method == "POST" {
            head.push_str("Content-Type: application/json\r\n");
            write!(head, "Content-Length: {}\r\n", body.len()).map_err(|_| DeviceError::Backend)?;
        }
        head.push_str("\r\n");
        stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(body))
            .map_err(|_| DeviceError::Unreachable)?;
        let mut response = Vec::new();
        stream
            .take(MAX_HTTP_RESPONSE + 1)
            .read_to_end(&mut response)
            .map_err(|_| DeviceError::Unreachable)?;
        if response.len() as u64 > MAX_HTTP_RESPONSE {
            return Err(DeviceError::Backend);
        }
        match kobo_net::split_response(&response, MAX_RELAY_BODY) {
            Ok(kobo_net::Response::Body(body)) => Ok(body.into_owned()),
            Ok(kobo_net::Response::Redirect(_)) => Err(DeviceError::Unreachable),
            // A relay overload or outage is retryable; only a definite client
            // error such as 404 may be treated as the relay's final verdict.
            Err(kobo_protocol::TaskError::NotFound) if transient_status(&response) => {
                Err(DeviceError::Unreachable)
            }
            Err(error) => Err(network_error(error)),
        }
    }
}

fn transient_status(response: &[u8]) -> bool {
    let line = response.split(|byte| *byte == b'\r').next().unwrap_or(&[]);
    let mut parts = line
        .split(|byte| *byte == b' ')
        .filter(|part| !part.is_empty());
    parts
        .nth(1)
        .and_then(|code| std::str::from_utf8(code).ok())
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| matches!(code, 408 | 425 | 429 | 500..=599))
}

fn relay_tls_config() -> Result<Arc<rustls::ClientConfig>, DeviceError> {
    if let Some(config) = RELAY_TLS_CONFIG.get() {
        return Ok(Arc::clone(config));
    }
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|_| DeviceError::Unreachable)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::clone(
        RELAY_TLS_CONFIG.get_or_init(|| Arc::new(config)),
    ))
}

fn network_error(error: kobo_protocol::TaskError) -> DeviceError {
    match error {
        kobo_protocol::TaskError::Unauthorized => DeviceError::Authentication,
        kobo_protocol::TaskError::Offline
        | kobo_protocol::TaskError::Unreachable
        | kobo_protocol::TaskError::TimedOut => DeviceError::Unreachable,
        kobo_protocol::TaskError::NotFound => DeviceError::NotFound,
        kobo_protocol::TaskError::TooLarge
        | kobo_protocol::TaskError::Denied
        | kobo_protocol::TaskError::NoCredential => DeviceError::Backend,
    }
}

#[derive(Clone, Debug)]
struct Identity {
    secret: SecretKey,
}

impl Identity {
    fn load_or_create(root: &Path) -> Result<Self, DeviceError> {
        let directory = state_root(root);
        fs::create_dir_all(&directory).map_err(|_| DeviceError::Backend)?;
        set_mode(&directory, 0o700)?;
        let path = directory.join(PRIVATE_KEY_FILE);
        match fs::read(&path) {
            Ok(bytes) => {
                let secret = SecretKey::from_slice(&bytes).map_err(|_| DeviceError::Integrity)?;
                Ok(Self { secret })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = [0_u8; 32];
                let secret = loop {
                    random_bytes(&mut bytes)?;
                    if let Ok(secret) = SecretKey::from_slice(&bytes) {
                        break secret;
                    }
                };
                atomic_write(&path, secret.to_bytes().as_ref(), 0o600)?;
                Ok(Self { secret })
            }
            Err(_) => Err(DeviceError::Backend),
        }
    }

    fn public_key(&self) -> Result<String, DeviceError> {
        self.secret
            .public_key()
            .to_public_key_der()
            .map(|document| URL_SAFE_NO_PAD.encode(document.as_bytes()))
            .map_err(|_| DeviceError::Backend)
    }

    /// A short digest of the public key, published only through the QR code
    /// fragment so a browser can detect a relay substituting its own key.
    fn public_key_fingerprint(&self) -> Result<String, DeviceError> {
        let public = self.public_key()?;
        let digest = digest::digest(&digest::SHA256, public.as_bytes());
        Ok(URL_SAFE_NO_PAD.encode(&digest.as_ref()[..16]))
    }
}

fn random_bytes(bytes: &mut [u8]) -> Result<(), DeviceError> {
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(bytes))
        .map_err(|_| DeviceError::Backend)
}

fn new_link_secret() -> Result<String, DeviceError> {
    let mut bytes = [0_u8; 16];
    random_bytes(&mut bytes)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Credential {
    device_id: String,
    token: String,
    pairing: Option<Pairing>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pairing {
    code: String,
    url: String,
    expires_at: u64,
    /// Shared only through the QR code fragment, never through the relay.
    /// A browser proves its signing key with an HMAC keyed by this secret.
    secret: String,
}

impl Credential {
    fn load(root: &Path) -> Result<Option<Self>, DeviceError> {
        let bytes = match fs::read(state_root(root).join(CREDENTIAL_FILE)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(DeviceError::Backend),
        };
        let value = parse_json(&bytes)?;
        let version = value.get("version").and_then(Value::as_i64);
        let device_id = value.get("device_id").and_then(Value::as_str);
        let token = value.get("device_token").and_then(Value::as_str);
        if version != Some(1)
            || !device_id.is_some_and(valid_uuid)
            || !token.is_some_and(valid_token)
        {
            return Err(DeviceError::Integrity);
        }
        let pairing = match value.get("pairing") {
            Some(Value::Null) | None => None,
            Some(value) => {
                let code = value.get("code").and_then(Value::as_str);
                let url = value.get("url").and_then(Value::as_str);
                let secret = value.get("secret").and_then(Value::as_str);
                let expires_at = value
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok());
                if !code.is_some_and(valid_pairing_code)
                    || !url.is_some_and(valid_https_url)
                    || !secret.is_some_and(valid_link_secret)
                    || expires_at.is_none()
                {
                    return Err(DeviceError::Integrity);
                }
                Some(Pairing {
                    code: code.unwrap_or_default().to_owned(),
                    url: url.unwrap_or_default().to_owned(),
                    expires_at: expires_at.unwrap_or_default(),
                    secret: secret.unwrap_or_default().to_owned(),
                })
            }
        };
        Ok(Some(Self {
            device_id: device_id.unwrap_or_default().to_owned(),
            token: token.unwrap_or_default().to_owned(),
            pairing,
        }))
    }

    fn save(&self, root: &Path) -> Result<(), DeviceError> {
        let pairing = self.pairing.as_ref().map_or(Value::Null, |pairing| {
            ObjectBuilder::new()
                .set("code", pairing.code.clone())
                .set("url", pairing.url.clone())
                .set("expires_at", pairing.expires_at.to_string())
                .set("secret", pairing.secret.clone())
                .build()
        });
        let body = ObjectBuilder::new()
            .set("version", 1_i32)
            .set("device_id", self.device_id.clone())
            .set("device_token", self.token.clone())
            .set("pairing", pairing)
            .build()
            .to_json();
        atomic_write(
            &state_root(root).join(CREDENTIAL_FILE),
            body.as_bytes(),
            0o600,
        )
    }
}

fn begin_with(
    root: &Path,
    relay: &mut impl Relay,
    now: u64,
    name: &str,
) -> Result<DeviceResult, DeviceError> {
    let identity = Identity::load_or_create(root)?;
    let mut credential = if let Some(mut credential) = Credential::load(root)? {
        let path = format!("/v1/devices/{}/pairings", credential.device_id);
        let response = relay.send("POST", &path, Some(&credential.token), Some("{}"))?;
        let mut pairing = parse_pairing(&response)?;
        pairing.secret = new_link_secret()?;
        credential.pairing = Some(pairing);
        credential.save(root)?;
        credential
    } else {
        let body = ObjectBuilder::new()
            .set("device_name", name)
            .set("device_public_key", identity.public_key()?)
            .build()
            .to_json();
        let response = relay.send("POST", "/v1/pairings", None, Some(&body))?;
        let value = parse_json(&response)?;
        let device_id = required_string(&value, "device_id")?;
        let token = required_string(&value, "device_token")?;
        if !valid_uuid(&device_id) || !valid_token(&token) {
            return Err(DeviceError::Integrity);
        }
        let pairing = parse_pairing_value(&value)?;
        let credential = Credential {
            device_id,
            token,
            pairing: Some(Pairing {
                secret: new_link_secret()?,
                ..pairing
            }),
        };
        credential.save(root)?;
        credential
    };
    let state = local_pairing_state(&credential, &identity, now).unwrap_or(AppLinkState::Unpaired);
    if matches!(state, AppLinkState::Unpaired) {
        credential.pairing = None;
        credential.save(root)?;
    }
    Ok(DeviceResult::AppLink(state))
}

fn read_with(root: &Path, relay: &mut impl Relay, now: u64) -> Result<DeviceResult, DeviceError> {
    let Some(mut credential) = Credential::load(root)? else {
        return Ok(DeviceResult::AppLink(AppLinkState::Unpaired));
    };
    if credential
        .pairing
        .as_ref()
        .is_some_and(|pairing| pairing.expires_at <= now)
    {
        credential.pairing = None;
        credential.save(root)?;
    }
    let path = format!("/v1/devices/{}/pairing", credential.device_id);
    let response = relay.send("GET", &path, Some(&credential.token), None)?;
    let value = parse_json(&response)?;
    let paired = value
        .get("paired")
        .and_then(Value::as_bool)
        .ok_or(DeviceError::Integrity)?;
    let browsers = value
        .get("browser_count")
        .and_then(Value::as_i64)
        .and_then(|count| u8::try_from(count).ok())
        .filter(|count| *count <= 8)
        .ok_or(DeviceError::Integrity)?;
    if paired {
        if browsers == 0 {
            return Err(DeviceError::Integrity);
        }
        let pairing = credential.pairing.as_ref();
        let added = sync_browser_pins(root, pairing, &value)?;
        if pairing.is_some() {
            if added == 0 {
                return Err(DeviceError::Integrity);
            }
            credential.pairing = None;
            credential.save(root)?;
        }
        if load_browsers(root)?.is_empty() {
            return Err(DeviceError::Integrity);
        }
        return Ok(DeviceResult::AppLink(AppLinkState::Paired { browsers }));
    }
    let identity = Identity::load_or_create(root)?;
    let state = local_pairing_state(&credential, &identity, now).unwrap_or(AppLinkState::Unpaired);
    if matches!(state, AppLinkState::Unpaired) && credential.pairing.take().is_some() {
        credential.save(root)?;
    }
    Ok(DeviceResult::AppLink(state))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PinnedBrowser {
    public_key: String,
    proven: bool,
}

/// Pins browser signing keys reported by the relay. A key joins the pinned
/// set only while a pairing this device created is outstanding: a valid HMAC
/// proof from the QR fragment secret pins a verified sender. Manual entry uses
/// that same secret, so a proofless relay key is never accepted. Keys the relay
/// reports at any other moment are ignored, so a later-compromised relay cannot
/// add senders.
fn sync_browser_pins(
    root: &Path,
    pairing: Option<&Pairing>,
    value: &Value,
) -> Result<usize, DeviceError> {
    let entries = value
        .get("browser_keys")
        .and_then(Value::as_array)
        .ok_or(DeviceError::Integrity)?;
    if entries.len() > PINNED_BROWSER_LIMIT {
        return Err(DeviceError::Integrity);
    }
    let mut pinned = load_browsers(root)?;
    let mut changed = false;
    let mut added = 0;
    for entry in entries {
        let key = entry
            .get("public_key")
            .and_then(Value::as_str)
            .filter(|key| valid_browser_key(key))
            .ok_or(DeviceError::Integrity)?;
        if pinned.iter().any(|browser| browser.public_key == key) {
            continue;
        }
        let proof = match entry.get("proof") {
            None | Some(Value::Null) => None,
            Some(proof) => Some(
                proof
                    .as_str()
                    .filter(|proof| valid_pair_proof(proof))
                    .ok_or(DeviceError::Integrity)?,
            ),
        };
        let Some(pairing) = pairing else { continue };
        let Some(proof) = proof else { continue };
        if !verify_pair_proof(&pairing.secret, key, proof) {
            continue;
        }
        if pinned.len() >= PINNED_BROWSER_LIMIT {
            break;
        }
        pinned.push(PinnedBrowser {
            public_key: key.to_owned(),
            proven: true,
        });
        changed = true;
        added += 1;
    }
    if changed {
        save_browsers(root, &pinned)?;
    }
    Ok(added)
}

fn verify_pair_proof(secret: &str, browser_key: &str, proof: &str) -> bool {
    let Ok(secret) = URL_SAFE_NO_PAD.decode(secret) else {
        return false;
    };
    let Ok(proof) = URL_SAFE_NO_PAD.decode(proof) else {
        return false;
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, &secret);
    let mut message = Vec::new();
    message.extend_from_slice(PAIR_PROOF_CONTEXT.as_bytes());
    message.push(b'\n');
    message.extend_from_slice(browser_key.as_bytes());
    hmac::verify(&key, &message, &proof).is_ok()
}

fn load_browsers(root: &Path) -> Result<Vec<PinnedBrowser>, DeviceError> {
    let text = match fs::read_to_string(state_root(root).join(BROWSERS_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(DeviceError::Backend),
    };
    let mut browsers = Vec::new();
    for line in text.lines() {
        let (kind, key) = line.split_once(' ').ok_or(DeviceError::Integrity)?;
        let proven = match kind {
            "proven" => true,
            "tofu" => false,
            _ => return Err(DeviceError::Integrity),
        };
        if !valid_browser_key(key) || browsers.len() >= PINNED_BROWSER_LIMIT {
            return Err(DeviceError::Integrity);
        }
        browsers.push(PinnedBrowser {
            public_key: key.to_owned(),
            proven,
        });
    }
    Ok(browsers)
}

fn save_browsers(root: &Path, browsers: &[PinnedBrowser]) -> Result<(), DeviceError> {
    let mut body = String::new();
    for browser in browsers {
        body.push_str(if browser.proven { "proven " } else { "tofu " });
        body.push_str(&browser.public_key);
        body.push('\n');
    }
    atomic_write(
        &state_root(root).join(BROWSERS_FILE),
        body.as_bytes(),
        0o600,
    )
}

fn poll_with(
    root: &Path,
    relay: &mut impl Relay,
    now: u64,
    prepare: impl FnOnce(&Path, &str) -> Result<crate::app_store::RemoteInstallPlan, DeviceError>,
    install: impl FnOnce(&Path, &str) -> Result<(), DeviceError>,
) -> Result<DeviceResult, DeviceError> {
    let Some(credential) = Credential::load(root)? else {
        return Ok(DeviceResult::AppLink(AppLinkState::Unpaired));
    };
    if let Some(pending) = Pending::load(root)? {
        return resume_pending(root, relay, &credential, pending, now, install);
    }
    let status = read_with(root, relay, now)?;
    if !matches!(status, DeviceResult::AppLink(AppLinkState::Paired { .. })) {
        return Ok(status);
    }
    let path = format!("/v1/devices/{}/commands", credential.device_id);
    let response = relay.send("GET", &path, Some(&credential.token), None)?;
    let value = parse_json(&response)?;
    let Some(command) = value.get("command") else {
        return Err(DeviceError::Integrity);
    };
    if matches!(command, Value::Null) {
        return Ok(DeviceResult::RemoteInstall(RemoteInstallOutcome::None));
    }
    process_command(root, relay, &credential, command, now, prepare, install)
}

fn validate_command(
    root: &Path,
    credential: &Credential,
    command: &Value,
    now: u64,
) -> Result<(InstallRequest, u64), (Option<&'static str>, DeviceError)> {
    let invalid = |error| (Some("invalid-command"), error);
    let (created_at, expires_at) = required_string(command, "created_at")
        .and_then(|value| parse_timestamp(&value).ok_or(DeviceError::Integrity))
        .and_then(|created_at| {
            required_string(command, "expires_at")
                .and_then(|value| parse_timestamp(&value).ok_or(DeviceError::Integrity))
                .map(|expires_at| (created_at, expires_at))
        })
        .map_err(invalid)?;
    if expires_at <= now
        || expires_at.saturating_sub(created_at) > COMMAND_TTL_SECONDS
        || created_at > now.saturating_add(CLOCK_SKEW_SECONDS)
    {
        return Err((Some("expired"), DeviceError::InvalidInput));
    }
    let envelope = command
        .get("envelope")
        .ok_or_else(|| invalid(DeviceError::Integrity))?;
    let sender = command
        .get("browser_public_key")
        .and_then(Value::as_str)
        .filter(|key| valid_browser_key(key))
        .ok_or_else(|| invalid(DeviceError::Integrity))?;
    let signature = command
        .get("signature")
        .and_then(Value::as_str)
        .filter(|signature| valid_install_signature(signature))
        .ok_or_else(|| invalid(DeviceError::Integrity))?;
    if !load_browsers(root)
        .map_err(|error| (None, error))?
        .iter()
        .any(|browser| browser.public_key == sender)
    {
        return Err((Some("unknown-sender"), DeviceError::Integrity));
    }
    if !verify_install_signature(sender, signature, &credential.device_id, envelope) {
        return Err((Some("invalid-signature"), DeviceError::Integrity));
    }
    let identity = Identity::load_or_create(root).map_err(|error| (None, error))?;
    let request = decrypt_command(envelope, &identity.secret, &credential.device_id)
        .map_err(|error| (Some("invalid-envelope"), error))?;
    // Freshness and uniqueness come from inside the sealed request, so the
    // relay cannot replay an old envelope under a new command identity.
    if request.requested_at > now.saturating_add(CLOCK_SKEW_SECONDS)
        || now > request.requested_at.saturating_add(COMMAND_TTL_SECONDS)
    {
        return Err((Some("expired"), DeviceError::InvalidInput));
    }
    if completed(root)
        .map_err(|error| (None, error))?
        .iter()
        .any(|known| known == &request.request_id)
    {
        return Err((Some("replayed-command"), DeviceError::Integrity));
    }
    Ok((request, expires_at))
}

fn process_command(
    root: &Path,
    relay: &mut impl Relay,
    credential: &Credential,
    command: &Value,
    now: u64,
    prepare: impl FnOnce(&Path, &str) -> Result<crate::app_store::RemoteInstallPlan, DeviceError>,
    install: impl FnOnce(&Path, &str) -> Result<(), DeviceError>,
) -> Result<DeviceResult, DeviceError> {
    let command_id = required_string(command, "id")?;
    if !valid_uuid(&command_id) {
        return Err(DeviceError::Integrity);
    }
    let (request, expires_at) = match validate_command(root, credential, command, now) {
        Ok(validated) => validated,
        Err((None, error)) => return Err(error),
        Err((Some(failure), error)) => {
            return reject_command(root, relay, credential, &command_id, failure, error);
        }
    };
    let app_id = request.app_id;
    let plan = match prepare(root, &app_id) {
        Ok(plan) => plan,
        Err(error) => {
            return reject_command(
                root,
                relay,
                credential,
                &command_id,
                failure_code(error),
                error,
            );
        }
    };
    let pending = Pending {
        command_id,
        request_id: request.request_id,
        app_id,
        expires_at,
        phase: PendingPhase::Ready {
            install: plan.install,
            outcome: plan.outcome,
        },
    };
    pending.save(root)?;
    resume_pending(root, relay, credential, pending, now, install)
}

fn resume_pending(
    root: &Path,
    relay: &mut impl Relay,
    credential: &Credential,
    pending: Pending,
    now: u64,
    install: impl FnOnce(&Path, &str) -> Result<(), DeviceError>,
) -> Result<DeviceResult, DeviceError> {
    match pending.phase.clone() {
        PendingPhase::Ready {
            install: run,
            outcome,
        } => {
            if pending.expires_at <= now {
                let final_pending = pending.final_failure("expired", DeviceError::InvalidInput);
                final_pending.save(root)?;
                return finish_pending(root, relay, credential, &final_pending);
            }
            send_ack(relay, credential, &pending.command_id, Ack::Installing)?;
            let pending = Pending {
                expires_at: pending
                    .expires_at
                    .max(now.saturating_add(INSTALL_COMPLETION_TTL_SECONDS)),
                ..pending
            };
            pending.save(root)?;
            if matches!(outcome, RemoteInstallOutcome::Unavailable { .. }) {
                let final_pending = pending.final_outcome_failure("unavailable", outcome.clone());
                final_pending.save(root)?;
                return finish_pending(root, relay, credential, &final_pending);
            }
            if matches!(outcome, RemoteInstallOutcome::RequiresCobalt { .. }) {
                let final_pending =
                    pending.final_outcome_failure("requires-cobalt", outcome.clone());
                final_pending.save(root)?;
                return finish_pending(root, relay, credential, &final_pending);
            }
            if run {
                if let Err(error) = install(root, &pending.app_id) {
                    let final_pending = pending.final_failure(failure_code(error), error);
                    final_pending.save(root)?;
                    return finish_pending(root, relay, credential, &final_pending);
                }
            }
            let final_pending = pending.final_success(outcome);
            final_pending.save(root)?;
            finish_pending(root, relay, credential, &final_pending)
        }
        PendingPhase::Final { .. } => finish_pending(root, relay, credential, &pending),
    }
}

fn finish_pending(
    root: &Path,
    relay: &mut impl Relay,
    credential: &Credential,
    pending: &Pending,
) -> Result<DeviceResult, DeviceError> {
    let PendingPhase::Final { ack, report } = pending.phase.clone() else {
        return Err(DeviceError::Backend);
    };
    match send_ack(relay, credential, &pending.command_id, ack) {
        Ok(()) | Err(DeviceError::NotFound) => {
            if !pending.request_id.is_empty() {
                remember_completed(root, &pending.request_id)?;
            }
            remove_state_file(root, PENDING_FILE)?;
        }
        Err(error) => return Err(error),
    }
    match report {
        Report::Outcome(outcome) => Ok(DeviceResult::RemoteInstall(outcome)),
        Report::Error(error) => Err(error),
    }
}

fn reject_command(
    root: &Path,
    relay: &mut impl Relay,
    credential: &Credential,
    command_id: &str,
    failure: &str,
    report: DeviceError,
) -> Result<DeviceResult, DeviceError> {
    let pending = Pending {
        command_id: command_id.to_owned(),
        request_id: String::new(),
        app_id: String::new(),
        expires_at: now(),
        phase: PendingPhase::Final {
            ack: Ack::Failed(failure.to_owned()),
            report: Report::Error(report),
        },
    };
    pending.save(root)?;
    finish_pending(root, relay, credential, &pending)
}

fn disconnect_with(root: &Path, relay: &mut impl Relay) -> Result<DeviceResult, DeviceError> {
    // A missing or unreadable credential cannot authenticate a remote revoke,
    // but unpairing must still succeed as a guaranteed local reset.
    let remote = match Credential::load(root) {
        Ok(Some(credential)) => {
            let path = format!("/v1/devices/{}", credential.device_id);
            relay.send("DELETE", &path, Some(&credential.token), None)
        }
        Ok(None) | Err(_) => Ok(Vec::new()),
    };
    remove_state_file(root, CREDENTIAL_FILE)?;
    remove_state_file(root, PENDING_FILE)?;
    remove_state_file(root, COMPLETED_FILE)?;
    remove_state_file(root, BROWSERS_FILE)?;
    remove_state_file(root, PRIVATE_KEY_FILE)?;
    match remote {
        Ok(_)
        | Err(
            DeviceError::NotFound
            | DeviceError::Authentication
            | DeviceError::TimedOut
            | DeviceError::Unreachable,
        ) => {}
        Err(error) => return Err(error),
    }
    Ok(DeviceResult::AppLink(AppLinkState::Unpaired))
}

fn parse_pairing(response: &[u8]) -> Result<Pairing, DeviceError> {
    parse_pairing_value(&parse_json(response)?)
}

fn parse_pairing_value(value: &Value) -> Result<Pairing, DeviceError> {
    let code = value
        .get("pairing_code")
        .or_else(|| value.get("code"))
        .and_then(Value::as_str)
        .ok_or(DeviceError::Integrity)?;
    let url = value
        .get("pairing_url")
        .or_else(|| value.get("url"))
        .and_then(Value::as_str)
        .ok_or(DeviceError::Integrity)?;
    let expires_at = required_string(value, "expires_at")
        .and_then(|value| parse_timestamp(&value).ok_or(DeviceError::Integrity))?;
    if !valid_pairing_code(code) || !valid_https_url(url) {
        return Err(DeviceError::Integrity);
    }
    Ok(Pairing {
        code: code.to_owned(),
        url: url.to_owned(),
        expires_at,
        secret: String::new(),
    })
}

fn local_pairing_state(
    credential: &Credential,
    identity: &Identity,
    now: u64,
) -> Option<AppLinkState> {
    let pairing = credential.pairing.as_ref()?;
    let remaining = pairing.expires_at.checked_sub(now)?;
    // The fragment travels inside the QR code only: browsers never send it to
    // the relay, so it can carry the device key digest and the pairing proof
    // secret that keep the relay honest.
    let url = format!(
        "{}#k={}&s={}",
        pairing.url,
        identity.public_key_fingerprint().ok()?,
        pairing.secret
    );
    Some(AppLinkState::Pairing {
        code: pairing.code.clone(),
        url,
        expires_in: u32::try_from(remaining.min(10 * 60)).unwrap_or(10 * 60),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct InstallRequest {
    app_id: String,
    request_id: String,
    requested_at: u64,
}

fn verify_install_signature(
    sender: &str,
    signature: &str,
    device_id: &str,
    envelope: &Value,
) -> bool {
    let (Some(ephemeral), Some(nonce), Some(ciphertext)) = (
        envelope.get("ephemeral_public_key").and_then(Value::as_str),
        envelope.get("nonce").and_then(Value::as_str),
        envelope.get("ciphertext").and_then(Value::as_str),
    ) else {
        return false;
    };
    let Ok(sender) = URL_SAFE_NO_PAD.decode(sender) else {
        return false;
    };
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let Ok(key) = PublicKey::from_public_key_der(&sender) else {
        return false;
    };
    let Ok(signature) = p256::ecdsa::Signature::from_slice(&signature) else {
        return false;
    };
    let message =
        format!("{INSTALL_SIGNATURE_CONTEXT}\n{device_id}\n{ephemeral}\n{nonce}\n{ciphertext}");
    p256::ecdsa::VerifyingKey::from(&key)
        .verify(message.as_bytes(), &signature)
        .is_ok()
}

fn decrypt_command(
    envelope: &Value,
    secret: &SecretKey,
    device_id: &str,
) -> Result<InstallRequest, DeviceError> {
    if required_string(envelope, "algorithm")? != "ECDH-P256-AES-256-GCM" {
        return Err(DeviceError::Integrity);
    }
    let public = decode_bounded(&required_string(envelope, "ephemeral_public_key")?, 91, 91)?;
    let nonce = decode_bounded(&required_string(envelope, "nonce")?, 12, 12)?;
    let mut ciphertext = decode_bounded(&required_string(envelope, "ciphertext")?, 17, 768)?;
    let public = PublicKey::from_public_key_der(&public).map_err(|_| DeviceError::Integrity)?;
    let shared = diffie_hellman(secret.to_nonzero_scalar(), public.as_affine());
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]);
    let prk = salt.extract(shared.raw_secret_bytes().as_ref());
    let info = [HKDF_INFO];
    let okm = prk
        .expand(&info, AesKeyLength)
        .map_err(|_| DeviceError::Integrity)?;
    let mut key = [0_u8; 32];
    okm.fill(&mut key).map_err(|_| DeviceError::Integrity)?;
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .map(aead::LessSafeKey::new)
        .map_err(|_| DeviceError::Integrity)?;
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| DeviceError::Integrity)?;
    let plaintext = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(device_id.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| DeviceError::Integrity)?;
    let value = parse_json(plaintext)?;
    let Value::Object(fields) = &value else {
        return Err(DeviceError::InvalidInput);
    };
    if fields.len() != 4 || value.get("version").and_then(Value::as_i64) != Some(2) {
        return Err(DeviceError::InvalidInput);
    }
    let id = value
        .get("app_id")
        .and_then(Value::as_str)
        .ok_or(DeviceError::InvalidInput)?;
    if !kobo_protocol::valid_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .filter(|request_id| valid_request_id(request_id))
        .ok_or(DeviceError::InvalidInput)?;
    let requested_at = value
        .get("requested_at")
        .and_then(Value::as_i64)
        .and_then(|requested_at| u64::try_from(requested_at).ok())
        .ok_or(DeviceError::InvalidInput)?;
    Ok(InstallRequest {
        app_id: id.to_owned(),
        request_id: request_id.to_owned(),
        requested_at,
    })
}

struct AesKeyLength;

impl hkdf::KeyType for AesKeyLength {
    fn len(&self) -> usize {
        32
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pending {
    command_id: String,
    request_id: String,
    app_id: String,
    expires_at: u64,
    phase: PendingPhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingPhase {
    Ready {
        install: bool,
        outcome: RemoteInstallOutcome,
    },
    Final {
        ack: Ack,
        report: Report,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Ack {
    Installing,
    Installed(RemoteInstallOutcome),
    Failed(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Report {
    Outcome(RemoteInstallOutcome),
    Error(DeviceError),
}

impl Pending {
    fn load(root: &Path) -> Result<Option<Self>, DeviceError> {
        let bytes = match fs::read(state_root(root).join(PENDING_FILE)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(DeviceError::Backend),
        };
        let value = parse_json(&bytes)?;
        if value.get("version").and_then(Value::as_i64) != Some(1) {
            return Err(DeviceError::Integrity);
        }
        let command_id = required_string(&value, "command_id")?;
        let request_id = value
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let app_id = required_string(&value, "app_id")?;
        let expires_at = required_string(&value, "expires_at")?
            .parse::<u64>()
            .map_err(|_| DeviceError::Integrity)?;
        if !valid_uuid(&command_id)
            || (!app_id.is_empty() && !kobo_protocol::valid_app_id(&app_id))
            || (!request_id.is_empty() && !valid_request_id(&request_id))
        {
            return Err(DeviceError::Integrity);
        }
        let phase = match required_string(&value, "phase")?.as_str() {
            "ready" => PendingPhase::Ready {
                install: value
                    .get("install")
                    .and_then(Value::as_bool)
                    .ok_or(DeviceError::Integrity)?,
                outcome: parse_outcome(&value)?,
            },
            "final-success" => {
                let outcome = parse_outcome(&value)?;
                PendingPhase::Final {
                    ack: Ack::Installed(outcome.clone()),
                    report: Report::Outcome(outcome),
                }
            }
            "final-outcome-failure" => {
                let outcome = parse_outcome(&value)?;
                PendingPhase::Final {
                    ack: Ack::Failed(required_string(&value, "failure")?),
                    report: Report::Outcome(outcome),
                }
            }
            "final-error" => {
                let error = parse_device_error(&required_string(&value, "error")?)
                    .ok_or(DeviceError::Integrity)?;
                PendingPhase::Final {
                    ack: Ack::Failed(required_string(&value, "failure")?),
                    report: Report::Error(error),
                }
            }
            _ => return Err(DeviceError::Integrity),
        };
        Ok(Some(Self {
            command_id,
            request_id,
            app_id,
            expires_at,
            phase,
        }))
    }

    fn save(&self, root: &Path) -> Result<(), DeviceError> {
        let mut body = ObjectBuilder::new()
            .set("version", 1_i32)
            .set("command_id", self.command_id.clone())
            .set("request_id", self.request_id.clone())
            .set("app_id", self.app_id.clone())
            .set("expires_at", self.expires_at.to_string());
        body = match &self.phase {
            PendingPhase::Ready { install, outcome } => body
                .set("phase", "ready")
                .set("install", *install)
                .set("outcome", outcome_name(outcome)),
            PendingPhase::Final {
                ack: Ack::Installed(outcome),
                report: Report::Outcome(_),
            } => body
                .set("phase", "final-success")
                .set("outcome", outcome_name(outcome)),
            PendingPhase::Final {
                ack: Ack::Failed(failure),
                report: Report::Outcome(outcome),
            } => body
                .set("phase", "final-outcome-failure")
                .set("failure", failure.clone())
                .set("outcome", outcome_name(outcome)),
            PendingPhase::Final {
                ack: Ack::Failed(failure),
                report: Report::Error(error),
            } => body
                .set("phase", "final-error")
                .set("failure", failure.clone())
                .set("error", device_error_name(*error)),
            PendingPhase::Final { .. } => return Err(DeviceError::Backend),
        };
        if let Some(minimum_cobalt_version) = match &self.phase {
            PendingPhase::Ready { outcome, .. }
            | PendingPhase::Final {
                report: Report::Outcome(outcome),
                ..
            } => match outcome {
                RemoteInstallOutcome::RequiresCobalt {
                    minimum_cobalt_version,
                    ..
                } => Some(minimum_cobalt_version),
                _ => None,
            },
            PendingPhase::Final { .. } => None,
        } {
            body = body.set("minimum_cobalt_version", minimum_cobalt_version.clone());
        }
        atomic_write(
            &state_root(root).join(PENDING_FILE),
            body.build().to_json().as_bytes(),
            0o600,
        )
    }

    fn final_success(mut self, outcome: RemoteInstallOutcome) -> Self {
        self.phase = PendingPhase::Final {
            ack: Ack::Installed(outcome.clone()),
            report: Report::Outcome(outcome),
        };
        self
    }

    fn final_outcome_failure(mut self, failure: &str, outcome: RemoteInstallOutcome) -> Self {
        self.phase = PendingPhase::Final {
            ack: Ack::Failed(failure.to_owned()),
            report: Report::Outcome(outcome),
        };
        self
    }

    fn final_failure(mut self, failure: &str, error: DeviceError) -> Self {
        self.phase = PendingPhase::Final {
            ack: Ack::Failed(failure.to_owned()),
            report: Report::Error(error),
        };
        self
    }
}

fn send_ack(
    relay: &mut impl Relay,
    credential: &Credential,
    command_id: &str,
    ack: Ack,
) -> Result<(), DeviceError> {
    let body = match ack {
        Ack::Installing => ObjectBuilder::new().set("state", "installing").build(),
        Ack::Installed(outcome) => ObjectBuilder::new()
            .set("state", "installed")
            .set(
                "outcome",
                relay_outcome(&outcome).ok_or(DeviceError::InvalidInput)?,
            )
            .build(),
        Ack::Failed(failure) => {
            if failure.is_empty() || failure.len() > 96 {
                return Err(DeviceError::InvalidInput);
            }
            ObjectBuilder::new()
                .set("state", "failed")
                .set("failure", failure)
                .build()
        }
    }
    .to_json();
    let path = format!(
        "/v1/devices/{}/commands/{command_id}/ack",
        credential.device_id
    );
    relay
        .send("POST", &path, Some(&credential.token), Some(&body))
        .map(|_| ())
}

fn outcome_name(outcome: &RemoteInstallOutcome) -> String {
    match outcome {
        RemoteInstallOutcome::None => "none",
        RemoteInstallOutcome::Installed { .. } => "installed",
        RemoteInstallOutcome::Updated { .. } => "updated",
        RemoteInstallOutcome::AlreadyInstalled { .. } => "already-installed",
        RemoteInstallOutcome::Included { .. } => "included",
        RemoteInstallOutcome::Unavailable { .. } => "unavailable",
        RemoteInstallOutcome::RequiresCobalt { .. } => "requires-cobalt",
    }
    .to_owned()
}

fn parse_outcome(value: &Value) -> Result<RemoteInstallOutcome, DeviceError> {
    let id = required_string(value, "app_id")?;
    if !kobo_protocol::valid_app_id(&id) {
        return Err(DeviceError::Integrity);
    }
    match required_string(value, "outcome")?.as_str() {
        "installed" => Ok(RemoteInstallOutcome::Installed { id }),
        "updated" => Ok(RemoteInstallOutcome::Updated { id }),
        "already-installed" => Ok(RemoteInstallOutcome::AlreadyInstalled { id }),
        "included" => Ok(RemoteInstallOutcome::Included { id }),
        "unavailable" => Ok(RemoteInstallOutcome::Unavailable { id }),
        "requires-cobalt" => {
            let minimum_cobalt_version = required_string(value, "minimum_cobalt_version")?;
            if minimum_cobalt_version.is_empty()
                || minimum_cobalt_version.len() > 32
                || !minimum_cobalt_version
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            {
                return Err(DeviceError::Integrity);
            }
            Ok(RemoteInstallOutcome::RequiresCobalt {
                id,
                minimum_cobalt_version,
            })
        }
        _ => Err(DeviceError::Integrity),
    }
}

fn relay_outcome(outcome: &RemoteInstallOutcome) -> Option<&'static str> {
    match outcome {
        RemoteInstallOutcome::Installed { .. } => Some("installed"),
        RemoteInstallOutcome::Updated { .. } => Some("updated"),
        RemoteInstallOutcome::AlreadyInstalled { .. } => Some("already-installed"),
        RemoteInstallOutcome::Included { .. } => Some("included"),
        RemoteInstallOutcome::None
        | RemoteInstallOutcome::Unavailable { .. }
        | RemoteInstallOutcome::RequiresCobalt { .. } => None,
    }
}

fn failure_code(error: DeviceError) -> &'static str {
    match error {
        DeviceError::NotFound => "not-found",
        DeviceError::Authentication => "authentication",
        DeviceError::TimedOut => "timed-out",
        DeviceError::Unreachable => "unreachable",
        DeviceError::InvalidInput => "invalid-input",
        DeviceError::Backend => "backend",
        DeviceError::Integrity => "integrity",
    }
}

fn device_error_name(error: DeviceError) -> &'static str {
    failure_code(error)
}

fn parse_device_error(value: &str) -> Option<DeviceError> {
    Some(match value {
        "not-found" => DeviceError::NotFound,
        "authentication" => DeviceError::Authentication,
        "timed-out" => DeviceError::TimedOut,
        "unreachable" => DeviceError::Unreachable,
        "invalid-input" => DeviceError::InvalidInput,
        "backend" => DeviceError::Backend,
        "integrity" => DeviceError::Integrity,
        _ => return None,
    })
}

fn completed(root: &Path) -> Result<Vec<String>, DeviceError> {
    let text = match fs::read_to_string(state_root(root).join(COMPLETED_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(DeviceError::Backend),
    };
    let mut ids = Vec::new();
    for line in text.lines() {
        // Uuids are accepted for journals written before entries became
        // browser-chosen request identifiers.
        if !valid_request_id(line) && !valid_uuid(line) {
            return Err(DeviceError::Integrity);
        }
        ids.push(line.to_owned());
    }
    Ok(ids)
}

fn remember_completed(root: &Path, request_id: &str) -> Result<(), DeviceError> {
    let mut ids = completed(root)?;
    ids.retain(|known| known != request_id);
    ids.push(request_id.to_owned());
    if ids.len() > COMPLETED_LIMIT {
        ids.drain(..ids.len() - COMPLETED_LIMIT);
    }
    let mut body = ids.join("\n");
    body.push('\n');
    atomic_write(
        &state_root(root).join(COMPLETED_FILE),
        body.as_bytes(),
        0o600,
    )
}

fn state_root(root: &Path) -> PathBuf {
    root.join("state").join(STATE_DIRECTORY)
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), DeviceError> {
    let parent = path.parent().ok_or(DeviceError::Backend)?;
    fs::create_dir_all(parent).map_err(|_| DeviceError::Backend)?;
    set_mode(parent, 0o700)?;
    let next = path.with_extension("next");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&next)
        .map_err(|_| DeviceError::Backend)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| DeviceError::Backend)?;
    fs::set_permissions(&next, fs::Permissions::from_mode(mode))
        .map_err(|_| DeviceError::Backend)?;
    fs::rename(&next, path).map_err(|_| DeviceError::Backend)?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DeviceError::Backend)
}

fn remove_state_file(root: &Path, name: &str) -> Result<(), DeviceError> {
    let path = state_root(root).join(name);
    match fs::remove_file(path) {
        Ok(()) => File::open(state_root(root))
            .and_then(|directory| directory.sync_all())
            .map_err(|_| DeviceError::Backend),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(DeviceError::Backend),
    }
}

fn set_mode(path: &Path, mode: u32) -> Result<(), DeviceError> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|_| DeviceError::Backend)
}

fn parse_json(bytes: &[u8]) -> Result<Value, DeviceError> {
    let text = std::str::from_utf8(bytes).map_err(|_| DeviceError::Integrity)?;
    kobo_json::parse(text).map_err(|_| DeviceError::Integrity)
}

fn required_string(value: &Value, key: &str) -> Result<String, DeviceError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(DeviceError::Integrity)
}

fn decode_bounded(value: &str, minimum: usize, maximum: usize) -> Result<Vec<u8>, DeviceError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| DeviceError::Integrity)?;
    if !(minimum..=maximum).contains(&decoded.len()) {
        return Err(DeviceError::Integrity);
    }
    Ok(decoded)
}

fn valid_token(value: &str) -> bool {
    valid_base64url(value, 43)
}

fn valid_base64url(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn valid_link_secret(value: &str) -> bool {
    valid_base64url(value, 22)
}

fn valid_request_id(value: &str) -> bool {
    valid_base64url(value, 22)
}

fn valid_browser_key(value: &str) -> bool {
    valid_base64url(value, 122)
}

fn valid_pair_proof(value: &str) -> bool {
    valid_base64url(value, 43)
}

fn valid_install_signature(value: &str) -> bool {
    valid_base64url(value, 86)
}

fn valid_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            14 => *byte == b'4',
            19 => matches!(*byte, b'8' | b'9' | b'a' | b'b'),
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        })
}

fn valid_pairing_code(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&byte))
}

fn valid_https_url(value: &str) -> bool {
    value.starts_with("https://")
        && value.len() <= kobo_protocol::MAX_URL_LEN
        && !value.chars().any(char::is_control)
}

fn parse_timestamp(value: &str) -> Option<u64> {
    let (date, time) = value.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.split('-');
    let year = date.next()?.parse::<i64>().ok()?;
    let month = date.next()?.parse::<u32>().ok()?;
    let day = date.next()?.parse::<u32>().ok()?;
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if date.next().is_some() || day == 0 || day > month_days {
        return None;
    }
    let time = time.split('.').next()?;
    let mut time = time.split(':');
    let hour = time.next()?.parse::<u64>().ok()?;
    let minute = time.next()?.parse::<u64>().ok()?;
    let second = time.next()?.parse::<u64>().ok()?;
    if time.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let shifted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    if days < 0 {
        return None;
    }
    u64::try_from(days)
        .ok()?
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdh::EphemeralSecret;
    use p256::elliptic_curve::rand_core::OsRng;
    use std::cell::Cell;
    use std::collections::VecDeque;

    fn root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("cobalt-app-link-{}-{name}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        root
    }

    #[derive(Default)]
    struct FakeRelay {
        responses: VecDeque<Result<Vec<u8>, DeviceError>>,
        requests: Vec<(String, String, Option<String>, Option<String>)>,
    }

    impl FakeRelay {
        fn response(mut self, body: &str) -> Self {
            self.responses.push_back(Ok(body.as_bytes().to_vec()));
            self
        }

        fn failure(mut self, error: DeviceError) -> Self {
            self.responses.push_back(Err(error));
            self
        }
    }

    impl Relay for FakeRelay {
        fn send(
            &mut self,
            method: &str,
            path: &str,
            token: Option<&str>,
            body: Option<&str>,
        ) -> Result<Vec<u8>, DeviceError> {
            self.requests.push((
                method.to_owned(),
                path.to_owned(),
                token.map(str::to_owned),
                body.map(str::to_owned),
            ));
            self.responses
                .pop_front()
                .unwrap_or(Err(DeviceError::Backend))
        }
    }

    fn credential() -> Credential {
        Credential {
            device_id: "12345678-1234-4123-8123-123456789abc".to_owned(),
            token: "A".repeat(43),
            pairing: None,
        }
    }

    fn pairing() -> Pairing {
        Pairing {
            code: "2345ABCD".to_owned(),
            url: "https://example.test/pair/?code=2345ABCD".to_owned(),
            expires_at: 2_000_000_000,
            secret: URL_SAFE_NO_PAD.encode([9_u8; 16]),
        }
    }

    fn request_id() -> String {
        URL_SAFE_NO_PAD.encode([0_u8; 16])
    }

    fn browser_signing_key() -> p256::ecdsa::SigningKey {
        p256::ecdsa::SigningKey::from_slice(&[3_u8; 32]).expect("browser key")
    }

    fn browser_public_key() -> String {
        URL_SAFE_NO_PAD.encode(
            browser_signing_key()
                .verifying_key()
                .to_public_key_der()
                .expect("browser public key")
                .as_bytes(),
        )
    }

    fn pin_browser(root: &Path) {
        save_browsers(
            root,
            &[PinnedBrowser {
                public_key: browser_public_key(),
                proven: true,
            }],
        )
        .expect("pin browser");
    }

    fn sign_command(device_id: &str, envelope: &Value) -> String {
        use p256::ecdsa::signature::Signer as _;
        let message = format!(
            "{INSTALL_SIGNATURE_CONTEXT}\n{device_id}\n{}\n{}\n{}",
            envelope
                .get("ephemeral_public_key")
                .and_then(Value::as_str)
                .expect("ephemeral key"),
            envelope
                .get("nonce")
                .and_then(Value::as_str)
                .expect("nonce"),
            envelope
                .get("ciphertext")
                .and_then(Value::as_str)
                .expect("ciphertext"),
        );
        let signature: p256::ecdsa::Signature = browser_signing_key().sign(message.as_bytes());
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    }

    fn pairing_response() -> &'static str {
        r#"{"pairing_code":"2345ABCD","pairing_url":"https://example.test/pair/?code=2345ABCD","expires_at":"2026-08-26T13:00:00.000Z"}"#
    }

    fn paired_response() -> &'static str {
        r#"{"paired":true,"browser_count":1,"pairing":null,"browser_keys":[]}"#
    }

    fn encrypted_envelope(
        identity: &Identity,
        device_id: &str,
        id: &str,
        requested_at: u64,
    ) -> Value {
        let ephemeral = EphemeralSecret::random(&mut OsRng);
        let public = PublicKey::from(&ephemeral);
        let shared = ephemeral.diffie_hellman(&identity.secret.public_key());
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]);
        let prk = salt.extract(shared.raw_secret_bytes().as_ref());
        let info = [HKDF_INFO];
        let okm = prk.expand(&info, AesKeyLength).expect("expand");
        let mut bytes = [0_u8; 32];
        okm.fill(&mut bytes).expect("key");
        let key = aead::UnboundKey::new(&aead::AES_256_GCM, &bytes)
            .map(aead::LessSafeKey::new)
            .expect("AES key");
        let nonce = [7_u8; 12];
        let mut plaintext = format!(
            r#"{{"version":2,"app_id":"{id}","request_id":"{}","requested_at":{requested_at}}}"#,
            request_id()
        )
        .into_bytes();
        key.seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(device_id.as_bytes()),
            &mut plaintext,
        )
        .expect("encrypt");
        ObjectBuilder::new()
            .set("algorithm", "ECDH-P256-AES-256-GCM")
            .set(
                "ephemeral_public_key",
                URL_SAFE_NO_PAD.encode(
                    public
                        .to_public_key_der()
                        .expect("ephemeral public key")
                        .as_bytes(),
                ),
            )
            .set("nonce", URL_SAFE_NO_PAD.encode(nonce))
            .set("ciphertext", URL_SAFE_NO_PAD.encode(plaintext))
            .build()
    }

    #[test]
    fn identity_is_persistent_private_and_has_an_uncompressed_public_key() {
        let root = root("identity");
        let first = Identity::load_or_create(&root).expect("first identity");
        let second = Identity::load_or_create(&root).expect("second identity");
        assert_eq!(first.secret.to_bytes(), second.secret.to_bytes());
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(first.public_key().expect("encoded public key"))
                .expect("public key")
                .len(),
            91
        );
        let mode = fs::metadata(state_root(&root).join(PRIVATE_KEY_FILE))
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn first_pairing_registers_once_and_persists_the_opaque_capability() {
        let root = root("register");
        let registration = format!(
            r#"{{"device_id":"{}","device_token":"{}","pairing_code":"2345ABCD","pairing_url":"https://example.test/pair/","expires_at":"2026-08-26T13:00:00.000Z"}}"#,
            credential().device_id,
            credential().token
        );
        let mut relay = FakeRelay::default().response(&registration);
        let result = begin_with(&root, &mut relay, 1_777_207_000, "Clara BW").expect("pair");
        assert!(matches!(
            result,
            DeviceResult::AppLink(AppLinkState::Pairing { .. })
        ));
        assert_eq!(relay.requests[0].0, "POST");
        assert_eq!(relay.requests[0].1, "/v1/pairings");
        assert!(relay.requests[0].2.is_none());
        let saved = Credential::load(&root)
            .expect("read credential")
            .expect("credential");
        assert_eq!(saved.token, "A".repeat(43));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn existing_registration_requests_a_new_pairing_without_rotating_identity() {
        let root = root("repair");
        credential().save(&root).expect("credential");
        let identity = Identity::load_or_create(&root).expect("identity");
        let key = identity.secret.to_bytes();
        let mut relay = FakeRelay::default().response(pairing_response());
        begin_with(&root, &mut relay, 1_777_207_000, "reader").expect("pair");
        assert_eq!(
            Identity::load_or_create(&root)
                .expect("same identity")
                .secret
                .to_bytes(),
            key
        );
        assert_eq!(
            relay.requests[0].1,
            format!("/v1/devices/{}/pairings", credential().device_id)
        );
        let token = "A".repeat(43);
        assert_eq!(relay.requests[0].2.as_deref(), Some(token.as_str()));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn command_decryption_uses_hkdf_and_device_id_as_aad() {
        let root = root("decrypt");
        let identity = Identity::load_or_create(&root).expect("identity");
        let device_id = credential().device_id;
        let envelope = encrypted_envelope(&identity, &device_id, "word-count", 1_787_748_392);
        assert_eq!(
            decrypt_command(&envelope, &identity.secret, &device_id).expect("decrypt"),
            InstallRequest {
                app_id: "word-count".to_owned(),
                request_id: request_id(),
                requested_at: 1_787_748_392,
            }
        );
        assert_eq!(
            decrypt_command(
                &envelope,
                &identity.secret,
                &credential().device_id.replace('1', "2")
            ),
            Err(DeviceError::Integrity)
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn decrypts_a_fixed_browser_webcrypto_envelope() {
        // Generated with WebCrypto importKey/deriveBits/deriveKey/encrypt using
        // fixed P-256 private scalars and a fixed nonce.
        let private = URL_SAFE_NO_PAD
            .decode("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE")
            .expect("device private key");
        let secret = SecretKey::from_slice(&private).expect("valid P-256 scalar");
        let public = URL_SAFE_NO_PAD.encode(
            secret
                .public_key()
                .to_public_key_der()
                .expect("device public key")
                .as_bytes(),
        );
        assert_eq!(
            public,
            "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEb_A7lJJBzh2t1DUZ5pYOCoW0GmmgXDKBA6orzhWUyhY8T3U6Vb8B3FP2wLDH7ueLQMb_fSWpbiKCuYnO9xwUSg"
        );
        let envelope = ObjectBuilder::new()
            .set("algorithm", "ECDH-P256-AES-256-GCM")
            .set(
                "ephemeral_public_key",
                "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEVQ9HEAPz35fD31Bqx5f2ch-xoft7j2-D0iRJimXIjiQTYJPXAS5QmnNxXL0LAKPMD_S1wBs_-hlqsfsycDa45g",
            )
            .set("nonce", "AAECAwQFBgcICQoL")
            .set(
                "ciphertext",
                "-h-bS-QE5N6219s4MC9U0Om5uE6i7wLT1unUn0jXykNwlauxLiOWLzQdMzWBsA9HCSZ7wVZyPOK7k0gkTxQjkviE2KpYGhsk7FNeIBEduCw-udeu8P8n2Xiw-IFO3UPRhA5Pny75yV3GRrJ1zzARRpT_Kw",
            )
            .build();
        assert_eq!(
            decrypt_command(&envelope, &secret, "12345678-1234-4123-8123-123456789abc"),
            Ok(InstallRequest {
                app_id: "word-count".to_owned(),
                request_id: "AAAAAAAAAAAAAAAAAAAAAA".to_owned(),
                requested_at: 1_787_748_000,
            })
        );
    }

    #[test]
    fn polling_decrypts_prepares_installs_and_acknowledges_in_order() {
        let root = root("poll");
        let credential = credential();
        credential.save(&root).expect("credential");
        pin_browser(&root);
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(
            &identity,
            &credential.device_id,
            "word-count",
            1_787_748_392,
        );
        let signature = sign_command(&credential.device_id, &envelope);
        let command = ObjectBuilder::new()
            .set("id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .set("envelope", envelope)
            .set("browser_public_key", browser_public_key())
            .set("signature", signature)
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(&response)
            .response("{}")
            .response("{}");
        let installs = Cell::new(0);
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, id| {
                assert_eq!(id, "word-count");
                Ok(crate::app_store::RemoteInstallPlan {
                    outcome: RemoteInstallOutcome::Installed { id: id.to_owned() },
                    install: true,
                })
            },
            |path, id| {
                assert_eq!(id, "word-count");
                assert!(
                    Pending::load(path)
                        .expect("pending state")
                        .expect("installing command")
                        .expires_at
                        >= 1_787_748_392 + INSTALL_COMPLETION_TTL_SECONDS
                );
                installs.set(installs.get() + 1);
                Ok(())
            },
        )
        .expect("poll");
        assert_eq!(installs.get(), 1);
        assert_eq!(
            result,
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Installed {
                id: "word-count".to_owned()
            })
        );
        assert_eq!(
            relay
                .requests
                .iter()
                .map(|request| (request.0.as_str(), request.1.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (
                    "GET",
                    "/v1/devices/12345678-1234-4123-8123-123456789abc/pairing"
                ),
                (
                    "GET",
                    "/v1/devices/12345678-1234-4123-8123-123456789abc/commands"
                ),
                (
                    "POST",
                    "/v1/devices/12345678-1234-4123-8123-123456789abc/commands/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/ack"
                ),
                (
                    "POST",
                    "/v1/devices/12345678-1234-4123-8123-123456789abc/commands/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/ack"
                ),
            ]
        );
        assert!(relay.requests[2]
            .3
            .as_deref()
            .is_some_and(|body| { body.contains(r#""state":"installing""#) }));
        assert!(relay.requests[3].3.as_deref().is_some_and(|body| {
            body.contains(r#""state":"installed""#) && body.contains(r#""outcome":"installed""#)
        }));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn an_expired_command_is_failed_without_decryption_or_installation() {
        let root = root("expired");
        credential().save(&root).expect("credential");
        pin_browser(&root);
        let response = r#"{"command":{"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","envelope":{},"created_at":"2026-08-25T12:46:31.000Z","expires_at":"2026-08-26T12:46:31.000Z"}}"#;
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(response)
            .response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, _| panic!("expired command must not prepare"),
            |_, _| panic!("expired command must not install"),
        );
        assert_eq!(result, Err(DeviceError::InvalidInput));
        assert!(relay.requests[2].3.as_deref().is_some_and(|body| {
            body.contains(r#""state":"failed""#) && body.contains(r#""failure":"expired""#)
        }));
        assert!(Pending::load(&root).expect("pending").is_none());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn final_acknowledgements_survive_restart_and_do_not_repeat_installation() {
        let root = root("pending");
        let credential = credential();
        credential.save(&root).expect("credential");
        let pending = Pending {
            command_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
            request_id: request_id(),
            app_id: "word-count".to_owned(),
            expires_at: 2_000_000_000,
            phase: PendingPhase::Final {
                ack: Ack::Installed(RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned(),
                }),
                report: Report::Outcome(RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned(),
                }),
            },
        };
        pending.save(&root).expect("pending");
        let mut relay = FakeRelay::default().response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_800_000_000,
            |_, _| panic!("a final acknowledgement must not prepare again"),
            |_, _| panic!("a final acknowledgement must not install again"),
        )
        .expect("retry");
        assert!(matches!(
            result,
            DeviceResult::RemoteInstall(RemoteInstallOutcome::Installed { .. })
        ));
        assert!(Pending::load(&root).expect("pending state").is_none());
        assert_eq!(completed(&root).expect("completed"), vec![request_id()]);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn cobalt_requirement_survives_a_pending_install_restart() {
        let root = root("requires-cobalt-pending");
        let pending = Pending {
            command_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
            request_id: request_id(),
            app_id: "word-count".to_owned(),
            expires_at: 2_000_000_000,
            phase: PendingPhase::Ready {
                install: false,
                outcome: RemoteInstallOutcome::RequiresCobalt {
                    id: "word-count".to_owned(),
                    minimum_cobalt_version: "0.4.0".to_owned(),
                },
            },
        };
        pending.save(&root).expect("save pending requirement");
        assert_eq!(
            Pending::load(&root).expect("load pending requirement"),
            Some(pending)
        );
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn disconnect_revokes_the_device_and_removes_every_local_capability() {
        let root = root("disconnect");
        Identity::load_or_create(&root).expect("identity");
        credential().save(&root).expect("credential");
        fs::write(state_root(&root).join(COMPLETED_FILE), "").expect("journal");
        let mut relay = FakeRelay::default().response("{}");
        let result = disconnect_with(&root, &mut relay).expect("disconnect");
        assert_eq!(result, DeviceResult::AppLink(AppLinkState::Unpaired));
        assert_eq!(relay.requests[0].0, "DELETE");
        assert!(Credential::load(&root).expect("credential state").is_none());
        assert!(!state_root(&root).join(COMPLETED_FILE).exists());
        assert!(!state_root(&root).join(PRIVATE_KEY_FILE).exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn disconnect_revokes_the_local_identity_while_offline() {
        let root = root("disconnect-offline");
        Identity::load_or_create(&root).expect("identity");
        credential().save(&root).expect("credential");
        let mut relay = FakeRelay::default().failure(DeviceError::Unreachable);
        let result = disconnect_with(&root, &mut relay).expect("local disconnect");
        assert_eq!(result, DeviceResult::AppLink(AppLinkState::Unpaired));
        assert!(Credential::load(&root).expect("credential state").is_none());
        assert!(!state_root(&root).join(PRIVATE_KEY_FILE).exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_corrupt_credential_still_unpairs_and_clears_local_state() {
        let root = root("disconnect-corrupt");
        Identity::load_or_create(&root).expect("identity");
        atomic_write(
            &state_root(&root).join(CREDENTIAL_FILE),
            b"{not json",
            0o600,
        )
        .expect("corrupt credential");
        let mut relay = FakeRelay::default();
        let result = disconnect_with(&root, &mut relay).expect("unpair");
        assert_eq!(result, DeviceResult::AppLink(AppLinkState::Unpaired));
        assert!(relay.requests.is_empty());
        assert!(!state_root(&root).join(CREDENTIAL_FILE).exists());
        assert!(!state_root(&root).join(PRIVATE_KEY_FILE).exists());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_relay_outage_during_the_final_acknowledgement_keeps_the_pending_record() {
        let root = root("ack-outage");
        credential().save(&root).expect("credential");
        let pending = Pending {
            command_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
            request_id: request_id(),
            app_id: "word-count".to_owned(),
            expires_at: 2_000_000_000,
            phase: PendingPhase::Final {
                ack: Ack::Installed(RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned(),
                }),
                report: Report::Outcome(RemoteInstallOutcome::Installed {
                    id: "word-count".to_owned(),
                }),
            },
        };
        pending.save(&root).expect("pending");
        let mut relay = FakeRelay::default().failure(DeviceError::Unreachable);
        let result = poll_with(
            &root,
            &mut relay,
            1_800_000_000,
            |_, _| panic!("a final acknowledgement must not prepare"),
            |_, _| panic!("a final acknowledgement must not install"),
        );
        assert_eq!(result, Err(DeviceError::Unreachable));
        assert!(Pending::load(&root).expect("pending state").is_some());
        assert!(completed(&root).expect("completed").is_empty());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn relay_status_codes_distinguish_outages_from_final_verdicts() {
        assert!(transient_status(
            b"HTTP/1.1 500 Internal Server Error\r\n\r\n"
        ));
        assert!(transient_status(
            b"HTTP/1.1 503 Service Unavailable\r\n\r\n"
        ));
        assert!(transient_status(b"HTTP/1.1 429 Too Many Requests\r\n\r\n"));
        assert!(!transient_status(b"HTTP/1.1 404 Not Found\r\n\r\n"));
        assert!(!transient_status(b"HTTP/1.1 409 Conflict\r\n\r\n"));
        assert!(!transient_status(b"garbage"));
    }

    fn pair_proof(secret: &str, key: &str) -> String {
        let secret = URL_SAFE_NO_PAD.decode(secret).expect("secret");
        let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, &secret);
        let mut message = Vec::new();
        message.extend_from_slice(PAIR_PROOF_CONTEXT.as_bytes());
        message.push(b'\n');
        message.extend_from_slice(key.as_bytes());
        URL_SAFE_NO_PAD.encode(hmac::sign(&hmac_key, &message).as_ref())
    }

    fn second_browser_key() -> String {
        URL_SAFE_NO_PAD.encode(
            p256::ecdsa::SigningKey::from_slice(&[4_u8; 32])
                .expect("second browser key")
                .verifying_key()
                .to_public_key_der()
                .expect("second public key")
                .as_bytes(),
        )
    }

    #[test]
    fn browser_keys_pin_only_while_a_device_created_pairing_is_outstanding() {
        let root = root("pins");
        let pairing = pairing();
        let key = browser_public_key();
        let proven = ObjectBuilder::new()
            .set(
                "browser_keys",
                vec![
                    ObjectBuilder::new()
                        .set("public_key", second_browser_key())
                        .build(),
                    ObjectBuilder::new()
                        .set("public_key", key.clone())
                        .set("proof", pair_proof(&pairing.secret, &key))
                        .build(),
                ],
            )
            .build();
        assert_eq!(
            sync_browser_pins(&root, Some(&pairing), &proven).expect("pin proven"),
            1
        );
        assert_eq!(
            load_browsers(&root).expect("browsers"),
            vec![PinnedBrowser {
                public_key: key.clone(),
                proven: true,
            }]
        );
        let unproven = ObjectBuilder::new()
            .set(
                "browser_keys",
                vec![ObjectBuilder::new()
                    .set("public_key", second_browser_key())
                    .build()],
            )
            .build();
        sync_browser_pins(&root, None, &unproven).expect("ignore without pairing");
        assert_eq!(load_browsers(&root).expect("browsers").len(), 1);
        assert_eq!(
            sync_browser_pins(&root, Some(&pairing), &unproven).expect("reject proofless pairing"),
            0
        );
        assert_eq!(load_browsers(&root).expect("browsers").len(), 1);
        let forged = ObjectBuilder::new()
            .set(
                "browser_keys",
                vec![ObjectBuilder::new()
                    .set("public_key", "B".repeat(122))
                    .set(
                        "proof",
                        pair_proof(&URL_SAFE_NO_PAD.encode([1_u8; 16]), "B"),
                    )
                    .build()],
            )
            .build();
        sync_browser_pins(&root, Some(&pairing), &forged).expect("ignore forged proof");
        assert_eq!(load_browsers(&root).expect("browsers").len(), 1);
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn pairing_remains_open_when_no_browser_key_can_be_pinned() {
        let root = root("empty-pins");
        let credential = Credential {
            pairing: Some(pairing()),
            ..credential()
        };
        credential.save(&root).expect("credential");
        let mut relay = FakeRelay::default().response(paired_response());

        assert_eq!(
            read_with(&root, &mut relay, 1_787_748_392),
            Err(DeviceError::Integrity)
        );
        assert!(Credential::load(&root)
            .expect("credential state")
            .expect("credential")
            .pairing
            .is_some());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn expired_pairing_cannot_pin_a_browser_key() {
        let root = root("expired-pairing");
        let pairing = pairing();
        let key = browser_public_key();
        Credential {
            pairing: Some(pairing.clone()),
            ..credential()
        }
        .save(&root)
        .expect("credential");
        let response = ObjectBuilder::new()
            .set("paired", true)
            .set("browser_count", 1)
            .set("pairing", Value::Null)
            .set(
                "browser_keys",
                vec![ObjectBuilder::new()
                    .set("public_key", key.clone())
                    .set("proof", pair_proof(&pairing.secret, &key))
                    .build()],
            )
            .build()
            .to_json();
        let mut relay = FakeRelay::default().response(&response);

        assert_eq!(
            read_with(&root, &mut relay, pairing.expires_at),
            Err(DeviceError::Integrity)
        );
        assert!(Credential::load(&root)
            .expect("credential state")
            .expect("credential")
            .pairing
            .is_none());
        assert!(load_browsers(&root).expect("browsers").is_empty());
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn commands_from_unknown_senders_are_rejected_without_decryption() {
        let root = root("unknown-sender");
        let credential = credential();
        credential.save(&root).expect("credential");
        save_browsers(
            &root,
            &[PinnedBrowser {
                public_key: second_browser_key(),
                proven: true,
            }],
        )
        .expect("pin other browser");
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(
            &identity,
            &credential.device_id,
            "word-count",
            1_787_748_392,
        );
        let signature = sign_command(&credential.device_id, &envelope);
        let command = ObjectBuilder::new()
            .set("id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .set("envelope", envelope)
            .set("browser_public_key", browser_public_key())
            .set("signature", signature)
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(&response)
            .response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, _| panic!("an unknown sender must not prepare"),
            |_, _| panic!("an unknown sender must not install"),
        );
        assert_eq!(result, Err(DeviceError::Integrity));
        assert!(relay.requests[2].3.as_deref().is_some_and(|body| {
            body.contains(r#""state":"failed""#) && body.contains(r#""failure":"unknown-sender""#)
        }));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_tampered_signature_is_rejected_before_decryption() {
        let root = root("bad-signature");
        let credential = credential();
        credential.save(&root).expect("credential");
        pin_browser(&root);
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(
            &identity,
            &credential.device_id,
            "word-count",
            1_787_748_392,
        );
        let command = ObjectBuilder::new()
            .set("id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .set("envelope", envelope)
            .set("browser_public_key", browser_public_key())
            .set("signature", "C".repeat(86))
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(&response)
            .response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, _| panic!("a forged command must not prepare"),
            |_, _| panic!("a forged command must not install"),
        );
        assert_eq!(result, Err(DeviceError::Integrity));
        assert!(relay.requests[2]
            .3
            .as_deref()
            .is_some_and(|body| { body.contains(r#""failure":"invalid-signature""#) }));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_replayed_request_is_rejected_even_under_a_fresh_command_identity() {
        let root = root("replay");
        let credential = credential();
        credential.save(&root).expect("credential");
        pin_browser(&root);
        remember_completed(&root, &request_id()).expect("journal");
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(
            &identity,
            &credential.device_id,
            "word-count",
            1_787_748_392,
        );
        let signature = sign_command(&credential.device_id, &envelope);
        let command = ObjectBuilder::new()
            .set("id", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
            .set("envelope", envelope)
            .set("browser_public_key", browser_public_key())
            .set("signature", signature)
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(&response)
            .response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, _| panic!("a replayed request must not prepare"),
            |_, _| panic!("a replayed request must not install"),
        );
        assert_eq!(result, Err(DeviceError::Integrity));
        assert!(relay.requests[2]
            .3
            .as_deref()
            .is_some_and(|body| { body.contains(r#""failure":"replayed-command""#) }));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn a_stale_sealed_request_is_rejected_as_expired() {
        let root = root("stale-request");
        let credential = credential();
        credential.save(&root).expect("credential");
        pin_browser(&root);
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(
            &identity,
            &credential.device_id,
            "word-count",
            1_787_748_392 - COMMAND_TTL_SECONDS - 1,
        );
        let signature = sign_command(&credential.device_id, &envelope);
        let command = ObjectBuilder::new()
            .set("id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .set("envelope", envelope)
            .set("browser_public_key", browser_public_key())
            .set("signature", signature)
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(paired_response())
            .response(&response)
            .response("{}");
        let result = poll_with(
            &root,
            &mut relay,
            1_787_748_392,
            |_, _| panic!("a stale request must not prepare"),
            |_, _| panic!("a stale request must not install"),
        );
        assert_eq!(result, Err(DeviceError::InvalidInput));
        assert!(relay.requests[2]
            .3
            .as_deref()
            .is_some_and(|body| body.contains(r#""failure":"expired""#)));
        let _ignored = fs::remove_dir_all(root);
    }

    #[test]
    fn timestamps_are_strict_and_expiry_is_bounded_to_seventy_two_hours() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_timestamp("2026-08-26T12:46:31.167Z"),
            Some(1_787_748_391)
        );
        assert_eq!(parse_timestamp("2026-08-26 12:46:31Z"), None);
        assert_eq!(parse_timestamp("2026-13-26T12:46:31Z"), None);
        assert_eq!(parse_timestamp("2026-02-29T12:46:31Z"), None);
        assert_eq!(COMMAND_TTL_SECONDS, 259_200);
    }

    #[test]
    fn relay_urls_are_https_and_single_line() {
        assert!(valid_https_url(
            "https://bandarlabs.github.io/Cobalt/pair/?code=2345ABCD"
        ));
        assert!(!valid_https_url("http://example.test/pair"));
        assert!(!valid_https_url("https://example.test/pair\nforged"));
        assert!(!valid_https_url("https://example.test/\u{7f}forged"));
    }
}
