#![forbid(unsafe_code)]

//! Which of a paired computer's hosts an app dials, and when it tries the
//! other.
//!
//! A computer on a tailnet answers at two hosts: its address on the network
//! it is on, which the reader reaches at home on the same Wi-Fi, and its
//! tailnet name, which the reader reaches from anywhere while Tailscale is
//! on. Neither works everywhere. Each pairing used to hold one host, so the
//! owner chose where the pairing would work and it failed in the other
//! place.
//!
//! Every app that talks to a paired computer keeps its pairing in a format
//! of its own, and every one of those parsers reads the code as everything
//! after the address. A second host added to that record would become part
//! of the code on a reader that has not been updated, and the computer would
//! refuse it. So the record is left exactly as it was, and the hosts live
//! under a key of their own, [`HOSTS_KEY`], beside it. An app that predates
//! this never loads that key and pairs as it always did with the first host.
//!
//! The hosts record names the paired host itself, so it is only believed
//! while it does. A pairing typed on the reader for some other computer
//! leaves the record behind, and an app reading both would otherwise dial a
//! computer it was never paired with.
//!
//! The app tries the host that last answered first, and the other only when
//! the first is not there. "Not there" is [`TaskError::Unreachable`] and
//! nothing else; see [`not_there`]. The host that answers becomes the first
//! one next time, and the app saves the new order with [`Hosts::answered`],
//! so a reader that went away from home stops trying its home address
//! first.

use kobo_protocol::TaskError;

/// The store key, beside each app's own pairing, that lists every host the
/// paired computer answers at, one per line, the one to try first at the
/// top.
pub const HOSTS_KEY: &str = "hosts";

/// The most hosts one pairing holds.
///
/// Two places, two hosts. A third would mean a third full connection
/// attempt before an app could say the computer is not answering.
pub const MOST_HOSTS: usize = 2;

/// The hosts record for `hosts`, in the order given.
#[must_use]
pub fn record<S: AsRef<str>>(hosts: &[S]) -> String {
    hosts
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `error` says the host is not there, so the other host is worth
/// trying.
///
/// Only [`TaskError::Unreachable`] says that: a name that does not resolve,
/// a connection refused or never answered, or a host that would not finish
/// a secure connection. Every other failure is either about the reader or an
/// answer from the computer itself, and the other host would give the same
/// one:
///
/// * [`TaskError::Offline`] is the reader without a network, which no host
///   can fix.
/// * [`TaskError::TimedOut`] is a computer that took a connection and was
///   slow to answer. It is there, and asking it again somewhere else would
///   make the owner wait twice for the same slow computer.
/// * [`TaskError::Unauthorized`] and [`TaskError::NotFound`] are the paired
///   computer refusing the code or the request. Both hosts lead to the same
///   computer, which would refuse it again.
///
/// The runtime reports a refused certificate as `Unreachable` too, and an
/// app cannot tell it apart. Trying the other host after one is safe: that
/// host has to prove itself against the same trusted root before the code
/// is sent, and when it cannot, the app says what it said before.
#[must_use]
pub const fn not_there(error: TaskError) -> bool {
    matches!(error, TaskError::Unreachable)
}

/// The hosts one pairing answers at, in the order they are tried.
///
/// Empty until a pairing is known. Each round of dialing starts with the
/// first host and moves to the second only when the first is not there.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hosts {
    order: Vec<String>,
    /// Which host this round is trying: 0, or 1 once the first was not
    /// there.
    trying: usize,
}

impl Hosts {
    /// A pairing with one host, which is every pairing typed on the reader
    /// and every one written before there were two.
    #[must_use]
    pub fn one(host: impl Into<String>) -> Self {
        Self {
            order: vec![host.into()],
            trying: 0,
        }
    }

    /// The pairing's hosts, from the host its own record holds and the
    /// hosts record saved beside it, if there is one.
    ///
    /// `read` is the app's own reading of a typed address, so a host from
    /// the record is held to the same rules, and written the same way, as
    /// one the owner typed. A line it refuses is passed over.
    ///
    /// The record is believed only when it names `paired`. When it does not,
    /// it belongs to an earlier pairing and the app dials `paired` alone.
    #[must_use]
    pub fn paired(
        paired: &str,
        record: Option<&[u8]>,
        read: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let mut listed: Vec<String> = Vec::new();
        for line in record
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or_default()
            .lines()
        {
            if let Some(host) = read(line.trim()) {
                if !listed.contains(&host) {
                    listed.push(host);
                }
            }
        }
        if !listed.iter().any(|host| host == paired) {
            return Self::one(paired);
        }
        // The paired host and the first other one, in the record's order.
        let mut kept = false;
        listed.retain(|host| {
            if host == paired {
                return true;
            }
            !std::mem::replace(&mut kept, true)
        });
        Self {
            order: listed,
            trying: 0,
        }
    }

    /// The host to dial now. Empty before a pairing is known.
    #[must_use]
    pub fn current(&self) -> &str {
        self.order
            .get(self.trying)
            .or_else(|| self.order.first())
            .map_or("", String::as_str)
    }

    /// Every host, in the order a round tries them.
    #[must_use]
    pub fn all(&self) -> &[String] {
        &self.order
    }

    /// Whether this round still has another host to go to if the current
    /// one is not there.
    #[must_use]
    pub fn other_left(&self) -> bool {
        self.trying + 1 < self.order.len()
    }

    /// Takes a failure from the current host. Returns whether to send the
    /// same request again now, to [`Self::current`], which is then the other
    /// host.
    ///
    /// A failure that does not say the host is gone, or a second host that is
    /// not there either, ends the round, and the app reports the failure as
    /// it always did. The next round starts again with the first host.
    pub fn failed(&mut self, error: TaskError) -> bool {
        if not_there(error) && self.other_left() {
            self.trying += 1;
            return true;
        }
        self.trying = 0;
        false
    }

    /// Ends this round with something other than a failure, such as an
    /// answer the app could not use: the host was there, so the other is not
    /// tried, and the next round starts with the first host again.
    pub fn end_round(&mut self) {
        self.trying = 0;
    }

    /// Takes an answer from the current host. Returns the hosts record to
    /// save when that host was not the first, which it now is.
    ///
    /// The host that answered last is the likelier one to answer next: a
    /// reader that has gone out stays out for a while, and one that has come
    /// home stays home.
    pub fn answered(&mut self) -> Option<String> {
        let trying = std::mem::take(&mut self.trying);
        if trying == 0 || trying >= self.order.len() {
            return None;
        }
        let host = self.order.remove(trying);
        self.order.insert(0, host);
        Some(record(&self.order))
    }
}

#[cfg(test)]
mod tests {
    use super::{not_there, record, Hosts, TaskError};

    /// Stands in for an app's own address reading: lowercase, a default
    /// port, and nothing with a space in it.
    fn read(text: &str) -> Option<String> {
        if text.is_empty() || text.contains(' ') {
            return None;
        }
        let text = text.to_ascii_lowercase();
        Some(if text.contains(':') {
            text
        } else {
            format!("{text}:9331")
        })
    }

    const HOME: &str = "192.168.1.20:9331";
    const AWAY: &str = "studio.example.ts.net:9331";

    fn both() -> Hosts {
        Hosts::paired(HOME, Some(format!("{HOME}\n{AWAY}").as_bytes()), read)
    }

    #[test]
    fn a_pairing_without_a_hosts_record_dials_its_one_host_as_it_always_did() {
        let mut hosts = Hosts::paired(HOME, None, read);
        assert_eq!(hosts, Hosts::one(HOME));
        assert_eq!(hosts.current(), HOME);
        assert!(!hosts.other_left());
        assert!(!hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.current(), HOME);
        assert_eq!(hosts.answered(), None);
    }

    #[test]
    fn a_hosts_record_that_names_the_paired_host_adds_the_other_in_its_order() {
        let hosts = both();
        assert_eq!(hosts.all(), [HOME, AWAY]);
        assert_eq!(hosts.current(), HOME);
        let away_first = Hosts::paired(HOME, Some(format!("{AWAY}\n{HOME}\n").as_bytes()), read);
        assert_eq!(away_first.all(), [AWAY, HOME]);
        assert_eq!(away_first.current(), AWAY);
    }

    #[test]
    fn a_hosts_record_left_from_another_computer_is_not_believed() {
        let other = b"10.0.0.4:9331\nother.example.ts.net:9331";
        assert_eq!(Hosts::paired(HOME, Some(other), read), Hosts::one(HOME));
    }

    #[test]
    fn hosts_are_read_the_way_the_app_reads_a_typed_address() {
        let record = b"  192.168.1.20  \nnot an address\n\nStudio.Example.TS.net\n192.168.1.20:9331\nthird.example:9331";
        let hosts = Hosts::paired(HOME, Some(record), read);
        assert_eq!(hosts.all(), [HOME, AWAY]);
        assert_eq!(
            Hosts::paired(HOME, Some(&[0xff, 0xfe]), read),
            Hosts::one(HOME)
        );
        assert_eq!(Hosts::paired(HOME, Some(b""), read), Hosts::one(HOME));
    }

    #[test]
    fn only_a_host_that_is_not_there_sends_the_request_to_the_other() {
        for error in [
            TaskError::Offline,
            TaskError::TimedOut,
            TaskError::Unauthorized,
            TaskError::NotFound,
            TaskError::Denied,
            TaskError::NoCredential,
            TaskError::TooLarge,
            TaskError::RateLimited(30),
        ] {
            assert!(!not_there(error), "{error:?}");
            let mut hosts = both();
            assert!(!hosts.failed(error), "{error:?}");
            assert_eq!(hosts.current(), HOME, "{error:?}");
        }
        assert!(not_there(TaskError::Unreachable));
        let mut hosts = both();
        assert!(hosts.other_left());
        assert!(hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.current(), AWAY);
        assert!(!hosts.other_left());
    }

    #[test]
    fn a_round_where_neither_host_is_there_ends_and_the_next_starts_with_the_first() {
        let mut hosts = both();
        assert!(hosts.failed(TaskError::Unreachable));
        assert!(!hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.current(), HOME);
        assert_eq!(
            hosts.all(),
            [HOME, AWAY],
            "nothing answered, so nothing moved"
        );
    }

    #[test]
    fn a_refusal_from_the_second_host_ends_the_round_without_going_back() {
        let mut hosts = both();
        assert!(hosts.failed(TaskError::Unreachable));
        assert!(!hosts.failed(TaskError::Unauthorized));
        assert_eq!(hosts.current(), HOME);
    }

    #[test]
    fn an_answer_the_app_could_not_use_ends_the_round_without_moving_anything() {
        let mut hosts = both();
        assert!(hosts.failed(TaskError::Unreachable));
        hosts.end_round();
        assert_eq!(hosts.current(), HOME);
        assert_eq!(hosts.all(), [HOME, AWAY]);
        assert!(hosts.other_left());
    }

    #[test]
    fn the_host_that_answers_is_tried_first_from_then_on() {
        let mut hosts = both();
        assert_eq!(
            hosts.answered(),
            None,
            "the first answered, nothing to save"
        );
        assert!(hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.answered(), Some(format!("{AWAY}\n{HOME}")));
        assert_eq!(hosts.current(), AWAY);
        assert_eq!(hosts.all(), [AWAY, HOME]);
        // Back home with Tailscale off: the tailnet name is not there.
        assert!(hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.current(), HOME);
        assert_eq!(hosts.answered(), Some(format!("{HOME}\n{AWAY}")));
    }

    #[test]
    fn the_saved_order_reads_back_as_the_same_hosts() {
        let mut hosts = both();
        assert!(hosts.failed(TaskError::Unreachable));
        let saved = hosts.answered().expect("moved");
        let again = Hosts::paired(HOME, Some(saved.as_bytes()), read);
        assert_eq!(again.all(), [AWAY, HOME]);
        assert_eq!(record(&[HOME, AWAY]), format!("{HOME}\n{AWAY}"));
    }

    #[test]
    fn an_unpaired_app_has_no_host_to_dial() {
        let mut hosts = Hosts::default();
        assert_eq!(hosts.current(), "");
        assert!(!hosts.failed(TaskError::Unreachable));
        assert_eq!(hosts.answered(), None);
    }
}
