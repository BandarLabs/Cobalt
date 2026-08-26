use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use kobo_json::{ObjectBuilder, Value};
use kobo_protocol::{AppLinkState, DeviceError, DeviceResult, RemoteInstallOutcome};
use p256::ecdh::diffie_hellman;
use p256::pkcs8::{DecodePublicKey, EncodePublicKey};
use p256::{PublicKey, SecretKey};
use ring::{aead, hkdf};
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
const MAX_RELAY_BODY: u32 = 16 * 1024;
const MAX_HTTP_RESPONSE: u64 = 48 * 1024;
const COMMAND_TTL_SECONDS: u64 = 24 * 60 * 60;
const INSTALL_COMPLETION_TTL_SECONDS: u64 = 15 * 60;
const CLOCK_SKEW_SECONDS: u64 = 5 * 60;
const COMPLETED_LIMIT: usize = 64;
const HKDF_INFO: &[u8] = b"cobalt-app-install-v1";
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
    poll_with(
        root,
        &mut relay,
        now(),
        crate::app_store::prepare_remote_install,
        crate::app_store::install,
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
        match kobo_net::split_response(&response, MAX_RELAY_BODY).map_err(network_error)? {
            kobo_net::Response::Body(body) => Ok(body.into_owned()),
            kobo_net::Response::Redirect(_) => Err(DeviceError::Unreachable),
        }
    }
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
                    File::open("/dev/urandom")
                        .and_then(|mut random| random.read_exact(&mut bytes))
                        .map_err(|_| DeviceError::Backend)?;
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
                let expires_at = value
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .and_then(|value| value.parse::<u64>().ok());
                if !code.is_some_and(valid_pairing_code)
                    || !url.is_some_and(valid_https_url)
                    || expires_at.is_none()
                {
                    return Err(DeviceError::Integrity);
                }
                Some(Pairing {
                    code: code.unwrap_or_default().to_owned(),
                    url: url.unwrap_or_default().to_owned(),
                    expires_at: expires_at.unwrap_or_default(),
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
        credential.pairing = Some(parse_pairing(&response)?);
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
            pairing: Some(pairing),
        };
        credential.save(root)?;
        credential
    };
    let state = local_pairing_state(&credential, now).unwrap_or(AppLinkState::Unpaired);
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
        if credential.pairing.take().is_some() {
            credential.save(root)?;
        }
        return Ok(DeviceResult::AppLink(AppLinkState::Paired { browsers }));
    }
    let state = local_pairing_state(&credential, now).unwrap_or(AppLinkState::Unpaired);
    if matches!(state, AppLinkState::Unpaired) && credential.pairing.take().is_some() {
        credential.save(root)?;
    }
    Ok(DeviceResult::AppLink(state))
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
    if completed(root)?.iter().any(|known| known == &command_id) {
        return reject_command(
            root,
            relay,
            credential,
            &command_id,
            "replayed-command",
            DeviceError::Integrity,
        );
    }
    let timestamps = required_string(command, "created_at")
        .and_then(|value| parse_timestamp(&value).ok_or(DeviceError::Integrity))
        .and_then(|created_at| {
            required_string(command, "expires_at")
                .and_then(|value| parse_timestamp(&value).ok_or(DeviceError::Integrity))
                .map(|expires_at| (created_at, expires_at))
        });
    let (created_at, expires_at) = match timestamps {
        Ok(timestamps) => timestamps,
        Err(error) => {
            return reject_command(
                root,
                relay,
                credential,
                &command_id,
                "invalid-command",
                error,
            );
        }
    };
    if expires_at <= now
        || expires_at.saturating_sub(created_at) > COMMAND_TTL_SECONDS
        || created_at > now.saturating_add(CLOCK_SKEW_SECONDS)
    {
        return reject_command(
            root,
            relay,
            credential,
            &command_id,
            "expired",
            DeviceError::InvalidInput,
        );
    }
    let Some(envelope) = command.get("envelope") else {
        return reject_command(
            root,
            relay,
            credential,
            &command_id,
            "invalid-command",
            DeviceError::Integrity,
        );
    };
    let app_id = match decrypt_command(
        envelope,
        &Identity::load_or_create(root)?.secret,
        &credential.device_id,
    ) {
        Ok(id) => id,
        Err(error) => {
            return reject_command(
                root,
                relay,
                credential,
                &command_id,
                "invalid-envelope",
                error,
            );
        }
    };
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
            remember_completed(root, &pending.command_id)?;
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
    let remote = if let Some(credential) = Credential::load(root)? {
        let path = format!("/v1/devices/{}", credential.device_id);
        relay.send("DELETE", &path, Some(&credential.token), None)
    } else {
        Ok(Vec::new())
    };
    remove_state_file(root, CREDENTIAL_FILE)?;
    remove_state_file(root, PENDING_FILE)?;
    remove_state_file(root, COMPLETED_FILE)?;
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
    })
}

fn local_pairing_state(credential: &Credential, now: u64) -> Option<AppLinkState> {
    let pairing = credential.pairing.as_ref()?;
    let remaining = pairing.expires_at.checked_sub(now)?;
    Some(AppLinkState::Pairing {
        code: pairing.code.clone(),
        url: pairing.url.clone(),
        expires_in: u32::try_from(remaining.min(10 * 60)).unwrap_or(10 * 60),
    })
}

fn decrypt_command(
    envelope: &Value,
    secret: &SecretKey,
    device_id: &str,
) -> Result<String, DeviceError> {
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
    if fields.len() != 2
        || value.get("version").and_then(Value::as_i64) != Some(1)
        || value.get("app_id").and_then(Value::as_str).is_none()
    {
        return Err(DeviceError::InvalidInput);
    }
    let id = value
        .get("app_id")
        .and_then(Value::as_str)
        .ok_or(DeviceError::InvalidInput)?;
    if !kobo_protocol::valid_app_id(id) {
        return Err(DeviceError::InvalidInput);
    }
    Ok(id.to_owned())
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
        let app_id = required_string(&value, "app_id")?;
        let expires_at = required_string(&value, "expires_at")?
            .parse::<u64>()
            .map_err(|_| DeviceError::Integrity)?;
        if !valid_uuid(&command_id) || (!app_id.is_empty() && !kobo_protocol::valid_app_id(&app_id))
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
            app_id,
            expires_at,
            phase,
        }))
    }

    fn save(&self, root: &Path) -> Result<(), DeviceError> {
        let mut body = ObjectBuilder::new()
            .set("version", 1_i32)
            .set("command_id", self.command_id.clone())
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
        _ => Err(DeviceError::Integrity),
    }
}

fn relay_outcome(outcome: &RemoteInstallOutcome) -> Option<&'static str> {
    match outcome {
        RemoteInstallOutcome::Installed { .. } => Some("installed"),
        RemoteInstallOutcome::Updated { .. } => Some("updated"),
        RemoteInstallOutcome::AlreadyInstalled { .. } => Some("already-installed"),
        RemoteInstallOutcome::Included { .. } => Some("included"),
        RemoteInstallOutcome::None | RemoteInstallOutcome::Unavailable { .. } => None,
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
        if !valid_uuid(line) {
            return Err(DeviceError::Integrity);
        }
        ids.push(line.to_owned());
    }
    Ok(ids)
}

fn remember_completed(root: &Path, command_id: &str) -> Result<(), DeviceError> {
    let mut ids = completed(root)?;
    ids.retain(|known| known != command_id);
    ids.push(command_id.to_owned());
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
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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

    fn pairing_response() -> &'static str {
        r#"{"pairing_code":"2345ABCD","pairing_url":"https://example.test/pair/?code=2345ABCD","expires_at":"2026-08-26T13:00:00.000Z"}"#
    }

    fn encrypted_envelope(identity: &Identity, device_id: &str, id: &str) -> Value {
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
        let mut plaintext = ObjectBuilder::new()
            .set("version", 1_i32)
            .set("app_id", id)
            .build()
            .to_json()
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
        let envelope = encrypted_envelope(&identity, &device_id, "word-count");
        assert_eq!(
            decrypt_command(&envelope, &identity.secret, &device_id).expect("decrypt"),
            "word-count"
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
                "-h-bS-QE5N6219s7MC9U0Om5uE6i7wLT1unUn0jXykNwlfr69mWa5Y6zED2tc4kWaq9u",
            )
            .build();
        assert_eq!(
            decrypt_command(&envelope, &secret, "12345678-1234-4123-8123-123456789abc"),
            Ok("word-count".to_owned())
        );
    }

    #[test]
    fn polling_decrypts_prepares_installs_and_acknowledges_in_order() {
        let root = root("poll");
        let credential = credential();
        credential.save(&root).expect("credential");
        let identity = Identity::load_or_create(&root).expect("identity");
        let envelope = encrypted_envelope(&identity, &credential.device_id, "word-count");
        let command = ObjectBuilder::new()
            .set("id", "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
            .set("envelope", envelope)
            .set("created_at", "2026-08-26T12:46:31.000Z")
            .set("expires_at", "2026-08-26T12:47:00.000Z")
            .build();
        let response = ObjectBuilder::new()
            .set("command", command)
            .build()
            .to_json();
        let mut relay = FakeRelay::default()
            .response(r#"{"paired":true,"browser_count":1,"pairing":null}"#)
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
        let response = r#"{"command":{"id":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","envelope":{},"created_at":"2026-08-25T12:46:31.000Z","expires_at":"2026-08-26T12:46:31.000Z"}}"#;
        let mut relay = FakeRelay::default()
            .response(r#"{"paired":true,"browser_count":1,"pairing":null}"#)
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
        assert_eq!(
            completed(&root).expect("completed"),
            vec!["aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"]
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
    fn timestamps_are_strict_and_expiry_is_bounded_to_twenty_four_hours() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_timestamp("2026-08-26T12:46:31.167Z"),
            Some(1_787_748_391)
        );
        assert_eq!(parse_timestamp("2026-08-26 12:46:31Z"), None);
        assert_eq!(parse_timestamp("2026-13-26T12:46:31Z"), None);
        assert_eq!(parse_timestamp("2026-02-29T12:46:31Z"), None);
        assert_eq!(COMMAND_TTL_SECONDS, 86_400);
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
