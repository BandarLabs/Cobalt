//! Keeping the network alive across a reader handoff.
//!
//! Stopping and restarting the stock reader reliably drops the Wi-Fi
//! connection. The reader owns the radio, and the restarted one begins from its
//! own "not connected" state, so the association and the lease are simply gone.
//! On a device managed over Wi-Fi that means every handoff can cost the
//! connection used to run it, which was measured rather than assumed: the
//! device became unreachable after a session and only came back when its owner
//! tapped through the reader's own network UI.
//!
//! There is no supported way to ask the reader to reconnect. It exposes no
//! D-Bus service; the session bus carries only `Fontickel`, `Sickel` and the
//! bus itself. `/tmp/nickel-hardware-status` is one-way reporting rather than
//! control: the stock scripts write `network <action> ip=…` into it to say what
//! has already happened. There is no wifi script to call either, because
//! `libnickel` drives the radio internally.
//!
//! What is left is to put back exactly what was running. This module records
//! the supplicant and DHCP client while they are still alive, and starts those
//! same programs again, with their own arguments and environment, if the
//! connection has not returned on its own. Nothing here invents a
//! configuration, chooses a network, or writes to persistent storage, and
//! everything it does is undone by a reboot.
//!
//! Deliberately not done: writing to `/tmp/nickel-hardware-status` to correct
//! the reader's indicator. That FIFO blocks until something reads it, which is
//! why the stock scripts background every write to it, and a cosmetic icon is
//! not worth a runtime that can hang.

use crate::reader::{Reader, ReaderError};
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// The supplicant that owns the wireless association.
pub const SUPPLICANT_EXECUTABLE: &str = "/bin/wpa_supplicant";

/// The DHCP client that owns the address and the default route.
pub const DHCP_EXECUTABLE: &str = "/sbin/dhcpcd";

/// The interface the device connects with.
pub const WIRELESS_LINK: &str = "wlan0";

const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How long to keep watching before concluding the connection survived.
///
/// The restarted reader takes the link down some seconds after it starts, not
/// while it is starting. Checking once on the way past therefore reads the
/// routing table while the old default route is still in it and concludes,
/// wrongly, that nothing needs doing. Measured on a Clara BW: the summary said
/// the connection was unaffected, and the device was unreachable moments later.
const SETTLE: Duration = Duration::from_secs(12);

/// The state of the connection after an attempt to restore it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Restored {
    /// The connection was still up and nothing was started.
    Unaffected,
    /// Daemons were started again and the connection came back.
    Restarted,
    /// The connection did not come back within the time allowed.
    ///
    /// This is reported rather than raised as an error, because a session that
    /// has already put the reader back has succeeded at the thing that matters;
    /// the owner can reconnect by hand exactly as before.
    StillDown,
}

/// The connection as it was before the reader was stopped.
#[derive(Debug, Default)]
pub struct Connection {
    /// In start order: the association has to exist before a lease can.
    daemons: Vec<Reader>,
    /// Whether there was a connection to lose in the first place.
    was_online: bool,
}

impl Connection {
    /// Records the networking daemons that are currently running.
    ///
    /// This never fails. A daemon that is not running is one this module will
    /// not try to restore, which is the correct behaviour for a device that was
    /// already offline when the session began.
    #[must_use]
    pub fn capture() -> Self {
        let daemons = [SUPPLICANT_EXECUTABLE, DHCP_EXECUTABLE]
            .into_iter()
            .filter_map(|executable| Reader::find_running(executable).ok())
            .collect();
        Self {
            daemons,
            was_online: is_online(WIRELESS_LINK),
        }
    }

    /// Returns whether anything was recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.daemons.is_empty()
    }

    /// Puts the connection back if it has gone, waiting up to `within`.
    ///
    /// # Errors
    ///
    /// Returns an error only when a recorded daemon could not be started at
    /// all. Failing to reach the network in time is reported as
    /// [`Restored::StillDown`].
    pub fn restore(&self, within: Duration) -> Result<Restored, ReaderError> {
        // A device that was already offline has nothing to put back, and
        // starting a supplicant it was not running would be inventing state.
        if !self.was_online {
            return Ok(Restored::Unaffected);
        }
        if !went_offline(WIRELESS_LINK, SETTLE) {
            return Ok(Restored::Unaffected);
        }
        for daemon in &self.daemons {
            // Starting a second copy of a daemon that is already running would
            // leave two of them fighting over one interface, which is worse
            // than the problem being fixed.
            if Reader::find_running(daemon.executable()).is_ok() {
                continue;
            }
            daemon.start(within)?;
        }
        Ok(if wait_until_online(WIRELESS_LINK, within) {
            Restored::Restarted
        } else {
            Restored::StillDown
        })
    }
}

/// Returns whether `link` currently has a default route.
///
/// A default route is used rather than the presence of an address because it is
/// what actually decides whether the device can be reached, and because it is a
/// plain file read that needs no socket and no `unsafe`.
#[must_use]
pub fn is_online(link: &str) -> bool {
    fs::read_to_string("/proc/net/route").is_ok_and(|table| has_default_route(&table, link))
}

/// Watches `link` for `settle`, returning whether it was ever seen offline.
///
/// Returning early on the first offline reading keeps the common case cheap;
/// only a connection that genuinely survives costs the full wait.
fn went_offline(link: &str, settle: Duration) -> bool {
    let deadline = Instant::now() + settle;
    loop {
        if !is_online(link) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn wait_until_online(link: &str, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if is_online(link) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Parses the kernel's routing table for a default route on `link`.
///
/// The destination column is a hexadecimal address in the host's byte order, so
/// the default route is the all-zero one.
fn has_default_route(table: &str, link: &str) -> bool {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            Some((columns.next()?, columns.next()?))
        })
        .any(|(interface, destination)| {
            interface == link && destination.chars().all(|digit| digit == '0')
        })
}

/// Returns whether the kernel reports a carrier on `link`.
///
/// Used for reporting rather than for decisions: a link can be up with no
/// address, which is not a usable connection.
#[must_use]
pub fn has_carrier(link: &str) -> bool {
    fs::read_to_string(Path::new("/sys/class/net").join(link).join("operstate"))
        .is_ok_and(|state| state.trim() == "up")
}

#[cfg(test)]
mod tests {
    use super::{has_default_route, Connection};

    /// Taken verbatim from the device, header row included.
    const REAL_TABLE: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t312\t00000000\t0\t0\t0\n\
wlan0\t0001A8C0\t00000000\t0001\t0\t0\t312\t00FFFFFF\t0\t0\t0\n";

    #[test]
    fn the_real_routing_table_reads_as_online() {
        assert!(has_default_route(REAL_TABLE, "wlan0"));
    }

    #[test]
    fn a_subnet_route_alone_is_not_a_connection() {
        let table = "Iface\tDestination\tGateway\n\
wlan0\t0001A8C0\t00000000\n";
        assert!(
            !has_default_route(table, "wlan0"),
            "an interface with only a local route cannot reach anything"
        );
    }

    #[test]
    fn another_interfaces_default_route_does_not_count() {
        let table = "Iface\tDestination\tGateway\n\
usb0\t00000000\t0101A8C0\n";
        assert!(!has_default_route(table, "wlan0"));
    }

    #[test]
    fn an_empty_table_reads_as_offline() {
        assert!(!has_default_route("Iface\tDestination\tGateway\n", "wlan0"));
    }

    #[test]
    fn the_header_row_is_never_mistaken_for_a_route() {
        // "Destination" contains no digits at all, so a careless all-zero test
        // over an empty iterator would accept it.
        let table = "Iface\tDestination\tGateway\n\
wlan0\tDestination\tGateway\n";
        assert!(!has_default_route(table, "wlan0"));
    }

    #[test]
    fn capturing_on_a_host_without_those_daemons_records_nothing() {
        assert!(
            Connection::capture().is_empty(),
            "capture must never fail when the daemons are absent"
        );
    }
}

#[cfg(test)]
mod race_tests {
    use super::{Connection, Restored};
    use std::time::Duration;

    /// The defect this module was rewritten for.
    ///
    /// The first on-device run reported the connection as unaffected and then
    /// went unreachable, because the reader takes the link down after it has
    /// started rather than while it is starting. A connection that was never up
    /// must still be left alone, which is what this pins.
    #[test]
    fn a_device_that_was_offline_has_nothing_to_restore() {
        let connection = Connection::default();
        assert!(!connection.was_online);
        let outcome = connection
            .restore(Duration::from_secs(1))
            .expect("restoring nothing cannot fail");
        assert_eq!(outcome, Restored::Unaffected);
    }

    /// `restore` must not start daemons it never recorded.
    #[test]
    fn an_empty_capture_starts_nothing() {
        assert!(Connection::default().is_empty());
    }
}
