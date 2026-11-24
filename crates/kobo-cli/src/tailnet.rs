//! Whether this computer is on a Tailscale tailnet, and what it is called
//! there.
//!
//! A reader on its owner's tailnet can reach this computer from any network,
//! but only at a host the computer's certificate names. Pairing used to name
//! the address the home router gave out and nothing else, so an owner who
//! wanted Paperterm in a cafe had to know their tailnet address, pass it with
//! `--host`, and know that they had to. The computer can find that out
//! itself.
//!
//! It asks the Tailscale command-line tool, because that is the one source
//! that knows: an interface with an address in the tailnet range says nothing
//! about the name, or about whether the tailnet is connected. The tool is not
//! always there and not always on `PATH`, so every way of failing to get an
//! answer is the same answer, "not on a tailnet", and never an error. Pairing
//! on the local network must work exactly as it did on a computer that has
//! never heard of Tailscale.

use std::ffi::OsString;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How long the tool is given to answer, across every place it is looked for.
///
/// It answers from a local socket in a few hundredths of a second. This is
/// long enough for a busy machine and short enough that pairing never appears
/// to hang on a tool that is installed and stuck.
#[cfg_attr(
    test,
    allow(dead_code, reason = "tests never start the tool, so nothing waits")
)]
const PATIENCE: Duration = Duration::from_secs(2);

/// The most of the tool's answer that is read. Without its peers the answer
/// is a few kilobytes; a megabyte is something else talking.
const LARGEST_ANSWER: u64 = 1024 * 1024;

/// This computer as its tailnet knows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tailnet {
    /// The full tailnet name, such as `studio.example.ts.net`, when the
    /// tailnet gives one.
    pub name: Option<String>,
    /// The address every device on the tailnet reaches this computer at.
    pub address: String,
}

impl Tailnet {
    /// The first label of the name, which the reader also resolves.
    fn short_name(&self) -> Option<&str> {
        let name = self.name.as_deref()?;
        let short = name.split('.').next()?;
        (short != name).then_some(short)
    }

    /// Every host a reader on the tailnet might dial this computer at, the
    /// one worth writing down first.
    ///
    /// The reader keeps each tailnet computer's full name and short name in
    /// its hosts file while its Tailscale switch is on, so all three reach
    /// this computer, and a certificate that left one out would refuse the
    /// owner who typed that one.
    #[must_use]
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts = Vec::new();
        hosts.extend(self.name.clone());
        hosts.extend(self.short_name().map(str::to_owned));
        hosts.push(self.address.clone());
        hosts
    }

    /// The host a pairing meant to work away from home is written with: the
    /// name, which says whose computer it is, or the address when the
    /// tailnet gives this computer no name.
    #[must_use]
    pub fn preferred_host(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.address)
    }
}

/// Something that can be asked for the local Tailscale status.
///
/// A trait so the commands that pair a reader can be tested with an answer
/// written into the test. No test runs the real tool: what it says depends on
/// the machine the test is on.
pub trait StatusSource {
    /// The JSON the tool prints for `status --json`, or `None` when there is
    /// no tool, no answer in time, or a failure of any other kind.
    fn status(&self) -> Option<String>;
}

/// The Tailscale tool installed on this computer.
pub struct Installed;

impl StatusSource for Installed {
    #[cfg(not(test))]
    fn status(&self) -> Option<String> {
        let deadline = Instant::now() + PATIENCE;
        candidates(
            std::env::consts::OS,
            &[
                std::env::var_os("ProgramFiles"),
                std::env::var_os("ProgramFiles(x86)"),
            ],
        )
        .into_iter()
        .find_map(|program| ask(&program, deadline))
    }

    /// Under test this never starts a program, so a test that reaches it by
    /// accident sees a computer with no tailnet rather than this machine's.
    #[cfg(test)]
    fn status(&self) -> Option<String> {
        None
    }
}

/// A source for a pairing a tailnet has no part in, which is never asked.
pub struct NotAsked;

impl StatusSource for NotAsked {
    fn status(&self) -> Option<String> {
        None
    }
}

/// This computer's place on its tailnet, or `None` when it has none.
#[must_use]
pub fn detect(source: &dyn StatusSource) -> Option<Tailnet> {
    parse(&source.status()?)
}

/// Where the tool may be, in the order tried.
///
/// `PATH` first, which is where a package manager puts it on Linux, where
/// Homebrew puts it on a Mac and where the Windows installer adds it. Then
/// the places it lives when `PATH` does not say so: the Mac app carries the
/// tool inside its bundle and puts nothing on `PATH`, and a command started
/// from a desktop app inherits a `PATH` without the package manager's
/// folders.
fn candidates(os: &str, program_folders: &[Option<OsString>]) -> Vec<PathBuf> {
    let mut found = vec![PathBuf::from("tailscale")];
    match os {
        "macos" => found.extend(
            [
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
                "/opt/homebrew/bin/tailscale",
                "/usr/local/bin/tailscale",
            ]
            .map(PathBuf::from),
        ),
        "windows" => found.extend(program_folders.iter().flatten().map(|folder| {
            PathBuf::from(folder)
                .join("Tailscale")
                .join("tailscale.exe")
        })),
        _ => found.extend(["/usr/bin/tailscale", "/usr/local/bin/tailscale"].map(PathBuf::from)),
    }
    found
}

/// Runs one candidate and returns what it printed, or `None` when it is not
/// there, failed, or was still running at `deadline`.
///
/// `--peers=false` keeps the answer to this computer alone. The rest of the
/// tailnet is none of this command's business, and on a large tailnet it is
/// most of the output.
#[cfg_attr(
    test,
    allow(dead_code, reason = "tests never start the tool; see Installed")
)]
fn ask(program: &std::path::Path, deadline: Instant) -> Option<String> {
    let mut child = Command::new(program)
        .args(["status", "--json", "--peers=false"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let output = child.stdout.take()?;
    // Read on a thread of its own: a child that fills the pipe and is not
    // being read never exits, and would be waited on until the deadline.
    let reader = thread::spawn(move || {
        let mut text = String::new();
        output
            .take(LARGEST_ANSWER)
            .read_to_string(&mut text)
            .ok()
            .map(|_| text)
    });
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return status
                    .success()
                    .then(|| reader.join().ok().flatten())
                    .flatten();
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Reads the three facts this needs out of the tool's status and nothing
/// else: that the tailnet is connected, this computer's name, and its
/// address.
///
/// Anything short of a running tailnet with an address in the tailnet's own
/// range is `None`. A signed-out or stopped Tailscale still reports the name
/// and address it last had, and a certificate naming those would promise a
/// way to this computer that is not there.
///
/// The address has to be the IPv4 one. Paperterm's host and the Sidekick
/// helper both listen on IPv4 alone, and the reader keeps each tailnet name
/// against the first address its own Tailscale reports, which is the IPv4
/// one whenever there is one. A tailnet with IPv4 switched off is therefore
/// one this computer cannot be reached on yet, and is not offered.
fn parse(json: &str) -> Option<Tailnet> {
    let status = kobo_json::parse(json).ok()?;
    if status.get("BackendState")?.as_str()? != "Running" {
        return None;
    }
    let own = status.get("Self")?;
    let address = own
        .get("TailscaleIPs")?
        .as_array()?
        .iter()
        .filter_map(kobo_json::Value::as_str)
        .find(|address| {
            kobo_stream::is_tailnet_address(address)
                && address.parse::<std::net::Ipv4Addr>().is_ok()
        })?
        .to_owned();
    let name = own
        .get("DNSName")
        .and_then(kobo_json::Value::as_str)
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase())
        .filter(|name| is_host_name(name));
    Some(Tailnet { name, address })
}

/// Whether `name` is a host name and nothing more.
///
/// The name arrives in a JSON string, goes into a certificate and may be
/// written onto the reader as the address Paperterm dials, so it is held to
/// the letters a host name is made of before it goes anywhere.
fn is_host_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// The hosts one pairing mints a certificate for, and the ones it writes onto
/// the reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    /// Every host the certificate names, in the order given to the mint.
    pub certificate: Vec<String>,
    /// The hosts the reader is told to dial, the one to try first first.
    /// Empty when no host is known; never more than two.
    pub written: Vec<String>,
    /// What the tailnet added to the certificate, to say when it is minted.
    pub named: Option<String>,
    /// Where the written hosts work, to say once the reader holds them. Only
    /// a caller that has delivered the pairing may say it.
    pub reach: Option<String>,
}

/// Decides both, from what the owner asked for and what this computer has.
///
/// `explicit` is every `--host` given, and `local` this computer's address on
/// the network it is on.
///
/// Neither host works everywhere. The local address works while the reader
/// is on the same network as this computer, with nothing else switched on.
/// The tailnet name works on any network, but only while Tailscale is on at
/// both ends. So a computer on a tailnet writes both, and each app tries the
/// one that last answered first and the other when that one is not there:
///
/// * By default the reader is given the local address, then the tailnet
///   name when this computer is on a tailnet. Without a tailnet it is the
///   local address alone, as it always was.
/// * With `--tailnet` it is the tailnet name alone, for an owner who wants
///   the reader to reach this computer only that way.
///
/// `--host` given without `--tailnet` is the owner naming the hosts
/// themselves: the reader is given the first, nothing is added to what they
/// named, and the tool is not asked.
///
/// # Errors
/// Fails when `--tailnet` is asked for on a computer that is not on one.
pub fn plan(
    explicit: &[String],
    local: Option<&str>,
    prefer_tailnet: bool,
    source: &dyn StatusSource,
) -> Result<Plan, String> {
    if !explicit.is_empty() && !prefer_tailnet {
        return Ok(Plan {
            certificate: explicit.to_vec(),
            written: explicit.iter().take(1).cloned().collect(),
            named: None,
            reach: None,
        });
    }
    let mut certificate: Vec<String> = if explicit.is_empty() {
        local.map(str::to_owned).into_iter().collect()
    } else {
        explicit.to_vec()
    };
    let Some(tailnet) = detect(source) else {
        if prefer_tailnet {
            return Err(
                "This computer is not on a tailnet, so there is no tailnet name to pair with. \
                 Turn Tailscale on here and run this again."
                    .to_owned(),
            );
        }
        return Ok(Plan {
            written: certificate.iter().take(1).cloned().collect(),
            certificate,
            named: None,
            reach: None,
        });
    };
    let usual = certificate.first().cloned();
    let tailnet_hosts = tailnet.hosts();
    let added: Vec<String> = tailnet_hosts
        .iter()
        .filter(|host| !certificate.contains(host))
        .cloned()
        .collect();
    certificate.extend(added.iter().cloned());
    let also = if added.len() == certificate.len() {
        ""
    } else {
        "also "
    };
    let named = (!added.is_empty()).then(|| {
        format!(
            "This computer is on a tailnet, so the certificate {also}names {}.",
            listed(&added)
        )
    });
    let away = tailnet.preferred_host().to_owned();
    // With no address of its own to write, the tailnet host is the only one
    // there is. And when traffic leaves this computer through its tailnet,
    // the address it finds for itself is already a tailnet one, which a
    // second tailnet host would add nothing to.
    let written = match usual {
        Some(usual) if !prefer_tailnet && !tailnet_hosts.contains(&usual) => vec![usual, away],
        Some(usual) if !prefer_tailnet => vec![usual],
        _ => vec![away],
    };
    let reach = match written.as_slice() {
        [home, away] => format!(
            "The reader reaches this computer at home on the same Wi-Fi ({home}), and \
             anywhere while Tailscale is on here and on the reader ({away})."
        ),
        [only, ..] => {
            let kind = if kobo_stream::is_tailnet_address(only) {
                "address"
            } else {
                "name"
            };
            format!(
                "The reader reaches this computer at its tailnet {kind}, {only}: from any \
                 network, and only while Tailscale is on here and on the reader."
            )
        }
        [] => unreachable!("a tailnet always gives a host"),
    };
    Ok(Plan {
        certificate,
        written,
        named,
        reach: Some(reach),
    })
}

/// "a", "a and b", "a, b and c".
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::{candidates, detect, parse, plan, Installed, StatusSource, Tailnet};
    use std::path::PathBuf;

    /// A status answer written into the test, standing in for the tool.
    struct Says(Option<String>);

    impl StatusSource for Says {
        fn status(&self) -> Option<String> {
            self.0.clone()
        }
    }

    fn says(answer: &str) -> Says {
        Says(Some(answer.to_owned()))
    }

    /// What the tool prints for a connected computer, cut to the fields a
    /// real answer carries around the three that are read.
    const CONNECTED: &str = r#"{
        "Version": "1.80.0",
        "TUN": true,
        "BackendState": "Running",
        "AuthURL": "",
        "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1234"],
        "Self": {
            "HostName": "Studio",
            "DNSName": "studio.example.ts.net.",
            "OS": "macOS",
            "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1234"],
            "Online": true
        },
        "MagicDNSSuffix": "example.ts.net",
        "Peer": null
    }"#;

    fn studio() -> Tailnet {
        Tailnet {
            name: Some("studio.example.ts.net".to_owned()),
            address: "100.101.102.103".to_owned(),
        }
    }

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn a_connected_computer_is_read_as_its_tailnet_name_and_address() {
        assert_eq!(detect(&says(CONNECTED)), Some(studio()));
        assert_eq!(
            studio().hosts(),
            ["studio.example.ts.net", "studio", "100.101.102.103"]
        );
    }

    #[test]
    fn a_computer_whose_tailscale_is_stopped_or_signed_out_is_not_on_a_tailnet() {
        // Both still print the name and address the computer last had.
        for state in ["Stopped", "NeedsLogin", "NeedsMachineAuth", "Starting"] {
            let status = CONNECTED.replace("Running", state);
            assert_eq!(parse(&status), None, "{state}");
        }
    }

    #[test]
    fn every_way_of_getting_no_usable_answer_is_not_on_a_tailnet() {
        assert_eq!(detect(&Says(None)), None, "no tool");
        for answer in [
            "",
            "not json",
            "{}",
            r#"{"BackendState":"Running"}"#,
            r#"{"BackendState":"Running","Self":{"DNSName":"studio.example.ts.net."}}"#,
            r#"{"BackendState":"Running","Self":{"TailscaleIPs":[]}}"#,
            // An address outside the tailnet's ranges is not a tailnet address,
            // whatever the field is called.
            r#"{"BackendState":"Running","Self":{"TailscaleIPs":["192.168.1.20"]}}"#,
        ] {
            assert_eq!(parse(answer), None, "{answer}");
        }
    }

    #[test]
    fn a_name_that_is_not_a_host_name_is_left_out_and_the_address_still_counts() {
        // The last carries a line break inside the JSON string.
        for name in ["", ".", "studio example.ts.net.", "studio\\nevil.ts.net."] {
            let status = CONNECTED.replace("studio.example.ts.net.", name);
            let tailnet = parse(&status).expect("the address is enough");
            assert_eq!(tailnet.name, None, "{name:?}");
            assert_eq!(tailnet.hosts(), ["100.101.102.103"]);
            assert_eq!(tailnet.preferred_host(), "100.101.102.103");
        }
    }

    #[test]
    fn a_tailnet_without_an_ipv4_address_is_not_offered_because_nothing_listens_on_ipv6() {
        let status = CONNECTED.replace(r#""100.101.102.103", "#, "");
        assert!(status.contains(r#""TailscaleIPs": ["fd7a:115c:a1e0::1234"]"#));
        assert_eq!(parse(&status), None);
    }

    #[test]
    fn the_real_tool_is_never_started_by_a_test() {
        assert_eq!(Installed.status(), None);
    }

    #[test]
    fn the_tool_is_looked_for_on_the_path_first_and_then_where_each_system_keeps_it() {
        let mac = candidates("macos", &[]);
        assert_eq!(mac[0], PathBuf::from("tailscale"));
        assert!(mac.contains(&PathBuf::from(
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
        )));
        let linux = candidates("linux", &[]);
        assert_eq!(linux[0], PathBuf::from("tailscale"));
        assert!(linux.contains(&PathBuf::from("/usr/bin/tailscale")));
        let windows = candidates("windows", &[Some("C:\\Program Files".into()), None]);
        assert_eq!(windows[0], PathBuf::from("tailscale"));
        assert_eq!(
            windows[1],
            PathBuf::from("C:\\Program Files")
                .join("Tailscale")
                .join("tailscale.exe")
        );
        assert_eq!(windows.len(), 2, "a folder that is not set is not guessed");
    }

    #[test]
    fn without_a_tailnet_pairing_is_exactly_what_it_was() {
        let found = plan(&[], Some("192.168.1.20"), false, &Says(None)).expect("plans");
        assert_eq!(found.certificate, ["192.168.1.20"]);
        assert_eq!(found.written, ["192.168.1.20"]);
        assert_eq!((found.named, found.reach), (None, None));
        let nothing = plan(&[], None, false, &Says(None)).expect("plans");
        assert!(nothing.certificate.is_empty());
        assert!(nothing.written.is_empty());
    }

    #[test]
    fn on_a_tailnet_the_reader_is_given_the_local_address_and_then_the_tailnet_name() {
        let found = plan(&[], Some("192.168.1.20"), false, &says(CONNECTED)).expect("plans");
        assert_eq!(
            found.certificate,
            [
                "192.168.1.20",
                "studio.example.ts.net",
                "studio",
                "100.101.102.103"
            ]
        );
        assert_eq!(found.written, ["192.168.1.20", "studio.example.ts.net"]);
        assert_eq!(
            found.named.as_deref(),
            Some(
                "This computer is on a tailnet, so the certificate also names \
                 studio.example.ts.net, studio and 100.101.102.103."
            )
        );
        assert_eq!(
            found.reach.as_deref(),
            Some(
                "The reader reaches this computer at home on the same Wi-Fi (192.168.1.20), and \
                 anywhere while Tailscale is on here and on the reader (studio.example.ts.net)."
            )
        );
    }

    #[test]
    fn asking_for_the_tailnet_writes_the_tailnet_name_alone_onto_the_reader() {
        let found = plan(&[], Some("192.168.1.20"), true, &says(CONNECTED)).expect("plans");
        assert_eq!(found.certificate[0], "192.168.1.20");
        assert!(found.certificate.contains(&"100.101.102.103".to_owned()));
        assert_eq!(found.written, ["studio.example.ts.net"]);
        assert_eq!(
            found.reach.as_deref(),
            Some(
                "The reader reaches this computer at its tailnet name, studio.example.ts.net: \
                 from any network, and only while Tailscale is on here and on the reader."
            )
        );
    }

    #[test]
    fn asking_for_the_tailnet_on_a_computer_without_one_is_refused() {
        for source in [Says(None), says(r#"{"BackendState":"Stopped"}"#)] {
            let error = plan(&[], Some("192.168.1.20"), true, &source)
                .expect_err("there is no tailnet name to write");
            assert!(error.contains("not on a tailnet"), "{error}");
            assert!(error.contains("Turn Tailscale on"), "{error}");
        }
    }

    #[test]
    fn hosts_the_owner_named_are_minted_as_given_and_the_tool_is_not_asked() {
        /// Fails the test if anything asks it.
        struct NeverAsked;
        impl StatusSource for NeverAsked {
            fn status(&self) -> Option<String> {
                panic!("--host without --tailnet asked for the tailnet");
            }
        }
        let named = words(&["studio.local", "10.0.0.4"]);
        let found = plan(&named, Some("192.168.1.20"), false, &NeverAsked).expect("plans");
        assert_eq!(found.certificate, named);
        assert_eq!(
            found.written,
            ["studio.local"],
            "the first, as before, and nothing more"
        );
        assert_eq!((found.named, found.reach), (None, None));
    }

    #[test]
    fn named_hosts_and_the_tailnet_together_keep_both_and_write_the_tailnet_name() {
        let found = plan(
            &words(&["studio.local"]),
            Some("192.168.1.20"),
            true,
            &says(CONNECTED),
        )
        .expect("plans");
        assert_eq!(
            found.certificate,
            [
                "studio.local",
                "studio.example.ts.net",
                "studio",
                "100.101.102.103"
            ]
        );
        assert_eq!(found.written, ["studio.example.ts.net"]);
    }

    #[test]
    fn a_computer_that_finds_its_tailnet_address_for_itself_is_not_told_it_is_on_this_network() {
        // With traffic leaving through the tailnet, the address this computer
        // finds for itself is the tailnet one. It is written as it always
        // was, and described as what it is.
        let found = plan(&[], Some("100.101.102.103"), false, &says(CONNECTED)).expect("plans");
        assert_eq!(
            found.certificate,
            ["100.101.102.103", "studio.example.ts.net", "studio"]
        );
        assert_eq!(
            found.written,
            ["100.101.102.103"],
            "a second tailnet host would need Tailscale just the same"
        );
        let reach = found.reach.expect("says where it works");
        assert!(
            reach.contains("its tailnet address, 100.101.102.103"),
            "{reach}"
        );
        assert!(!reach.contains("this network only"), "{reach}");
    }

    #[test]
    fn with_no_address_of_its_own_the_reader_is_given_the_tailnet_name() {
        let found = plan(&[], None, false, &says(CONNECTED)).expect("plans");
        assert_eq!(
            found.certificate,
            ["studio.example.ts.net", "studio", "100.101.102.103"]
        );
        assert_eq!(found.written, ["studio.example.ts.net"]);
    }

    #[test]
    fn a_tailnet_without_a_name_is_written_second_by_its_address() {
        let status = CONNECTED.replace("studio.example.ts.net.", "");
        let found = plan(&[], Some("192.168.1.20"), false, &says(&status)).expect("plans");
        assert_eq!(found.written, ["192.168.1.20", "100.101.102.103"]);
    }
}
