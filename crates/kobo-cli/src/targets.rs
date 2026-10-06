//! One answer to "act on what?": the simulator on this computer, a reader at
//! a network address, or a reader saved under a name.
//!
//! Every command that asks parses through [`TargetArgs`], so the flags,
//! their spelling and their errors stay identical no matter which verb is in
//! front of them. A command that can only work on one kind of target says so
//! through the same resolver rather than growing its own dialect.

/// What a command acts on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// The simulator on this computer.
    Simulator,
    /// A reader at this network address, named for this invocation only.
    Address(String),
    /// A reader saved under this name. The name becomes an address through
    /// [`resolve_nickname`], never through a silent first-reader guess.
    Nickname(String),
}

/// The target flags found on one command line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetArgs {
    simulator: bool,
    device: Option<String>,
    reader: Option<String>,
}

/// Every spelling of the target flags, for usage lines: `--sim` for the
/// simulator, `--device HOST` (or `-s HOST`, the spelling adb hands already
/// know) for a reader by address, `--reader NAME` for a saved reader.
pub const TARGET_FLAGS: &str = "--sim | --device HOST | --reader NAME";

impl TargetArgs {
    /// Pulls the target flags out of `arguments`, wherever they appear, and
    /// returns them beside the arguments that are left.
    ///
    /// An unknown argument is not an error here; it belongs to the command,
    /// whose own parser refuses what it does not know. A repeated flag is
    /// left in the rest as well, where the command's usage line names it as
    /// the mistake it is.
    pub fn parse(arguments: &[String]) -> Result<(Self, Vec<String>), String> {
        let mut target = Self::default();
        let mut rest = Vec::new();
        let mut arguments = arguments.iter();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--sim" if !target.simulator => target.simulator = true,
                "--device" | "-s" if target.device.is_none() => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| crate::console::usage("--device takes a host"))?;
                    target.device = Some(value.clone());
                }
                "--reader" if target.reader.is_none() => {
                    let value = arguments
                        .next()
                        .ok_or_else(|| crate::console::usage("--reader takes a name"))?;
                    target.reader = Some(value.clone());
                }
                _ => rest.push(argument.clone()),
            }
        }
        Ok((target, rest))
    }

    /// Whether any target flag was given at all.
    pub fn is_empty(&self) -> bool {
        !self.simulator && self.device.is_none() && self.reader.is_none()
    }

    /// The one target the flags name.
    ///
    /// None and several are both usage mistakes. Picking a reader silently
    /// is how content lands on the wrong one, and a command line that names
    /// two targets is a misunderstanding to send back, not to arbitrate.
    pub fn resolve(&self) -> Result<Target, String> {
        match (&self.simulator, &self.device, &self.reader) {
            (true, None, None) => Ok(Target::Simulator),
            (false, Some(host), None) => Ok(Target::Address(host.clone())),
            (false, None, Some(name)) => Ok(Target::Nickname(name.clone())),
            (false, None, None) => Err(crate::console::usage(format!(
                "choose a target ({TARGET_FLAGS})"
            ))),
            _ => Err(crate::console::usage(format!(
                "choose one target, not several ({TARGET_FLAGS})"
            ))),
        }
    }
}

/// Turns a saved reader's name into its network address.
///
/// Resolution goes through the saved-reader store and accepts an address
/// only when the serial behind it is the saved one - an address is a lease,
/// not an identity, and the first reader to answer is never a substitute for
/// the one that was named. A miss is a target error the owner can act on.
pub fn resolve_nickname(name: &str) -> Result<String, String> {
    let path = crate::readers::store_path();
    let mut store = crate::readers::Store::load(&path)?;
    let address = crate::readers::resolve_saved(&mut store, name, probe_serial, sweep_serials)?;
    // A re-identified address is worth keeping; a store that cannot be
    // written never blocks a reader that was just found.
    let _ = store.save(&path);
    Ok(address)
}

/// The serial answering at one address, when a Kobo answers at all.
pub(crate) fn probe_serial(address: &str) -> Option<String> {
    crate::identify_device(address)
        .filter(crate::connect::Identity::is_kobo)
        .map(|identity| identity.serial)
}

/// Every Kobo answering on this computer's network, beside its address.
fn sweep_serials() -> Vec<(String, String)> {
    let Some(subnet) = crate::connect::local_subnet() else {
        return Vec::new();
    };
    crate::connect::sweep(&subnet, crate::connect::PROBE_TIMEOUT)
        .iter()
        .filter_map(|address| {
            let host = address.to_string();
            probe_serial(&host).map(|serial| (host, serial))
        })
        .collect()
}

/// The companion commands that act on a reader, by subcommand.
///
/// An empty list means the command itself acts on a reader whatever follows
/// it. Anything not listed is left exactly as it was typed: a preview, an
/// inspection or a status check has no reader to name.
const ACTS_ON_A_READER: &[(&str, &[&str])] = &[
    ("frame", &["init", "push", "ls", "rm"]),
    ("musicstand", &["init", "push", "plan", "ls", "rm"]),
    ("vault", &["init", "push", "plan", "ingest", "ls", "rm"]),
    ("fieldbook", &["push", "ls", "export"]),
    ("needles", &["push"]),
    ("nonograms", &["push"]),
    ("parser", &["push"]),
    ("panels", &["push"]),
    ("post", &["login"]),
    ("readlater", &["login"]),
    ("birds", &["listen", "push"]),
    ("feeds", &["push"]),
    ("deck", &["push"]),
    ("secret", &["set", "list", "remove"]),
    ("trust", &["set", "list", "remove"]),
    ("sync", &["setup"]),
    ("export", &[]),
];

/// Flags that already say where a command's result goes.
const TARGETS: &[&str] = &[
    "--sim",
    "--device",
    "-s",
    "--reader",
    "--out",
    "--volume",
    "--kobo-root",
];

/// The environment variable that names the reader to use when none is given.
pub const READER_VARIABLE: &str = "KOBO_READER";

/// Where a command with no target goes, if anywhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DefaultReader {
    /// `KOBO_READER` named an address.
    Address(String),
    /// `KOBO_READER` named a saved reader, or exactly one reader is saved.
    Nickname(String),
}

/// The reader a command with no target acts on.
///
/// `KOBO_READER` first, then the one reader this computer has saved. Never a
/// reader found on the network: a shared network can hold a neighbour's Kobo,
/// and content sent to the wrong one is not a mistake anyone can see. Two
/// saved readers and no variable is a question only the owner can answer.
#[must_use]
pub fn default_reader(variable: Option<&str>, saved: &[&str]) -> Option<DefaultReader> {
    if let Some(value) = variable.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(if value.contains('.') || value.contains(':') {
            DefaultReader::Address(value.to_owned())
        } else {
            DefaultReader::Nickname(value.to_owned())
        });
    }
    match saved {
        [only] => Some(DefaultReader::Nickname((*only).to_owned())),
        _ => None,
    }
}

/// Whether this command line acts on a reader.
fn acts_on_a_reader(arguments: &[String]) -> bool {
    let Some(command) = arguments.first() else {
        return false;
    };
    ACTS_ON_A_READER.iter().any(|(name, subcommands)| {
        name == command
            && (subcommands.is_empty()
                || arguments
                    .get(1)
                    .is_some_and(|sub| subcommands.contains(&sub.as_str())))
    })
}

/// The command line with its reader filled in, so every companion command
/// accepts `--reader NAME` and needs no target at all once a reader is saved.
///
/// Apps used to tell their owners to run `kobo frame push PHOTOS --device IP`
/// and nothing on the reader says what its IP is. With a reader saved by
/// `kobo pair`, the same push is `kobo frame push PHOTOS`. A command that
/// names a target keeps it; `--reader NAME` becomes the address that reader
/// answers at, because most companion commands only read `--device`.
///
/// `resolve` turns a saved name into an address; `said` reports what was
/// chosen, so the owner always sees which reader a command went to.
pub fn with_reader(
    arguments: &[String],
    default: Option<DefaultReader>,
    mut resolve: impl FnMut(&str) -> Result<String, String>,
    mut said: impl FnMut(String),
) -> Result<Vec<String>, String> {
    if !acts_on_a_reader(arguments) {
        return Ok(arguments.to_vec());
    }
    if let Some(position) = arguments.iter().position(|argument| argument == "--reader") {
        let Some(name) = arguments.get(position + 1) else {
            return Err(crate::console::usage("--reader takes a name"));
        };
        let address = resolve(name)?;
        let mut rewritten = arguments.to_vec();
        rewritten.splice(position..=position + 1, ["--device".to_owned(), address]);
        return Ok(rewritten);
    }
    if arguments
        .iter()
        .any(|argument| TARGETS.contains(&argument.as_str()))
    {
        return Ok(arguments.to_vec());
    }
    let address = match default {
        Some(DefaultReader::Address(address)) => {
            said(format!(
                "Using the reader at {address} ({READER_VARIABLE})."
            ));
            address
        }
        Some(DefaultReader::Nickname(name)) => {
            let address = resolve(&name)?;
            said(format!("Using your saved reader \"{name}\" at {address}."));
            address
        }
        None => return Ok(arguments.to_vec()),
    };
    let mut rewritten = arguments.to_vec();
    rewritten.push("--device".to_owned());
    rewritten.push(address);
    Ok(rewritten)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn target_flags_parse_wherever_they_appear() {
        let (target, rest) = TargetArgs::parse(&args(&["push", "--sim", "--fit", "pad"])).unwrap();
        assert_eq!(target.resolve().unwrap(), Target::Simulator);
        assert_eq!(rest, args(&["push", "--fit", "pad"]));

        let (target, rest) =
            TargetArgs::parse(&args(&["--device", "192.0.2.10", "--timeout", "5"])).unwrap();
        assert_eq!(
            target.resolve().unwrap(),
            Target::Address("192.0.2.10".into())
        );
        assert_eq!(rest, args(&["--timeout", "5"]));

        let (target, rest) = TargetArgs::parse(&args(&["-s", "192.0.2.11"])).unwrap();
        assert_eq!(
            target.resolve().unwrap(),
            Target::Address("192.0.2.11".into())
        );
        assert!(rest.is_empty());

        let (target, _) = TargetArgs::parse(&args(&["--reader", "clara"])).unwrap();
        assert_eq!(target.resolve().unwrap(), Target::Nickname("clara".into()));
    }

    #[test]
    fn missing_values_and_conflicts_are_usage_mistakes() {
        let error = TargetArgs::parse(&args(&["--device"])).expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_USAGE);
        let error = TargetArgs::parse(&args(&["--reader"])).expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_USAGE);

        let (empty, _) = TargetArgs::parse(&args(&[])).unwrap();
        assert!(empty.is_empty());
        let error = empty.resolve().expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_USAGE);
        assert!(console::display(&error).contains(TARGET_FLAGS), "{error}");

        for words in [
            &["--sim", "--device", "192.0.2.10"][..],
            &["--device", "192.0.2.10", "--reader", "clara"][..],
            &["--sim", "--reader", "clara"][..],
        ] {
            let (target, _) = TargetArgs::parse(&args(words)).unwrap();
            let error = target.resolve().expect_err("refused");
            assert_eq!(
                console::category_of(&error),
                console::EXIT_USAGE,
                "{words:?}"
            );
            assert!(console::display(&error).contains("one target"), "{error}");
        }
    }

    #[test]
    fn a_nickname_never_becomes_a_guess() {
        let error = resolve_nickname("clara").expect_err("no store yet");
        assert_eq!(console::category_of(&error), console::EXIT_TARGET);
        assert!(console::display(&error).contains("clara"), "{error}");
    }

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn a_companion_command_with_no_target_goes_to_the_saved_reader() {
        let mut said = Vec::new();
        let filled = with_reader(
            &words("frame push photos"),
            default_reader(None, &["clara"]),
            |name| Ok(format!("addr-of-{name}")),
            |line| said.push(line),
        )
        .unwrap();
        assert_eq!(filled, words("frame push photos --device addr-of-clara"));
        assert!(said[0].contains("\"clara\""));
    }

    #[test]
    fn a_named_reader_becomes_its_address_and_a_target_given_is_kept() {
        let resolve = |name: &str| Ok(format!("addr-of-{name}"));
        assert_eq!(
            with_reader(
                &words("vault push notes --reader study"),
                None,
                resolve,
                |_| {}
            )
            .unwrap(),
            words("vault push notes --device addr-of-study")
        );
        for kept in [
            "frame push p --sim",
            "frame push p --device 10.0.0.2",
            "nonograms push i --out d",
        ] {
            assert_eq!(
                with_reader(
                    &words(kept),
                    default_reader(None, &["clara"]),
                    resolve,
                    |_| {}
                )
                .unwrap(),
                words(kept)
            );
        }
    }

    #[test]
    fn previews_and_two_saved_readers_are_left_alone() {
        let resolve = |name: &str| Ok(format!("addr-of-{name}"));
        assert_eq!(
            with_reader(
                &words("vault preview a.md"),
                default_reader(None, &["clara"]),
                resolve,
                |_| {}
            )
            .unwrap(),
            words("vault preview a.md")
        );
        assert_eq!(default_reader(None, &["clara", "libra"]), None);
        assert_eq!(
            default_reader(Some("192.168.1.9"), &["clara", "libra"]),
            Some(DefaultReader::Address("192.168.1.9".into()))
        );
        assert_eq!(
            default_reader(Some("libra"), &["clara", "libra"]),
            Some(DefaultReader::Nickname("libra".into()))
        );
    }
}
