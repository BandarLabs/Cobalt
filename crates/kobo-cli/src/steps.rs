//! The setup steps this computer has completed, remembered across runs.
//!
//! The guided surface asks "where were we?" from here rather than asking the
//! owner to remember: a finished USB setup, a named reader, a first send.
//! One file, the same `key = value` blocks the reader store uses, so it
//! stays readable and editable without tooling. A step is recorded only
//! after the work it names actually finished - a dry run or a declined
//! confirmation completes nothing.

use std::path::{Path, PathBuf};

/// One completed step: what finished, for which reader, and when.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Step {
    /// The step's name, e.g. `setup`.
    pub name: String,
    /// The serial of the reader it was completed for.
    pub serial: String,
    /// Seconds since the epoch when it finished.
    pub at: u64,
}

/// Every completed step, oldest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Steps {
    /// Completed steps in the order they finished.
    pub steps: Vec<Step>,
}

impl Steps {
    /// Reads a steps file. Unknown lines are ignored, so the file can grow
    /// fields without stranding older builds.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut steps = Self::default();
        let mut step = Step::empty();
        let mut started = false;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                if started {
                    steps.steps.push(step.clone());
                    step = Step::empty();
                    started = false;
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "step" => {
                    if started {
                        steps.steps.push(step.clone());
                        step = Step::empty();
                    }
                    value.clone_into(&mut step.name);
                    started = true;
                }
                "serial" if started => value.clone_into(&mut step.serial),
                "at" if started => step.at = value.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
        if started {
            steps.steps.push(step);
        }
        steps
    }

    /// Writes the steps back out, in the same shape [`Steps::parse`] reads.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for step in &self.steps {
            let _ = writeln!(out, "step = {}", step.name);
            let _ = writeln!(out, "serial = {}", step.serial);
            let _ = writeln!(out, "at = {}", step.at);
            out.push('\n');
        }
        out
    }

    /// Loads the steps at `path`; a missing file is no steps.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
        }
    }

    /// Saves the steps at `path`, making its folder when needed.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("{} cannot be made: {error}", parent.display()))?;
        }
        std::fs::write(path, self.render())
            .map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    /// True when `name` was completed - for `serial` when one is given.
    #[must_use]
    pub fn completed(&self, name: &str, serial: Option<&str>) -> bool {
        self.steps
            .iter()
            .any(|step| step.name == name && serial.is_none_or(|serial| step.serial == serial))
    }

    /// Records a step just finished, once. Re-recording the same step for the
    /// same reader refreshes its time rather than stacking a duplicate.
    pub fn complete(&mut self, name: &str, serial: &str, at: u64) {
        self.steps
            .retain(|step| !(step.name == name && step.serial == serial));
        self.steps.push(Step {
            name: name.to_owned(),
            serial: serial.to_owned(),
            at,
        });
    }
}

impl Step {
    fn empty() -> Self {
        Self {
            name: String::new(),
            serial: String::new(),
            at: 0,
        }
    }
}

/// The `setup` step's name, so call sites and readers agree on it.
pub const SETUP: &str = "setup";

/// Where the steps live: `$KOBO_CONFIG_DIR/setup-steps`, else
/// `~/.config/kobo/setup-steps`, beside the reader store.
#[must_use]
pub fn steps_path() -> PathBuf {
    if let Some(value) = std::env::var_os("KOBO_CONFIG_DIR") {
        return PathBuf::from(value).join("setup-steps");
    }
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home)
        .join(".config")
        .join("kobo")
        .join("setup-steps")
}

/// The current time as the steps file records it.
#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip() {
        let steps = Steps::parse("step = setup\nserial = N365410043013\nat = 1758250000\n\n");
        assert_eq!(steps.steps.len(), 1);
        assert!(steps.completed(SETUP, Some("N365410043013")));
        assert!(steps.completed(SETUP, None));
        assert!(!steps.completed("paired", None));
        assert!(!steps.completed(SETUP, Some("N418999999999")));
        assert_eq!(Steps::parse(&steps.render()), steps);
    }

    #[test]
    fn completing_twice_refreshes_instead_of_stacking() {
        let mut steps = Steps::default();
        steps.complete(SETUP, "N365410043013", 1);
        steps.complete(SETUP, "N365410043013", 2);
        assert_eq!(steps.steps.len(), 1);
        assert_eq!(steps.steps[0].at, 2);
        steps.complete(SETUP, "N418999999999", 3);
        assert_eq!(steps.steps.len(), 2, "a second reader is its own step");
    }

    #[test]
    fn a_missing_file_is_no_steps() {
        let path = std::env::temp_dir().join("kobo-steps-test-absent");
        let _ = std::fs::remove_file(&path);
        assert!(Steps::load(&path).expect("loads").steps.is_empty());
    }
}
