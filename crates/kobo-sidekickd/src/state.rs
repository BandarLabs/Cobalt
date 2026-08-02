//! Where the daemon keeps its identity, and how it is first made.
//!
//! Everything lives in `~/.config/kobo/sidekick`: a small certificate
//! authority made once, a leaf certificate the daemon mints for itself from
//! whatever addresses the machine has right now, and a short pairing code.
//! All of it comes from the `openssl` binary every Mac and Linux box carries.
//!
//! The split matters. rustls verifies exactly the address that was dialled,
//! so a certificate that names the machine's LAN address goes stale every
//! time the router hands out a different one. The reader therefore trusts
//! the authority, which names no address and never changes, and `run` mints
//! a fresh leaf under it whenever the addresses have moved. A new IP costs a
//! daemon restart and nothing else: no cable, no `kobo trust set`, no new
//! pairing code.
//!
//! `init` also drops the authority into `~/.config/kobo/trust`, where the
//! host runtimes already look, so the simulator trusts the daemon with no
//! further ceremony; the reader gets the same file over
//! `kobo trust set sidekick --device IP`, once. A machine with more
//! addresses than the one we can see gets them added with `--host`.

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

/// Reads the state `init` wrote, minting a fresh leaf certificate first if
/// the machine's addresses have changed since the last one was made.
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
    // Older installs have a self-signed cert.pem and no authority. They need
    // one `init` (and one re-trust on the reader); after that, never again.
    read(CA_CERT)?;
    refresh_leaf(&directory)?;
    Ok(State {
        certificate: read(LEAF_CERT)?,
        key: read(LEAF_KEY)?,
        pairing: read("pairing")?.trim().to_owned(),
    })
}

const CA_CERT: &str = "ca-cert.pem";
const CA_KEY: &str = "ca-key.pem";
const LEAF_CERT: &str = "cert.pem";
const LEAF_KEY: &str = "key.pem";
/// Extra `--host` addresses, kept so `run` can mint leaves with them too.
const HOSTS: &str = "hosts";
/// What the current leaf names, so a change of address is noticed.
const LEAF_HOSTS: &str = "leaf-hosts";

/// Creates the daemon's identity: authority, leaf certificate, pairing code.
///
/// Made to be run again without ceremony: an existing authority and pairing
/// code are kept, so nothing the reader already trusts or knows is thrown
/// away just because the addresses changed.
///
/// # Errors
///
/// Reports what could not be made -- usually a missing `openssl` binary or
/// an unwritable home -- in words that say what to do about it.
pub fn init(extra_hosts: &[String]) -> Result<(), String> {
    let directory = state_directory()?;
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let mut extras = Vec::new();
    let mut extra = extra_hosts.iter();
    while let Some(flag) = extra.next() {
        if flag != "--host" {
            return Err(format!(
                "unknown argument '{flag}'; expected --host ADDRESS"
            ));
        }
        extras.push(extra.next().ok_or("--host needs an address")?.clone());
    }
    std::fs::write(directory.join(HOSTS), extras.join("\n"))
        .map_err(|error| format!("write the extra hosts: {error}"))?;
    let trusted_already = directory.join(CA_CERT).exists();
    ensure_authority(&directory)?;
    // A stale record forces a mint even when the addresses look unchanged,
    // because a fresh authority makes every earlier leaf worthless.
    if !trusted_already {
        let _ = std::fs::remove_file(directory.join(LEAF_HOSTS));
    }
    refresh_leaf(&directory)?;
    let pairing = match std::fs::read_to_string(directory.join("pairing")) {
        Ok(code) if !code.trim().is_empty() => code.trim().to_owned(),
        _ => {
            let code = pairing_code()?;
            std::fs::write(directory.join("pairing"), &code)
                .map_err(|error| format!("write pairing code: {error}"))?;
            code
        }
    };
    let trust = trust_directory()?;
    std::fs::create_dir_all(&trust)
        .map_err(|error| format!("create {}: {error}", trust.display()))?;
    let authority = std::fs::read_to_string(directory.join(CA_CERT))
        .map_err(|error| format!("read the authority: {error}"))?;
    std::fs::write(trust.join("sidekick.pem"), authority)
        .map_err(|error| format!("install the host trust root: {error}"))?;
    let reachable = lan_address().unwrap_or_else(|| "127.0.0.1".to_owned());
    println!("Sidekick is initialised.\n");
    println!("  address       {reachable}:{READER_PORT}");
    println!("  pairing code  {pairing}\n");
    if trusted_already {
        println!("The authority and pairing code were kept, so a reader that");
        println!("already trusts this daemon needs nothing done to it.\n");
    }
    println!("Next:");
    println!("  1. kobo trust set sidekick --device READER_IP");
    println!("  2. kobo-sidekickd setup codex   (or claude), follow it");
    println!("  3. kobo-sidekickd run");
    println!("  4. open Sidekick on the reader, enter the address and code");
    Ok(())
}

/// The addresses the leaf certificate must name right now: loopback, the
/// LAN address, and whatever `init --host` was told about.
fn wanted_hosts(directory: &std::path::Path) -> Vec<String> {
    let mut hosts = vec!["127.0.0.1".to_owned()];
    if let Some(lan) = lan_address() {
        hosts.push(lan);
    }
    if let Ok(extras) = std::fs::read_to_string(directory.join(HOSTS)) {
        for line in extras
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            hosts.push(line.to_owned());
        }
    }
    hosts.dedup();
    hosts
}

/// Makes the authority if there is none. Never replaces one: the reader
/// trusts it by fingerprint, and a new authority means a trip to the reader.
fn ensure_authority(directory: &std::path::Path) -> Result<(), String> {
    if directory.join(CA_CERT).exists() && directory.join(CA_KEY).exists() {
        return Ok(());
    }
    let output = Command::new("openssl")
        .args([
            "req", "-x509", "-newkey", "rsa:2048", "-sha256", "-days", "3650", "-nodes",
        ])
        .arg("-keyout")
        .arg(directory.join(CA_KEY))
        .arg("-out")
        .arg(directory.join(CA_CERT))
        .args(["-subj", "/CN=kobo-sidekickd authority"])
        .args(["-addext", "basicConstraints=critical,CA:TRUE"])
        .args(["-addext", "keyUsage=critical,keyCertSign"])
        .output()
        .map_err(|error| format!("run openssl: {error}; is it installed?"))?;
    if !output.status.success() {
        return Err(format!(
            "openssl refused to make the authority:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Mints a leaf for the machine's current addresses, unless the one on disk
/// already names exactly those.
fn refresh_leaf(directory: &std::path::Path) -> Result<(), String> {
    let hosts = wanted_hosts(directory);
    let record = hosts.join("\n");
    let current = std::fs::read_to_string(directory.join(LEAF_HOSTS)).unwrap_or_default();
    if current == record && directory.join(LEAF_CERT).exists() {
        return Ok(());
    }
    mint_leaf(directory, &hosts)?;
    std::fs::write(directory.join(LEAF_HOSTS), record)
        .map_err(|error| format!("record the leaf's addresses: {error}"))?;
    if hosts.len() > 1 {
        println!(
            "sidekick: minted a certificate for {}",
            hosts[1..].join(", ")
        );
    }
    Ok(())
}

/// Asks `openssl` for a leaf naming `hosts`, signed by the authority. The
/// written `cert.pem` carries the authority behind the leaf, so the server
/// presents the whole chain.
fn mint_leaf(directory: &std::path::Path, hosts: &[String]) -> Result<(), String> {
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
    let request = directory.join("leaf.csr");
    let extensions = directory.join("leaf.ext");
    std::fs::write(
        &extensions,
        format!(
            "subjectAltName={names}\n\
             basicConstraints=CA:FALSE\n\
             keyUsage=digitalSignature,keyEncipherment\n\
             extendedKeyUsage=serverAuth\n"
        ),
    )
    .map_err(|error| format!("write the leaf extensions: {error}"))?;
    let output = Command::new("openssl")
        .args(["req", "-newkey", "rsa:2048", "-nodes"])
        .arg("-keyout")
        .arg(directory.join(LEAF_KEY))
        .arg("-out")
        .arg(&request)
        .args(["-subj", "/CN=kobo-sidekickd"])
        .output()
        .map_err(|error| format!("run openssl: {error}; is it installed?"))?;
    if !output.status.success() {
        return Err(format!(
            "openssl refused to make the leaf request:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let output = Command::new("openssl")
        .args([
            "x509",
            "-req",
            "-sha256",
            "-days",
            "3650",
            "-CAcreateserial",
        ])
        .arg("-in")
        .arg(&request)
        .arg("-CA")
        .arg(directory.join(CA_CERT))
        .arg("-CAkey")
        .arg(directory.join(CA_KEY))
        .arg("-extfile")
        .arg(&extensions)
        .arg("-out")
        .arg(directory.join(LEAF_CERT))
        .output()
        .map_err(|error| format!("run openssl: {error}; is it installed?"))?;
    let _ = std::fs::remove_file(&request);
    let _ = std::fs::remove_file(&extensions);
    if !output.status.success() {
        return Err(format!(
            "openssl refused to sign the leaf:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let authority = std::fs::read_to_string(directory.join(CA_CERT))
        .map_err(|error| format!("read the authority: {error}"))?;
    let leaf = std::fs::read_to_string(directory.join(LEAF_CERT))
        .map_err(|error| format!("read the new leaf: {error}"))?;
    std::fs::write(directory.join(LEAF_CERT), format!("{leaf}{authority}"))
        .map_err(|error| format!("write the chain: {error}"))?;
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
