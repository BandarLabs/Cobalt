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
}
