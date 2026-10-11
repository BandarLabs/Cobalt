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

/// How a command's own parser reads the reader it is given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Flags {
    /// Anywhere on the line, and only as `--device`, so `--reader NAME` is
    /// turned into the address that reader answers at before it arrives.
    Device,
    /// Anywhere on the line, through [`TargetArgs`], which resolves
    /// `--reader NAME` itself. The name is left as typed, because `send`
    /// keeps its target words for `--retry` and a name outlives the lease
    /// its address came from.
    Reader,
    /// Only as the first words after the command, the way `adb -s` is read.
    /// `kobo shell` passes every later word to the reader's shell, so a
    /// `-s` or `--reader` further along belongs to that command, not here.
    Leading,
}

/// One command that acts on a reader, and how it takes one.
#[derive(Clone, Copy, Debug)]
struct ReaderCommand {
    command: &'static str,
    /// The subcommands that act on a reader. Empty when the command acts on
    /// one whatever follows it.
    subcommands: &'static [&'static str],
    flags: Flags,
    /// Words that already settle where the command goes, so no reader is
    /// filled in beside them.
    settled: &'static [&'static str],
    /// Whether a line with no target goes to the default reader. Off for a
    /// command whose bare form already means something on this computer,
    /// which still takes `--reader NAME` when one is typed.
    defaults: bool,
    /// Whether the command with nothing after it prints its usage, which a
    /// filled-in reader would turn into a malformed request.
    bare_is_help: bool,
}

/// Where a result goes instead of a reader: the simulator, a reader at an
/// address, a file on this computer, or a reader mounted over USB.
const SENDS_ELSEWHERE: &[&str] = &[
    "--sim",
    "--device",
    "-s",
    "--out",
    "--volume",
    "--kobo-root",
];

/// The same, for a command that brings something home from a reader. Its
/// `--out` is where the copy lands on this computer, which says nothing
/// about the reader it comes from.
const RECEIVES_ELSEWHERE: &[&str] = &["--sim", "--device", "-s"];

/// The same, for `send`. A preview is local by definition, and a retry
/// carries the target words the failed send was given, so filling in the
/// paired reader beside either would be a second target.
const SEND_ELSEWHERE: &[&str] = &["--sim", "--device", "-s", "--preview", "--retry"];

impl ReaderCommand {
    const fn sends(command: &'static str, subcommands: &'static [&'static str]) -> Self {
        Self {
            command,
            subcommands,
            flags: Flags::Device,
            settled: SENDS_ELSEWHERE,
            defaults: true,
            bare_is_help: false,
        }
    }

    const fn receives(command: &'static str, subcommands: &'static [&'static str]) -> Self {
        Self {
            settled: RECEIVES_ELSEWHERE,
            ..Self::sends(command, subcommands)
        }
    }

    const fn leading(command: &'static str, subcommands: &'static [&'static str]) -> Self {
        Self {
            flags: Flags::Leading,
            settled: RECEIVES_ELSEWHERE,
            ..Self::sends(command, subcommands)
        }
    }

    const fn help_when_bare(self) -> Self {
        Self {
            bare_is_help: true,
            ..self
        }
    }

    /// How many words name the command, which is where a leading target goes.
    fn words(&self) -> usize {
        if self.subcommands.is_empty() {
            1
        } else {
            2
        }
    }
}

/// The commands that act on a reader, and how each takes one.
///
/// Anything not listed is left exactly as it was typed: a preview, an
/// inspection or a status check has no reader to name, `doctor` diagnoses
/// this computer when it is given no reader, and `guard-test` takes the
/// panel, so it is only ever pointed at a reader by address.
const ACTS_ON_A_READER: &[ReaderCommand] = &[
    ReaderCommand::sends("frame", &["init", "push", "ls", "rm"]),
    ReaderCommand::sends("musicstand", &["init", "push", "plan", "ls", "rm"]),
    ReaderCommand::sends("vault", &["init", "push", "plan", "ingest", "ls", "rm"]),
    ReaderCommand::sends("fieldbook", &["push", "ls"]),
    ReaderCommand::receives("fieldbook", &["export"]),
    ReaderCommand::sends("needles", &["push"]),
    ReaderCommand::sends("nonograms", &["push"]),
    ReaderCommand::sends("parser", &["push"]),
    ReaderCommand::sends("panels", &["push"]),
    ReaderCommand::sends("post", &["login"]),
    ReaderCommand::sends("readlater", &["login"]),
    ReaderCommand::sends("birds", &["listen", "push"]),
    ReaderCommand::sends("feeds", &["push"]),
    ReaderCommand::sends("deck", &["push"]),
    ReaderCommand::sends("secret", &["set", "list", "remove"]),
    ReaderCommand::sends("trust", &["set", "list", "remove"]),
    ReaderCommand::sends("sync", &["setup"]),
    ReaderCommand::receives("export", &[]).help_when_bare(),
    ReaderCommand {
        flags: Flags::Reader,
        settled: SEND_ELSEWHERE,
        ..ReaderCommand::sends("send", &[]).help_when_bare()
    },
    ReaderCommand {
        flags: Flags::Reader,
        ..ReaderCommand::receives("wait", &[])
    },
    ReaderCommand::receives("record", &[]),
    ReaderCommand {
        defaults: false,
        ..ReaderCommand::receives("shot", &[])
    },
    ReaderCommand::leading("logs", &[]),
    ReaderCommand::leading("session", &[]),
    ReaderCommand::leading("touch-probe", &[]),
    ReaderCommand::leading("app-link", &["status", "unpair"]),
    ReaderCommand::leading("shell", &[]),
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

/// [`default_reader`] as this computer is set up: its environment and its
/// saved readers. A store that cannot be read saves nothing to default to.
#[must_use]
pub fn configured_default() -> Option<DefaultReader> {
    let store = crate::readers::Store::load(&crate::readers::store_path()).unwrap_or_default();
    let names: Vec<&str> = store
        .readers
        .iter()
        .map(|reader| reader.nickname.as_str())
        .collect();
    default_reader(std::env::var(READER_VARIABLE).ok().as_deref(), &names)
}

/// The entry this command line falls under, when it acts on a reader.
fn reader_command(arguments: &[String]) -> Option<&'static ReaderCommand> {
    let command = arguments.first()?;
    ACTS_ON_A_READER.iter().find(|entry| {
        entry.command == command
            && (entry.subcommands.is_empty()
                || arguments
                    .get(1)
                    .is_some_and(|sub| entry.subcommands.contains(&sub.as_str())))
    })
}

/// Whether this line asks for the command's usage rather than for it to run.
///
/// Answered before any reader is looked for, because help that waits on a
/// network probe, or fails when the paired reader is asleep, is help that
/// is not there when it is wanted.
fn asks_for_help(entry: &ReaderCommand, after: &[String]) -> bool {
    let is_help = |word: &String| word == "--help" || word == "-h";
    match entry.flags {
        Flags::Leading => after.first().is_some_and(is_help),
        Flags::Device | Flags::Reader => {
            (entry.bare_is_help && after.is_empty()) || after.iter().any(is_help)
        }
    }
}

/// The command line with its reader filled in, so every companion command
/// accepts `--reader NAME` and needs no target at all once a reader is saved.
///
/// Apps used to tell their owners to run `kobo frame push PHOTOS --device IP`
/// and nothing on the reader says what its IP is. With a reader saved by
/// `kobo pair`, the same push is `kobo frame push PHOTOS`. A command that
/// names a target keeps it; `--reader NAME` becomes the address that reader
/// answers at for a command that only reads `--device`.
///
/// `resolve` turns a saved name into an address; `said` reports what was
/// chosen, so the owner always sees which reader a command went to.
pub fn with_reader(
    arguments: &[String],
    default: Option<DefaultReader>,
    mut resolve: impl FnMut(&str) -> Result<String, String>,
    mut said: impl FnMut(String),
) -> Result<Vec<String>, String> {
    let Some(entry) = reader_command(arguments) else {
        return Ok(arguments.to_vec());
    };
    let (head, after) = arguments.split_at(entry.words());
    if asks_for_help(entry, after) {
        return Ok(arguments.to_vec());
    }
    // A leading target is only ever read where the command reads it. Further
    // along, the same words are somebody else's.
    let considered = match entry.flags {
        Flags::Leading => &after[..after.len().min(1)],
        Flags::Device | Flags::Reader => after,
    };
    if let Some(position) = considered.iter().position(|word| word == "--reader") {
        if entry.flags == Flags::Reader {
            return Ok(arguments.to_vec());
        }
        let Some(name) = after.get(position + 1) else {
            return Err(crate::console::usage("--reader takes a name"));
        };
        let address = resolve(name)?;
        let mut rest = after.to_vec();
        rest.drain(position..=position + 1);
        return Ok(placed(entry, head, rest, ["--device".to_owned(), address]));
    }
    if considered
        .iter()
        .any(|word| entry.settled.contains(&word.as_str()))
        || !entry.defaults
    {
        return Ok(arguments.to_vec());
    }
    let target = match (default, entry.flags) {
        (Some(DefaultReader::Address(address)), _) => {
            said(format!(
                "Using the reader at {address} ({READER_VARIABLE})."
            ));
            ["--device".to_owned(), address]
        }
        (Some(DefaultReader::Nickname(name)), Flags::Reader) => {
            said(format!("Using your saved reader \"{name}\"."));
            ["--reader".to_owned(), name]
        }
        (Some(DefaultReader::Nickname(name)), Flags::Device | Flags::Leading) => {
            let address = resolve(&name)?;
            said(format!("Using your saved reader \"{name}\" at {address}."));
            ["--device".to_owned(), address]
        }
        (None, _) => return Ok(arguments.to_vec()),
    };
    Ok(placed(entry, head, after.to_vec(), target))
}

/// The command words, the rest and the target, in the order the command's
/// parser reads them: the target first for a leading parser, last otherwise.
fn placed(
    entry: &ReaderCommand,
    head: &[String],
    rest: Vec<String>,
    target: [String; 2],
) -> Vec<String> {
    let mut line = head.to_vec();
    if entry.flags == Flags::Leading {
        line.extend(target);
        line.extend(rest);
    } else {
        line.extend(rest);
        line.extend(target);
    }
    line
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

    /// Fills in the saved reader "clara" the way `run` does, recording every
    /// name that had to be looked up.
    fn filled(line: &str) -> (Result<Vec<String>, String>, Vec<String>) {
        let mut looked_up = Vec::new();
        let filled = with_reader(
            &words(line),
            default_reader(None, &["clara"]),
            |name| {
                looked_up.push(name.to_owned());
                Ok(format!("addr-of-{name}"))
            },
            |_| {},
        );
        (filled, looked_up)
    }

    #[test]
    fn send_and_wait_go_to_the_saved_reader_by_name() {
        assert_eq!(
            filled("send photo.jpg").0.unwrap(),
            words("send photo.jpg --reader clara")
        );
        assert_eq!(filled("wait").0.unwrap(), words("wait --reader clara"));
        let (kept, looked_up) = filled("send photo.jpg --reader study");
        assert_eq!(kept.unwrap(), words("send photo.jpg --reader study"));
        assert!(
            looked_up.is_empty(),
            "send resolves its own names, so a retry keeps the name"
        );
    }

    #[test]
    fn a_preview_a_retry_and_a_bare_send_are_not_given_a_reader() {
        for line in [
            "send --preview photo.jpg --out preview",
            "send --retry",
            "send",
        ] {
            let (kept, looked_up) = filled(line);
            assert_eq!(kept.unwrap(), words(line), "{line}");
            assert!(looked_up.is_empty(), "{line}");
        }
    }

    #[test]
    fn help_is_answered_without_looking_for_a_reader() {
        for line in [
            "frame push --help",
            "export --help",
            "export",
            "fieldbook export -h",
            "logs --help",
            "shell -h",
        ] {
            let (kept, looked_up) = filled(line);
            assert_eq!(kept.unwrap(), words(line), "{line}");
            assert!(looked_up.is_empty(), "{line} probed for a reader");
        }
    }

    #[test]
    fn a_copy_received_into_a_folder_still_comes_from_the_saved_reader() {
        assert_eq!(
            filled("export --app verses --out copies").0.unwrap(),
            words("export --app verses --out copies --device addr-of-clara")
        );
        assert_eq!(
            filled("fieldbook export --out outings.csv").0.unwrap(),
            words("fieldbook export --out outings.csv --device addr-of-clara")
        );
        assert_eq!(
            filled("record --seconds 5 --out film").0.unwrap(),
            words("record --seconds 5 --out film --device addr-of-clara")
        );
    }

    #[test]
    fn a_leading_target_goes_first_and_only_the_first_words_are_read_as_one() {
        assert_eq!(
            filled("logs -f").0.unwrap(),
            words("logs --device addr-of-clara -f")
        );
        assert_eq!(
            filled("logs --reader study -f").0.unwrap(),
            words("logs --device addr-of-study -f")
        );
        assert_eq!(
            filled("app-link status").0.unwrap(),
            words("app-link status --device addr-of-clara")
        );
        // Everything after the target is the reader's own shell command, so
        // its -s and --reader are left for that command to read.
        assert_eq!(
            filled("shell grep -s --reader x /etc/hosts").0.unwrap(),
            words("shell --device addr-of-clara grep -s --reader x /etc/hosts")
        );
        assert_eq!(
            filled("shell --reader study uptime").0.unwrap(),
            words("shell --device addr-of-study uptime")
        );
        assert_eq!(
            filled("shell --device 10.0.0.2 uptime").0.unwrap(),
            words("shell --device 10.0.0.2 uptime")
        );
    }

    #[test]
    fn a_command_with_a_local_default_keeps_it_but_takes_a_named_reader() {
        let (kept, looked_up) = filled("shot --out panel.png");
        assert_eq!(kept.unwrap(), words("shot --out panel.png"));
        assert!(looked_up.is_empty());
        assert_eq!(
            filled("shot --reader study").0.unwrap(),
            words("shot --device addr-of-study")
        );
        assert_eq!(filled("doctor --json").0.unwrap(), words("doctor --json"));
    }

    #[test]
    fn a_reader_named_by_address_in_the_environment_is_used_as_given() {
        let filled = with_reader(
            &words("send photo.jpg"),
            default_reader(Some("192.168.1.9"), &["clara"]),
            |_| Err("nothing is looked up".to_owned()),
            |_| {},
        )
        .unwrap();
        assert_eq!(filled, words("send photo.jpg --device 192.168.1.9"));
    }
}
