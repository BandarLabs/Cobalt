//! Owner-attended Post sign-in: check a Hermes gateway's bearer token from
//! the computer, pin it to that one gateway, and deliver both to the reader.
//! The application only ever names the credential; the token itself stays in
//! the runtime's store, exactly as if it had been typed onto the device.

use std::fs;
use std::time::Duration;

const CREDENTIAL: &str = "hermes-post";
const DEVICE_STATE: &str = "/mnt/onboard/.adds/cobalt/state/post";
const GATEWAY_KEY: &str = "gateway";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);
const USAGE: &str =
    "usage: kobo post login --gateway URL --token-file PATH (--sim | --device IP)\n\
                     \n\
                     Signs Post in to a Hermes gateway. The token file holds the\n\
                     gateway's bearer token; it is checked against the gateway\n\
                     (GET /letters) before anything is installed, pinned to that\n\
                     one gateway, and delivered to the reader's credential store,\n\
                     where only the runtime reads it. The gateway address is\n\
                     saved into Post's state so the app opens the inbox directly.";

enum Target {
    Device(String),
    Sim,
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments.first().map(String::as_str) {
        Some("login") => login(&arguments[1..]),
        _ => Err(USAGE.to_owned()),
    }
}

struct Options {
    gateway: String,
    token: String,
    target: Target,
}

fn parse_options(arguments: &[String]) -> Result<Options, String> {
    let mut gateway = None;
    let mut token_file = None;
    let mut target = None;
    let mut rest = arguments;
    while let Some((flag, tail)) = rest.split_first() {
        match flag.as_str() {
            "--sim" => {
                target = Some(Target::Sim);
                rest = tail;
            }
            "--device" => {
                let (ip, tail) = tail.split_first().ok_or_else(|| USAGE.to_owned())?;
                target = Some(Target::Device(ip.clone()));
                rest = tail;
            }
            "--gateway" | "--token-file" => {
                let (value, tail) = tail.split_first().ok_or_else(|| USAGE.to_owned())?;
                match flag.as_str() {
                    "--gateway" => gateway = Some(value.clone()),
                    _ => token_file = Some(value.clone()),
                }
                rest = tail;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    let gateway = gateway.ok_or_else(|| USAGE.to_owned())?;
    if !gateway.starts_with("https://") {
        return Err("the gateway must be an https:// address".to_owned());
    }
    let token_file = token_file.ok_or_else(|| USAGE.to_owned())?;
    let metadata =
        fs::symlink_metadata(&token_file).map_err(|error| format!("read {token_file}: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() > 4096 {
        return Err("the token file must be a regular file no larger than 4 KiB".into());
    }
    let token = fs::read_to_string(&token_file)
        .map_err(|error| format!("read {token_file}: {error}"))?
        .trim()
        .to_owned();
    if token.is_empty() {
        return Err(format!("{token_file} holds no token"));
    }
    Ok(Options {
        gateway: gateway.trim_end_matches('/').to_owned(),
        token,
        target: target.ok_or_else(|| USAGE.to_owned())?,
    })
}

pub(super) fn login_error(service: &str, server: &str, error: kobo_protocol::TaskError) -> String {
    let guidance = match error {
        kobo_protocol::TaskError::Unauthorized => "The token was rejected. Create or copy a current token and retry.",
        kobo_protocol::TaskError::Unreachable => "The server or its TLS certificate could not be verified. Check the HTTPS address; for a private CA, install its root with 'kobo trust set NAME --from ROOT.pem --device ADDRESS'.",
        kobo_protocol::TaskError::Offline => "This computer has no network route. Reconnect it before checking the service.",
        kobo_protocol::TaskError::TimedOut => "The server did not answer in time. Nothing was installed; retry when it is reachable.",
        _ => "The service check failed. Nothing was installed.",
    };
    format!("{service} at {server} could not be connected: {guidance}")
}

/// The credential's file format, shared with the runtime: a version line, the
/// one server the token may ever be sent to, then the token itself.
fn credential_value(gateway: &str, token: &str) -> String {
    format!("cobalt-server-account-v1\n{gateway}\n{token}")
}

fn login(arguments: &[String]) -> Result<(), String> {
    let Options {
        gateway,
        token,
        target,
    } = parse_options(arguments)?;

    // The simulator's own trust override, so a developer's local fixture
    // gateway can be logged in to exactly the way the application will use it.
    if let Some(directory) = std::env::var_os("KOBO_SIM_TRUST_DIR") {
        let _ = kobo_net::trust_owner_roots_from_dir(std::path::Path::new(&directory));
    }

    // A token that does not open the inbox must never reach the reader: the
    // application would install it and then fail on the owner's hands.
    let authorization = format!("Bearer {token}");
    kobo_net::fetch_from(
        &format!("{gateway}/letters?page=1&per_page=1"),
        0,
        16 * 1024,
        Some(("Authorization", &authorization)),
        &[],
    )
    .map_err(|error| login_error("Hermes", &gateway, error))?;

    let credential = credential_value(&gateway, &token);
    match target {
        Target::Sim => {
            let secrets = std::env::temp_dir()
                .join("cobalt-sim-secrets")
                .join("apps")
                .join("post")
                .join("servers");
            fs::create_dir_all(&secrets).map_err(|error| format!("create {secrets:?}: {error}"))?;
            let credential_path = secrets.join(CREDENTIAL);
            fs::write(&credential_path, &credential)
                .map_err(|error| format!("write {credential_path:?}: {error}"))?;
            let state = std::env::temp_dir().join("cobalt-sim-state").join("post");
            fs::create_dir_all(&state).map_err(|error| format!("create {state:?}: {error}"))?;
            let gateway_path = state.join(GATEWAY_KEY);
            fs::write(&gateway_path, &gateway)
                .map_err(|error| format!("write {gateway_path:?}: {error}"))?;
            println!(
                "Signed Post in to {gateway}; the simulator picks up the credential on its next start."
            );
        }
        Target::Device(host) => {
            let install = super::secret_install_script(CREDENTIAL, &credential);
            let encoded = super::base64_encode(gateway.as_bytes());
            let script = format!(
                "{install}\
                 set -eu\n\
                 root='{DEVICE_STATE}'\n\
                 mkdir -p \"$root\"\n\
                 chmod 700 \"$root\"\n\
                 partial=\"$root/.{GATEWAY_KEY}.writing\"\n\
                 base64 -d > \"$partial\" <<'KOBO_POST_GATEWAY'\n\
                 {encoded}\n\
                 KOBO_POST_GATEWAY\n\
                 chmod 600 \"$partial\"\n\
                 mv -f \"$partial\" \"$root/{GATEWAY_KEY}\"\n\
                 sync\n"
            );
            remote(&host, &script)?;
            println!("Signed Post in to {gateway}; Post on {host} picks it up on its next start.");
        }
    }
    Ok(())
}

fn remote(host: &str, script: &str) -> Result<(), String> {
    let output = super::run_remote_shell(&format!("root@{host}"), script, TRANSFER_TIMEOUT)
        .map_err(super::unreachable_device)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(super::unreachable_if_ssh_gave_up(
            super::remote_shell_error(
                format!(
                    "Post sign-in delivery to {host} exited with {}",
                    output.status
                ),
                &output.stdout,
                &output.stderr,
            ),
            &output,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_errors_distinguish_token_network_and_private_trust() {
        let rejected = login_error(
            "Hermes",
            "https://letters.example",
            kobo_protocol::TaskError::Unauthorized,
        );
        assert!(rejected.contains("token was rejected"));
        let tls = login_error(
            "Hermes",
            "https://letters.example",
            kobo_protocol::TaskError::Unreachable,
        );
        assert!(tls.contains("TLS certificate"));
        assert!(tls.contains("kobo trust set"));
        assert!(login_error(
            "Hermes",
            "https://letters.example",
            kobo_protocol::TaskError::Offline
        )
        .contains("no network route"));
    }

    #[test]
    fn credential_value_pins_the_gateway() {
        assert_eq!(
            credential_value("https://letters.example", "token-1"),
            "cobalt-server-account-v1\nhttps://letters.example\ntoken-1"
        );
    }

    #[test]
    fn options_require_https_and_a_token() {
        let arguments = |gateway: &str| {
            vec![
                "login".to_owned(),
                "--gateway".to_owned(),
                gateway.to_owned(),
                "--token-file".to_owned(),
                "/nonexistent".to_owned(),
                "--sim".to_owned(),
            ]
        };
        assert!(parse_options(&arguments("http://letters.example")).is_err());
        // The missing token file is reported before any network use.
        assert!(parse_options(&arguments("https://letters.example")).is_err());
    }
}
