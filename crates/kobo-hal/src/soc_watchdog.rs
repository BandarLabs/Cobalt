//! The `SoC`'s own hardware watchdog.
//!
//! This is not Kobo's freeze watchdog in [`crate::supervisor`], which is a
//! service on the session bus that can be reasoned with. This one is a counter
//! inside the `MediaTek` chip. When it expires the device is reset, immediately,
//! by hardware. Nothing is synced, nothing is logged, and the next thing in the
//! kernel log is a cold boot.
//!
//! It cost days to find because every symptom pointed somewhere else. The
//! device reset itself about ten seconds after a session handed the panel back,
//! so the panel looked guilty. It is not: restarting the reader with no display
//! session, no touch session and no panel involvement at all resets the device
//! just the same.
//!
//! The margin is the whole story:
//!
//! ```text
//! mtk-wdt 10007000.toprgu: Watchdog enabled (timeout=31 sec, nowayout=0)
//! [112:feeding_thread] watchdog feeding_interval = 28000 ms
//! ```
//!
//! A kernel thread feeds a 31 second timer every 28 seconds. Three seconds of
//! slack. Stopping and restarting the reader is the heaviest thing that ever
//! happens on this device, and it spends them.
//!
//! Reading `/proc/wdk` early in the hunt was misleading in the other direction:
//! scanning `/proc/*/fd` for a process holding `/dev/watchdog` finds nobody,
//! because the feeder is a kernel thread and kernel threads have no file
//! descriptor table. That absence was read as "the watchdog is never armed",
//! which was wrong, and it sent the search after the Bluetooth chip, the freeze
//! watchdog and a phantom second reader before the A/B below settled it:
//!
//! | `/proc/wdk` | uptime across a session | outcome |
//! | --- | --- | --- |
//! | `0` (slack) | 247s to 389s | survived, and kept going |
//! | `1` (armed) | 420s to 446s | reset, cold boot |
//!
//! So the reader is given slack for exactly as long as we are standing between
//! it and the hardware, on the same lifetime as the freeze watchdog suspension,
//! and the counter is armed again once the reader is demonstrably back.
//!
//! Slack is the right word for what this does. It does not disable a safety
//! net and walk away: the window is bounded by a guard, the guard restores the
//! previous value on every exit path including a panic, and the kernel restores
//! it anyway on the next boot. A device that is reset every time a developer
//! looks at it has no working safety net either.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where the `MediaTek` watchdog kicker exposes itself. Root writable, and it
/// reads back as a two line table.
pub const WDK: &str = "/proc/wdk";

/// What `/proc/wdk` says about the counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct State {
    /// Whether the counter will reset the device when it expires.
    pub armed: bool,
    /// Seconds it runs for. On this `SoC` the register caps at 31 and writing a
    /// larger number is silently ignored, so it is carried through unchanged
    /// rather than treated as something we may choose.
    pub timeout: u32,
}

#[derive(Debug)]
pub enum WatchdogError {
    /// The node is not there. Every non-MediaTek target lands here, and so does
    /// the simulator, which is why callers treat it as "nothing to do" rather
    /// than a failure.
    Absent,
    /// The node is there but did not read back as the table we expect.
    Unreadable(String),
    Io(io::Error),
}

impl fmt::Display for WatchdogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => write!(
                formatter,
                "{WDK} is not present, so this device has no MediaTek watchdog to slacken"
            ),
            Self::Unreadable(content) => write!(
                formatter,
                "{WDK} did not read back as 'enabled timeout' over two lines, but as {content:?}"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for WatchdogError {}

impl From<io::Error> for WatchdogError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The hardware watchdog, addressed through `/proc/wdk`.
#[derive(Debug, Clone)]
pub struct SocWatchdog {
    node: PathBuf,
}

impl Default for SocWatchdog {
    fn default() -> Self {
        Self::at(Path::new(WDK))
    }
}

impl SocWatchdog {
    #[must_use]
    pub fn at(node: &Path) -> Self {
        Self {
            node: node.to_path_buf(),
        }
    }

    /// Reads the counter's current setting.
    ///
    /// # Errors
    ///
    /// [`WatchdogError::Absent`] when the node is not there at all, and
    /// [`WatchdogError::Unreadable`] when it is there but does not read back as
    /// the table this firmware is known to produce.
    pub fn state(&self) -> Result<State, WatchdogError> {
        if !self.node.exists() {
            return Err(WatchdogError::Absent);
        }
        let content = fs::read_to_string(&self.node)?;
        parse(&content).ok_or(WatchdogError::Unreadable(content))
    }

    /// Arms the counter, whatever it currently says.
    ///
    /// Recovery needs this and [`Slack`] cannot provide it. A session that was
    /// killed outright left the counter slack with no guard alive to restore
    /// it, and `slacken` deliberately refuses to touch a counter it did not
    /// slacken itself. Recovery is the one caller that knows the slack was ours
    /// and that the debt is owed.
    ///
    /// # Errors
    ///
    /// Whatever reading or writing the node produced. A device without the node
    /// is not an error, because there is nothing there to arm.
    pub fn arm(&self) -> Result<(), WatchdogError> {
        let timeout = match self.state() {
            Ok(state) if state.armed => return Ok(()),
            Ok(state) => state.timeout,
            Err(WatchdogError::Absent) => return Ok(()),
            Err(error) => return Err(error),
        };
        write(
            &self.node,
            State {
                armed: true,
                timeout,
            },
        )
    }

    /// Gives the device slack for the length of the returned guard.
    ///
    /// A device without the node is not an error, because there is genuinely
    /// nothing to do there. The guard returned in that case restores nothing.
    ///
    /// # Errors
    ///
    /// Whatever reading or writing the node produced, so a caller can refuse to
    /// stop the reader when the node is present but will not answer.
    pub fn slacken(&self) -> Result<Slack, WatchdogError> {
        let state = match self.state() {
            Ok(state) => state,
            Err(WatchdogError::Absent) => {
                return Ok(Slack {
                    node: self.node.clone(),
                    restore: None,
                })
            }
            Err(error) => return Err(error),
        };
        if !state.armed {
            // Already slack, and by something that is not us. Leaving it that
            // way on the way out is the honest thing to do.
            return Ok(Slack {
                node: self.node.clone(),
                restore: None,
            });
        }
        write(
            &self.node,
            State {
                armed: false,
                timeout: state.timeout,
            },
        )?;
        Ok(Slack {
            node: self.node.clone(),
            restore: Some(state),
        })
    }
}

/// The window in which the hardware will not reset the device.
///
/// Dropping this re-arms the counter, so the window closes on a panic or an
/// early return as well as on the ordinary path. [`Slack::rearm`] exists only
/// so the caller can find out whether it worked.
#[derive(Debug)]
#[must_use = "the hardware is only slack while this guard is alive"]
pub struct Slack {
    node: PathBuf,
    restore: Option<State>,
}

impl Slack {
    /// Whether this guard is actually holding the counter off, as opposed to
    /// having found nothing to do.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.restore.is_some()
    }

    /// Arms the counter again, reporting whether the write landed.
    ///
    /// # Errors
    ///
    /// Whatever writing the node produced. Dropping the guard performs the same
    /// write and discards this, so calling it is only worth it to report.
    pub fn rearm(mut self) -> Result<(), WatchdogError> {
        match self.restore.take() {
            Some(state) => write(&self.node, state),
            None => Ok(()),
        }
    }
}

impl Drop for Slack {
    fn drop(&mut self) {
        if let Some(state) = self.restore.take() {
            let _ignored = write(&self.node, state);
        }
    }
}

fn write(node: &Path, state: State) -> Result<(), WatchdogError> {
    fs::write(node, command(state))?;
    Ok(())
}

/// Renders a setting the way the node's parser wants it.
///
/// Both fields every time, as digits. The parser takes the first token as the
/// enable flag and anything it cannot read as a number counts as enabled, so
/// `disable` turns the watchdog **on**. That was confirmed on the device, and
/// it is the reason this returns a formatted pair rather than a word.
fn command(state: State) -> String {
    format!("{} {}\n", u8::from(state.armed), state.timeout)
}

/// Reads the two line table the node produces.
///
/// ```text
/// enabled timeout
/// 1    31
/// ```
///
/// The header is matched loosely and the values are taken from the first line
/// that is two numbers, so a firmware that pads its columns differently or
/// drops the header still parses.
fn parse(content: &str) -> Option<State> {
    content.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let armed: u32 = fields.next()?.parse().ok()?;
        let timeout: u32 = fields.next()?.parse().ok()?;
        if fields.next().is_some() {
            return None;
        }
        Some(State {
            armed: armed != 0,
            timeout,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("kobo-wdk-{}-{name}", std::process::id()));
        fs::create_dir_all(&directory).expect("create scratch directory");
        directory.join("wdk")
    }

    #[test]
    fn the_device_table_parses() {
        let state = parse("enabled timeout\n1    31      \n").expect("parse");
        assert_eq!(
            state,
            State {
                armed: true,
                timeout: 31
            }
        );
    }

    #[test]
    fn a_slack_counter_reads_as_slack() {
        let state = parse("enabled timeout\n0    31      \n").expect("parse");
        assert!(!state.armed);
        assert_eq!(state.timeout, 31);
    }

    #[test]
    fn the_header_is_never_mistaken_for_values() {
        assert!(parse("enabled timeout\n").is_none());
    }

    #[test]
    fn a_setting_is_written_as_two_numbers() {
        // Words are not an option here: the node reads anything unparsable as
        // enabled, so spelling this "disable" would arm the watchdog.
        assert_eq!(
            command(State {
                armed: false,
                timeout: 31
            }),
            "0 31\n"
        );
        assert_eq!(
            command(State {
                armed: true,
                timeout: 31
            }),
            "1 31\n"
        );
    }

    #[test]
    fn the_timeout_is_carried_through_rather_than_chosen() {
        // The register caps at 31 on this SoC, so whatever is found is what
        // gets put back.
        let node = scratch("carry");
        fs::write(&node, "enabled timeout\n1    24      \n").expect("seed");
        let watchdog = SocWatchdog::at(&node);
        let slack = watchdog.slacken().expect("slacken");
        assert_eq!(fs::read_to_string(&node).expect("read"), "0 24\n");
        slack.rearm().expect("rearm");
        assert_eq!(fs::read_to_string(&node).expect("read"), "1 24\n");
    }

    #[test]
    fn dropping_the_guard_arms_the_counter_again() {
        let node = scratch("drop");
        fs::write(&node, "enabled timeout\n1    31      \n").expect("seed");
        {
            let slack = SocWatchdog::at(&node).slacken().expect("slacken");
            assert!(slack.is_holding());
            assert_eq!(fs::read_to_string(&node).expect("read"), "0 31\n");
        }
        assert_eq!(fs::read_to_string(&node).expect("read"), "1 31\n");
    }

    #[test]
    fn a_counter_someone_else_left_slack_is_left_slack() {
        let node = scratch("already");
        fs::write(&node, "enabled timeout\n0    31      \n").expect("seed");
        {
            let slack = SocWatchdog::at(&node).slacken().expect("slacken");
            assert!(!slack.is_holding());
        }
        assert_eq!(
            fs::read_to_string(&node).expect("read"),
            "enabled timeout\n0    31      \n",
            "nothing was written, so nothing was restored"
        );
    }

    #[test]
    fn a_device_without_the_node_has_nothing_to_do() {
        let node = scratch("absent").with_file_name("nothing-here");
        let slack = SocWatchdog::at(&node)
            .slacken()
            .expect("absent is not a failure");
        assert!(!slack.is_holding());
        slack.rearm().expect("rearm");
        assert!(!node.exists());
    }
}
