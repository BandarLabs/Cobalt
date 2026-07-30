//! Where the daemon keeps its identity, and how it is first made.
//!
//! Everything lives in `~/.config/kobo/sidekick`: a self-signed certificate
//! and key from the `openssl` binary every Mac and Linux box carries, and a
//! short pairing code. `init` also drops the certificate into
//! `~/.config/kobo/trust`, where the host runtimes already look, so the
//! simulator trusts the daemon with no further ceremony; the reader gets the
//! same certificate over `kobo trust set sidekick --device IP`.
//!
//! The certificate names the machine's LAN address in an IP subject
//! alternative name, because that is what the reader will dial and rustls
//! verifies exactly what was dialled. A machine with more addresses than the
//! one we can see gets them added with `--host`.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Loopback port where hooks ask their questions.
pub const HOOK_PORT: u16 = 9330;
/// LAN port where the reader collects them, over TLS.
pub const READER_PORT: u16 = 9331;

/// How long `run` holds a hook's question before answering "no decision".
/// Codex allows a hook ten minutes by default and Claude Code less, so five
/// keeps everyone comfortable while never leaving an agent stuck.
pub const ASK_PATIENCE: Duration = Duration::from_secs(300);

/// What `run` needs to serve: the TLS identity and the pairing code.
pub struct State {
    pub certificate: String,
    pub key: String,
    pub pairing: String,
}

/// Reads the state `init` wrote.
///
/// # Errors
///
/// Missing files come back as the instruction to run `init`, since that is
/// always the fix.
pub fn load() -> Result<State, String> {
    let directory = state_directory()?;
    let read = |name: &str| {
        std::fs::read_to_string(directory.join(name)).map_err(|_| {
            format!(
                "no {name} in {}; run kobo-sidekickd init",
                directory.display()
            )
        })
    };
    Ok(State {
        certificate: read("cert.pem")?,
        key: read("key.pem")?,
        pairing: read("pairing")?.trim().to_owned(),
    })
}

/// Creates the daemon's identity: certificate, key, pairing code.
///
/// # Errors
///
/// Reports what could not be made -- usually a missing `openssl` binary or
/// an unwritable home -- in words that say what to do about it.
pub fn init(extra_hosts: &[String]) -> Result<(), String> {
    let directory = state_directory()?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let mut hosts = vec!["127.0.0.1".to_owned()];
    if let Some(lan) = lan_address() {
        hosts.push(lan);
    }
    let mut extra = extra_hosts.iter();
    while let Some(flag) = extra.next() {
        if flag != "--host" {
            return Err(format!(
                "unknown argument '{flag}'; expected --host ADDRESS"
            ));
        }
        let address = extra.next().ok_or("--host needs an address")?;
        hosts.push(address.clone());
    }
    hosts.dedup();
    generate_certificate(&directory, &hosts)?;
    let pairing = pairing_code()?;
    std::fs::write(directory.join("pairing"), &pairing)
        .map_err(|error| format!("write pairing code: {error}"))?;
    let trust = trust_directory()?;
    std::fs::create_dir_all(&trust)
        .map_err(|error| format!("create {}: {error}", trust.display()))?;
    let certificate = std::fs::read_to_string(directory.join("cert.pem"))
        .map_err(|error| format!("read the new certificate: {error}"))?;
    std::fs::write(trust.join("sidekick.pem"), certificate)
        .map_err(|error| format!("install the host trust root: {error}"))?;
    let reachable = hosts.get(1).cloned().unwrap_or_else(|| hosts[0].clone());
    println!("Sidekick is initialised.\n");
    println!("  address       {reachable}:{READER_PORT}");
    println!("  pairing code  {pairing}\n");
    println!("Next:");
    println!("  1. kobo trust set sidekick --device READER_IP");
    println!("  2. kobo-sidekickd setup codex   (or claude), follow it");
    println!("  3. kobo-sidekickd run");
    println!("  4. open Sidekick on the reader, enter the address and code");
    Ok(())
}

/// Asks `openssl` for a ten-year self-signed certificate naming `hosts`.
fn generate_certificate(directory: &std::path::Path, hosts: &[String]) -> Result<(), String> {
    let mut names = String::new();
    for host in hosts {
        if !names.is_empty() {
            names.push(',');
        }
        let kind = if host.parse::<std::net::IpAddr>().is_ok() {
            "IP"
        } else {
            "DNS"
        };
        names.push_str(kind);
        names.push(':');
        names.push_str(host);
    }
    let output = Command::new("openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "3650", "-nodes",
        ])
        .arg("-keyout")
        .arg(directory.join("key.pem"))
        .arg("-out")
        .arg(directory.join("cert.pem"))
        .args(["-subj", "/CN=kobo-sidekickd"])
        .arg("-addext")
        .arg(format!("subjectAltName={names}"))
        .output()
        .map_err(|error| format!("run openssl: {error}; is it installed?"))?;
    if !output.status.success() {
        return Err(format!(
            "openssl refused to make the certificate:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// The address the reader will dial: whichever interface routes outward.
///
/// Connecting a UDP socket sends nothing; it only asks the kernel which
/// source address it would use, which is exactly the question.
fn lan_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

/// Six characters a person can copy across the room without squinting:
/// no zero against o, no one against l.
fn pairing_code() -> Result<String, String> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    use std::io::Read;
    let mut noise = [0_u8; 6];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut noise))
        .map_err(|error| format!("read /dev/urandom: {error}"))?;
    let mut code = String::new();
    for byte in noise {
        code.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
    }
    Ok(code)
}

/// `~/.config/kobo/sidekick`.
///
/// # Errors
///
/// Only when there is no `HOME` to build it under.
pub fn state_directory() -> Result<PathBuf, String> {
    Ok(config_kobo()?.join("sidekick"))
}

/// `~/.config/kobo/trust`, the directory every host runtime reads roots from.
fn trust_directory() -> Result<PathBuf, String> {
    Ok(config_kobo()?.join("trust"))
}

fn config_kobo() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("no HOME in the environment")?;
    Ok(PathBuf::from(home).join(".config").join("kobo"))
}
