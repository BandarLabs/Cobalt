//! Owner-attended Read Later sign-in: complete Wallabag's OAuth password
//! grant on the computer, where typing is easy, and deliver the resulting
//! session to the reader. The application refreshes the token itself from
//! then on; the CLI is needed again only when the refresh grant is revoked.

use std::fs;
use std::time::Duration;

const DEVICE_ROOT: &str = "/mnt/onboard/.adds/cobalt/data/readlater";
const SESSION_FILE: &str = "session.v1";
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);
const USAGE: &str = "usage: kobo readlater login --server URL --client-id ID --client-secret-file PATH \\\n                     \x20      --username EMAIL [--password-env VAR | --password-file PATH] \\\n                     \x20      (--sim | --device IP)\n\
                     \n\
                     Signs Read Later in to a Wallabag server. The client id and\n\
                     secret come from the server's API client management page\n\
                     (wallabag.it: Settings > API clients). The password is read\n\
                     from KOBO_WALLABAG_PASSWORD unless --password-env or\n\
                     --password-file names another source; it never appears on\n\
                     the command line. The reader renews the session itself\n\
                     after login; run this again only if the grant is revoked.";

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
    server: String,
    client_id: String,
    client_secret: String,
    username: String,
    password: String,
    target: Target,
}

fn parse_options(arguments: &[String]) -> Result<Options, String> {
    let mut server = None;
    let mut client_id = None;
    let mut client_secret_file = None;
    let mut username = None;
    let mut password_env = None;
    let mut password_file = None;
    let mut target = None;
    let mut rest = arguments;
    while let Some((flag, tail)) = rest.split_first() {
        let (value, tail) = match flag.as_str() {
            "--sim" => {
                target = Some(Target::Sim);
                (None, tail)
            }
            "--device" => {
                let (ip, tail) = tail.split_first().ok_or_else(|| USAGE.to_owned())?;
                target = Some(Target::Device(ip.clone()));
                (None, tail)
            }
            "--server"
            | "--client-id"
            | "--client-secret-file"
            | "--username"
            | "--password-env"
            | "--password-file" => {
                let (value, tail) = tail.split_first().ok_or_else(|| USAGE.to_owned())?;
                (Some(value), tail)
            }
            _ => return Err(USAGE.to_owned()),
        };
        if let Some(value) = value {
            match flag.as_str() {
                "--server" => server = Some(value.clone()),
                "--client-id" => client_id = Some(value.clone()),
                "--client-secret-file" => client_secret_file = Some(value.clone()),
                "--username" => username = Some(value.clone()),
                "--password-env" => password_env = Some(value.clone()),
                "--password-file" => password_file = Some(value.clone()),
                _ => unreachable!(),
            }
        }
        rest = tail;
    }
    let server = server.ok_or_else(|| USAGE.to_owned())?;
    if !server.starts_with("https://") {
        return Err("the Wallabag server must be an https:// address".to_owned());
    }
    let client_secret_file = client_secret_file.ok_or_else(|| USAGE.to_owned())?;
    Ok(Options {
        server: server.trim_end_matches('/').to_owned(),
        client_id: client_id.ok_or_else(|| USAGE.to_owned())?,
        client_secret: secret_file(&client_secret_file, "client secret")?,
        username: username.ok_or_else(|| USAGE.to_owned())?,
        password: password(password_env, password_file)?,
        target: target.ok_or_else(|| USAGE.to_owned())?,
    })
}

fn login(arguments: &[String]) -> Result<(), String> {
    let options = parse_options(arguments)?;
    let Options {
        server,
        client_id,
        client_secret,
        username,
        password,
        target,
    } = options;

    let body = format!(
        "grant_type=password&client_id={}&client_secret={}&username={}&password={}",
        form(&client_id),
        form(&client_secret),
        form(&username),
        form(&password),
    );
    let answer = kobo_net::post(
        &format!("{server}/oauth/v2/token"),
        body.as_bytes(),
        "application/x-www-form-urlencoded",
        None,
        &[],
        16 * 1024,
    )
    .map_err(|error| format!("the Wallabag sign-in failed: {error}"))?;
    let token = parse_token(&answer)
        .ok_or_else(|| "the Wallabag sign-in answer held no tokens".to_owned())?;

    let session = format!(
        "{{\"version\":\"1\",\"server\":{},\"client_id\":{},\"client_secret\":{},\"refresh_token\":{},\"access_token\":{}}}",
        json_string(&server),
        json_string(&client_id),
        json_string(&client_secret),
        json_string(&token.refresh_token),
        json_string(&token.access_token),
    );
    match target {
        Target::Sim => {
            let root = kobo_sim::simulated_data_root("readlater");
            fs::create_dir_all(&root).map_err(|error| format!("create {root:?}: {error}"))?;
            let path = root.join(SESSION_FILE);
            fs::write(&path, session).map_err(|error| format!("write {path:?}: {error}"))?;
            println!("Signed in to {server}; the simulator's Read Later picks up the session on its next start.");
        }
        Target::Device(host) => {
            let encoded = super::base64_encode(session.as_bytes());
            let script = format!(
                "set -eu\n\
                 root='{DEVICE_ROOT}'\n\
                 mkdir -p \"$root\"\n\
                 chmod 700 \"$root\"\n\
                 partial=\"$root/.{SESSION_FILE}.writing\"\n\
                 base64 -d > \"$partial\" <<'KOBO_READLATER_SESSION'\n\
                 {encoded}\n\
                 KOBO_READLATER_SESSION\n\
                 chmod 600 \"$partial\"\n\
                 mv -f \"$partial\" \"$root/{SESSION_FILE}\"\n\
                 sync\n"
            );
            remote(&host, &script)?;
            println!("Signed in to {server}; Read Later on {host} picks up the session on its next start.");
        }
    }
    Ok(())
}

fn secret_file(file: &str, label: &str) -> Result<String, String> {
    let metadata = fs::symlink_metadata(file).map_err(|error| format!("read {file}: {error}"))?;
    if !metadata.file_type().is_file() || metadata.len() > 4096 {
        return Err(format!(
            "the {label} file must be a regular file no larger than 4 KiB"
        ));
    }
    let value = fs::read_to_string(file).map_err(|error| format!("read {file}: {error}"))?;
    let value = value.trim_end().to_owned();
    if value.is_empty() {
        return Err(format!("the {label} file is empty"));
    }
    Ok(value)
}

fn password(env: Option<String>, file: Option<String>) -> Result<String, String> {
    if let Some(file) = file {
        return secret_file(&file, "password");
    }
    let variable = env.unwrap_or_else(|| "KOBO_WALLABAG_PASSWORD".to_owned());
    std::env::var(&variable)
        .map_err(|_| format!("the password environment variable {variable} is not set"))
}

struct Tokens {
    access_token: String,
    refresh_token: String,
}

fn parse_token(bytes: &[u8]) -> Option<Tokens> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let text = |name: &str| {
        let found = value.get(name)?.as_str()?.trim();
        (!found.is_empty()).then(|| found.to_owned())
    };
    Some(Tokens {
        access_token: text("access_token")?,
        refresh_token: text("refresh_token")?,
    })
}

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

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch < ' ' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
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
                    "Read Later session transfer to {host} exited with {}",
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
    fn client_secret_is_never_accepted_as_a_command_line_value() {
        let args = [
            "--server",
            "https://wallabag.example",
            "--client-id",
            "owner",
            "--client-secret",
            "visible-in-shell-history",
            "--username",
            "owner@example.com",
            "--sim",
        ]
        .map(str::to_owned);
        let Err(error) = parse_options(&args) else {
            panic!("command-line secret was accepted")
        };
        assert!(error.contains("usage:"));
    }

    #[test]
    fn token_answers_parse_and_bad_answers_do_not() {
        let tokens =
            parse_token(br#"{"access_token":"a","refresh_token":"r","expires_in":3600}"#).unwrap();
        assert_eq!(tokens.access_token, "a");
        assert_eq!(tokens.refresh_token, "r");
        assert!(parse_token(br#"{"error":"invalid_grant"}"#).is_none());
        assert!(parse_token(b"not json").is_none());
    }

    #[test]
    fn form_values_escape_and_json_strings_quote() {
        assert_eq!(form("a b+c/d"), "a%20b%2Bc%2Fd");
        assert_eq!(form("plain-1_2.3~4"), "plain-1_2.3~4");
        assert_eq!(json_string("say \"hi\"\\"), "\"say \\\"hi\\\"\\\\\"");
        assert_eq!(json_string("line\nbreak"), "\"line\\u000abreak\"");
    }
}
