//! Finding a device on the network, and installing onto it over Wi-Fi.
//!
//! A Kobo is an awkward thing to reach. It powers Wi-Fi down when it sleeps,
//! it takes a new address from DHCP every time it comes back, and it never
//! says any of that out loud, the symptom is always the same connection
//! timeout. Most of the time lost to this project has been lost to guessing
//! which of those is happening.
//!
//! So the two commands built on this module are deliberately unglamorous.
//! `kobo devices` sweeps the local network and names what it finds, which
//! answers "what is its address now". [`OFFLINE_HELP`] is printed by every
//! command that fails to reach a device, which answers "why can I not reach
//! it". Neither needs an argument, because someone who knew the argument would
//! not be reading either message.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Where an installed Cobalt lives, as an absolute path on the device.
///
/// The archive stores the same location as a relative path so it can be
/// extracted from `/`; this is that path as the device sees it.
pub const INSTALL_DIRECTORY: &str = "/mnt/onboard/.adds/cobalt";

/// The port a device answers on, and the only one this tool ever knocks on.
pub const SSH_PORT: u16 = 22;

/// How long a single address is given to accept a connection during a sweep.
///
/// A device on the same wireless network answers in a few milliseconds. This
/// is far longer than that, because a sleeping radio can take a moment to get
/// a packet out, and short enough that a whole /24 finishes in seconds.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(600);

/// How long the second look at a silent address is given.
///
/// A reader's radio spends its idle time in Wi-Fi power save, where the
/// access point holds a packet until the radio's next scheduled wake. When
/// that hold outlives the first timeout, the kernel does not send the opening
/// packet again for about a second, so the first probe can pass a healthy,
/// awake reader over. The retry has to be long enough to cover that
/// retransmission, and it costs nothing on the empty addresses: those fail
/// fast in the first round and are the only ones retried in bulk.
pub const PROBE_RETRY_TIMEOUT: Duration = Duration::from_millis(2500);

/// How many addresses are probed at once.
///
/// A sweep is entirely waiting, so the limit is file descriptors rather than
/// processors. Sixty-four keeps a /24 to four rounds of the two timeouts
/// above.
pub const PROBE_CONCURRENCY: usize = 64;

/// What to try when a device does not answer.
///
/// Printed verbatim by every command that fails to reach one. The order is the
/// order the causes actually occur in: the radio is off far more often than
/// anything is wrong.
pub const OFFLINE_HELP: &str = "\
The device did not answer. In order of how often each one is the cause:

  1. The reader is asleep, so its Wi-Fi is off. Press the power button, wait
     for the home screen, and try again. Nothing on a Kobo keeps Wi-Fi up
     through sleep by default.
  2. Wi-Fi is off while awake. On the reader: the top bar, then the Wi-Fi
     icon, then join the network. Toggling airplane mode on and back off
     reconnects it faster than anything else when it is being stubborn.
  3. Its address changed. A Kobo takes a new one from DHCP on every
     reconnection, so the address that worked yesterday is often somebody
     else's today. Run 'kobo devices' to find it.
  4. SSH is not running on it. Cobalt does not install an SSH server, but the
     firmware ships one, switched off. Connect the reader by USB and run
     'kobo setup': it enables that server, installs Cobalt, and ejects. The
     server starts at the next restart, so restart the reader afterwards.

Once it answers, 'kobo session --device <ip> --wifi-always-on on' stops the
reader powering the radio down while you work, and 'kobo session --device <ip>
--keep-awake on' stops it suspending. Both are reversible and both clear on a
reboot.";

/// Guesses the /24 this machine is on, as the first three octets.
///
/// Opening a UDP socket and asking it for its own address is the shortest way
/// to learn which interface has a route out, and sends nothing: a connected UDP
/// socket only records the peer. A machine with no route at all has no subnet
/// to sweep, which is itself the answer.
#[must_use]
pub fn local_subnet() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let IpAddr::V4(address) = socket.local_addr().ok()?.ip() else {
        return None;
    };
    let [a, b, c, _] = address.octets();
    Some(format!("{a}.{b}.{c}"))
}

/// True when `subnet` is three dotted decimal octets and nothing else.
///
/// The sweep builds addresses by appending a host part, so anything else here
/// would produce addresses that were never asked for.
#[must_use]
pub fn valid_subnet(subnet: &str) -> bool {
    let mut octets = 0;
    for part in subnet.split('.') {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        if part.parse::<u8>().is_err() {
            return false;
        }
        octets += 1;
    }
    octets == 3
}

/// Every address in `subnet` that accepts a connection on [`SSH_PORT`].
///
/// Only a completed TCP handshake counts, and the connection is dropped
/// immediately without a byte being sent, so this cannot disturb whatever is
/// listening. Addresses are returned in numerical order however the threads
/// happen to finish.
#[must_use]
pub fn sweep(subnet: &str, timeout: Duration) -> Vec<Ipv4Addr> {
    let mut found = Vec::new();
    let hosts: Vec<u8> = (1..=254).collect();
    for chunk in hosts.chunks(PROBE_CONCURRENCY) {
        let (sender, receiver) = mpsc::channel();
        let mut workers = Vec::new();
        for host in chunk {
            let Ok(address) = format!("{subnet}.{host}").parse::<Ipv4Addr>() else {
                continue;
            };
            let sender = sender.clone();
            workers.push(thread::spawn(move || {
                if answers(address, timeout) {
                    let _ = sender.send(address);
                }
            }));
        }
        drop(sender);
        found.extend(receiver.iter());
        for worker in workers {
            let _ = worker.join();
        }
    }
    found.sort_unstable();
    found
}

/// True when `address` completes a TCP handshake on [`SSH_PORT`] within
/// `timeout`, looking twice at an address that stays silent.
///
/// A refusal comes back in milliseconds and is final: a machine is there with
/// nothing on this port. Silence is not final. A reader whose radio is dozing
/// in power save misses the opening packet, and the second look waits out the
/// kernel's retransmission of it (see [`PROBE_RETRY_TIMEOUT`]). Truly empty
/// addresses stay silent both times; they cost the retry and nothing else.
fn answers(address: Ipv4Addr, timeout: Duration) -> bool {
    match attempt(address, timeout) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            attempt(address, PROBE_RETRY_TIMEOUT).is_ok()
        }
        Err(_) => false,
    }
}

/// One connection attempt on [`SSH_PORT`], dropped as soon as it completes.
fn attempt(address: Ipv4Addr, timeout: Duration) -> std::io::Result<()> {
    let stream = TcpStream::connect_timeout(&SocketAddr::from((address, SSH_PORT)), timeout)?;
    let _ = stream.shutdown(std::net::Shutdown::Both);
    Ok(())
}

/// A script that prints one `key=value` line per fact identifying a device.
///
/// Reads only. A machine that is not a Kobo prints empty values rather than
/// failing, because the point is to tell them apart, not to refuse one.
#[must_use]
pub fn identity_script() -> String {
    format!(
        "version=$(cat /mnt/onboard/.kobo/version 2>/dev/null || true)\n\
         echo \"serial=${{version%%,*}}\"\n\
         echo \"firmware=$(echo \"$version\" | cut -d, -f3)\"\n\
         echo \"model=$(cat /sys/devices/soc0/machine 2>/dev/null || true)\"\n\
         if [ -d '{INSTALL_DIRECTORY}' ]; then\n\
           echo \"cobalt=$(cat '{INSTALL_DIRECTORY}/VERSION' 2>/dev/null || echo present)\"\n\
         else\n\
           echo 'cobalt='\n\
         fi\n\
         exit\n"
    )
}

/// One identified device, as far as a read-only look can tell.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Identity {
    /// Full serial, whose first four characters are the model code.
    pub serial: String,
    /// Firmware version string.
    pub firmware: String,
    /// Installed Cobalt version, empty when nothing is installed.
    pub cobalt: String,
}

impl Identity {
    /// Reads an [`identity_script`] answer.
    ///
    /// Unknown keys are ignored so the script can gain lines without this
    /// having to be changed in step.
    #[must_use]
    pub fn parse(output: &str) -> Self {
        let mut identity = Self::default();
        for line in output.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().to_owned();
            match key.trim() {
                "serial" => identity.serial = value,
                "firmware" => identity.firmware = value,
                "cobalt" => identity.cobalt = value,
                _ => {}
            }
        }
        identity
    }

    /// True when this answer came from something that is recognisably a Kobo.
    ///
    /// Every Kobo serial begins with an `N` followed by three digits, and no
    /// other machine on a home network has a `/mnt/onboard/.kobo/version` at
    /// all, so the presence of the file is most of the evidence.
    #[must_use]
    pub fn is_kobo(&self) -> bool {
        let bytes = self.serial.as_bytes();
        bytes.len() >= 4 && bytes[0] == b'N' && bytes[1..4].iter().all(u8::is_ascii_digit)
    }

    /// The four-character model code, which is what a device profile matches.
    #[must_use]
    pub fn model_code(&self) -> &str {
        self.serial.get(..4).unwrap_or_default()
    }

    /// A one-line description for a list of found devices.
    #[must_use]
    pub fn summary(&self) -> String {
        let firmware = if self.firmware.is_empty() {
            "unknown firmware".to_owned()
        } else {
            format!("firmware {}", self.firmware)
        };
        let cobalt = if self.cobalt.is_empty() {
            "Cobalt not installed".to_owned()
        } else {
            format!("Cobalt {}", self.cobalt.trim())
        };
        format!("{} · {firmware} · {cobalt}", self.model_code())
    }
}

/// A script that installs an uploaded archive into place, without a reboot.
///
/// The archive is written to `/tmp`, checked against the checksum of the bytes
/// this machine sent, checked again by gzip, and only then extracted. Every
/// path it contains is listed first and the extraction is refused outright if
/// any of them fall outside [`INSTALL_DIRECTORY`], the same rule the packager
/// applies when building, applied again on the device, because this one runs
/// as root.
///
/// A running panel session is refused rather than worked around: the files
/// being replaced are the ones it is executing.
#[must_use]
pub fn install_script(encoded_archive: &str, checksum: &str) -> String {
    let root = INSTALL_DIRECTORY.trim_start_matches('/');
    format!(
        "set -eu\n\
         umask 022\n\
         archive=/tmp/kobo-deploy.tgz\n\
         listing=/tmp/kobo-deploy.list\n\
         cleanup() {{ rm -f \"$archive\" \"$listing\"; }}\n\
         trap cleanup EXIT HUP INT TERM\n\
         if [ -n \"$(ps | grep '[k]obod' || true)\" ]; then\n\
           echo 'a Cobalt session is running on the device; exit it first' >&2\n\
           exit 1\n\
         fi\n\
         base64 -d > \"$archive\" <<'KOBO_PACKAGE_BASE64'\n\
         {encoded_archive}\n\
         KOBO_PACKAGE_BASE64\n\
         set -- $(sha256sum \"$archive\")\n\
         if [ \"$1\" != '{checksum}' ]; then\n\
           echo 'uploaded package checksum does not match' >&2\n\
           exit 1\n\
         fi\n\
         gzip -t \"$archive\"\n\
         tar ztf \"$archive\" > \"$listing\"\n\
         while IFS= read -r path; do\n\
           case \"$path\" in\n\
             /*|../*|*/../*|*/..|./*|*/./*|*/.|*//*) echo \"unsafe package path: $path\" >&2; exit 1 ;;\n\
             {root}/*) ;;\n\
             mnt/|mnt/onboard/|mnt/onboard/.adds/|{root}/) ;;\n\
             *) echo \"package would write outside {INSTALL_DIRECTORY}: $path\" >&2; exit 1 ;;\n\
           esac\n\
         done < \"$listing\"\n\
         tar zxf \"$archive\" -C /\n\
         sync\n\
         echo \"installed=$(cat '{INSTALL_DIRECTORY}/VERSION' 2>/dev/null || echo unknown)\"\n\
         echo \"binaries=$(ls '{INSTALL_DIRECTORY}/bin' | wc -l)\"\n\
         exit\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        install_script, valid_subnet, Identity, INSTALL_DIRECTORY, OFFLINE_HELP, PROBE_CONCURRENCY,
    };

    #[test]
    fn a_subnet_is_three_octets_and_nothing_else() {
        assert!(valid_subnet("192.168.1"));
        assert!(valid_subnet("10.0.0"));
        assert!(
            !valid_subnet("192.168.1.10"),
            "that is a host, not a subnet"
        );
        assert!(!valid_subnet("192.168"));
        assert!(!valid_subnet("192.168.300"));
        assert!(!valid_subnet("192.168.one"));
        assert!(!valid_subnet(""));
        assert!(!valid_subnet("192.168.1;reboot"));
    }

    #[test]
    fn an_identity_answer_is_read_key_by_key() {
        let identity = Identity::parse(
            "serial=N365410043013\nfirmware=4.45.23697\nmodel=\ncobalt=0.1.0\nnoise\n",
        );
        assert_eq!(identity.serial, "N365410043013");
        assert_eq!(identity.model_code(), "N365");
        assert_eq!(identity.firmware, "4.45.23697");
        assert!(identity.is_kobo());
        assert!(identity.summary().contains("Cobalt 0.1.0"));
    }

    #[test]
    fn a_machine_that_is_not_a_reader_is_not_reported_as_one() {
        let identity = Identity::parse("serial=\nfirmware=\ncobalt=\n");
        assert!(!identity.is_kobo());
        assert!(identity.summary().contains("Cobalt not installed"));
    }

    /// The whole point of the command is to be readable by somebody who has
    /// just been told nothing answered, so the four causes have to be in it.
    #[test]
    fn the_offline_help_names_every_cause_in_order() {
        let sleep = OFFLINE_HELP.find("asleep").expect("sleep is first");
        let wifi = OFFLINE_HELP.find("airplane mode").expect("wifi is second");
        let address = OFFLINE_HELP.find("kobo devices").expect("address is third");
        let ssh = OFFLINE_HELP.find("kobo setup").expect("ssh is last");
        assert!(sleep < wifi && wifi < address && address < ssh);
    }

    #[test]
    fn the_install_script_refuses_a_path_outside_the_install_directory() {
        let script = install_script("QQ==", "abc");
        assert!(script.contains("mnt/onboard/.adds/cobalt/*)"));
        assert!(script.contains(&format!("would write outside {INSTALL_DIRECTORY}")));
    }

    /// Overwriting the binaries a running session is executing is the one way
    /// this command could leave a device in a state a reboot has to fix.
    #[test]
    fn the_install_script_refuses_while_a_session_is_running() {
        let script = install_script("QQ==", "abc");
        let refusal = script
            .find("a Cobalt session is running")
            .expect("refuses a running session");
        let extract = script.find("tar zxf").expect("extracts");
        assert!(refusal < extract, "the check has to come first");
    }

    #[test]
    fn the_install_script_verifies_before_it_extracts() {
        let script = install_script("QQ==", "deadbeef");
        let checksum = script.find("sha256sum").expect("checksums");
        let gzip = script.find("gzip -t").expect("tests the archive");
        let extract = script.find("tar zxf").expect("extracts");
        assert!(checksum < gzip && gzip < extract);
    }

    #[test]
    fn a_sweep_round_is_bounded() {
        const { assert!(PROBE_CONCURRENCY > 0 && PROBE_CONCURRENCY <= 128) };
    }
}
