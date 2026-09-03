//! Stopping and restarting the stock reader.
//!
//! This is the single most consequential thing the platform does, because
//! nothing on the device restarts the reader for us: `/etc/inittab` respawns
//! only a getty, so a reader that is stopped and not restarted leaves the owner
//! looking at a frozen panel until they power cycle.
//!
//! That is also precisely why it is survivable. The platform never owns boot, so
//! a power cycle always lands in the stock reader. Every failure here costs a
//! reboot and nothing else: no file outside `/tmp` is written, no boot path is
//! touched, and `/tmp` is a tmpfs that a reboot empties.
//!
//! The rules this module enforces:
//!
//! - The reader is identified by the exact executable path in its own
//!   `/proc/<pid>/cmdline`, never by a name search, and exactly one match is
//!   required. `pidof`-style matching would happily signal an unrelated process.
//! - Identity is re-verified immediately before the signal is sent, because
//!   process ids are reused and the reader could have exited on its own in
//!   between.
//! - The restart uses the environment, arguments and working directory captured
//!   from the live process, so the reader we start is the reader that was
//!   running rather than a guess about what it needs.

use kobo_abi::process::{signal, SIGKILL, SIGTERM};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The exact executable the stock reader runs as. Anything else is not the
/// reader and is never signalled.
pub const READER_EXECUTABLE: &str = "/usr/local/Kobo/nickel";

/// Kobo's own freeze watchdog, `com.kobo.watchdog.Sickel`.
///
/// The reader pings this service over D-Bus. If the pings stop, it concludes
/// the reader has frozen and **reboots the device**. Stopping the reader
/// therefore starts a timer we did not set: from its point of view a stopped
/// reader and a hung one look identical.
///
/// This was found the hard way. A five second handoff completed normally; a
/// ninety second session rebooted the device mid-run. So the supervisor is
/// stopped first and the reader second, and because the reader spawns a fresh
/// supervisor on startup, the ordering on the way back is simply the reverse.
pub const SUPERVISOR_EXECUTABLE: &str = "/usr/local/Kobo/sickel";

/// How often to check whether a stopped reader has actually exited.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub enum ReaderError {
    /// No process is running the reader executable.
    NotRunning,
    /// More than one process claims to be the reader, so none can safely be
    /// singled out.
    Ambiguous(Vec<i32>),
    /// The process changed identity between discovery and signalling.
    IdentityChanged(i32),
    /// The reader did not exit within the allowed time even after `SIGKILL`.
    WillNotStop(i32),
    /// The reader was started but never appeared in the process table.
    DidNotStart,
    Io(io::Error),
}

impl fmt::Display for ReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => write!(formatter, "the stock reader is not running"),
            Self::Ambiguous(pids) => {
                write!(
                    formatter,
                    "several processes claim to be the reader: {pids:?}"
                )
            }
            Self::IdentityChanged(pid) => {
                write!(formatter, "process {pid} is no longer the reader")
            }
            Self::WillNotStop(pid) => write!(formatter, "reader {pid} did not exit"),
            Self::DidNotStart => write!(formatter, "the reader did not come back"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for ReaderError {}

impl From<io::Error> for ReaderError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// How a process is recognised as the one being described.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Identity {
    /// The process was started with an absolute path as its zeroth argument.
    ZerothArgument,
    /// The process is identified by the binary behind its `exe` link.
    Executable,
}

/// Everything needed to stop the running reader and start an identical one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reader {
    executable: String,
    pid: i32,
    arguments: Vec<OsString>,
    environment: BTreeMap<OsString, OsString>,
    working_directory: PathBuf,
    /// How the process was recognised, so that every later identity check
    /// asks the same question that found it. A daemon found by its `exe` link
    /// carries a bare zeroth argument, and re-checking it by argv concludes
    /// the process has changed identity when it has not moved at all — which
    /// turns `stop` into a refusal that never sends a signal.
    identity: Identity,
}

impl Reader {
    #[must_use]
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// The program this description starts.
    #[must_use]
    pub fn executable(&self) -> &str {
        &self.executable
    }

    /// The arguments after the executable itself.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    #[must_use]
    pub fn environment_len(&self) -> usize {
        self.environment.len()
    }

    /// Looks up one variable from the reader's captured environment.
    ///
    /// The freeze watchdog lives on a session bus whose address contains a
    /// per-boot socket path, so it has to be read from the reader rather than
    /// assumed.
    #[must_use]
    pub fn environment(&self, name: &str) -> Option<&OsStr> {
        self.environment
            .get(&OsString::from(name))
            .map(OsString::as_os_str)
    }

    /// Finds the single running reader and captures how to restart it.
    ///
    /// # Errors
    ///
    /// Returns an error when no reader is running, when more than one process
    /// matches, or when `/proc` cannot be read.
    pub fn find() -> Result<Self, ReaderError> {
        Self::find_in(Path::new("/proc"))
    }

    /// Finds Kobo's freeze watchdog, which must be stopped before the reader.
    ///
    /// # Errors
    ///
    /// Returns an error when it is not running or cannot be identified.
    pub fn find_supervisor() -> Result<Self, ReaderError> {
        Self::find_executable_in(Path::new("/proc"), SUPERVISOR_EXECUTABLE)
    }

    fn find_in(proc_root: &Path) -> Result<Self, ReaderError> {
        Self::find_executable_in(proc_root, READER_EXECUTABLE)
    }

    /// Finds a process by the executable it is actually running.
    ///
    /// System daemons are usually started from a shell and so carry a bare
    /// zeroth argument like `wpa_supplicant`, which says nothing about what is
    /// running. The `exe` link in `/proc` cannot be spoofed by whoever launched
    /// the process, so it is the identity used for those.
    ///
    /// # Errors
    ///
    /// Returns an error when no such process runs, when several do, or when
    /// `/proc` cannot be read.
    pub fn find_running(executable: &str) -> Result<Self, ReaderError> {
        Self::find_matching_in(Path::new("/proc"), executable, Identity::Executable)
    }

    /// Every process running this executable, lowest process id first.
    ///
    /// [`Self::find_running`] refuses to choose when several match, which is
    /// right when the caller means the one reader and a second would make the
    /// answer a guess. A leftover daemon is the opposite case: several is the
    /// normal answer, one per session that has handed the panel back, and each
    /// one is wanted. Reaping through the single-match lookup would do nothing
    /// on exactly the readers that need it most, because the second leftover
    /// makes the lookup ambiguous and the reap is skipped from then on.
    ///
    /// Reading `/proc` at all is the only failure worth a word, and a caller
    /// that has nothing to stop and a caller that cannot look are in the same
    /// position, so both come back empty.
    #[must_use]
    pub fn find_all_running(executable: &str) -> Vec<Self> {
        Self::find_all_matching_in(Path::new("/proc"), executable, Identity::Executable)
    }

    fn find_all_matching_in(proc_root: &Path, executable: &str, identity: Identity) -> Vec<Self> {
        Self::matching_pids(proc_root, executable, identity)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|pid| Self::describe(proc_root, executable, pid, identity).ok())
            .collect()
    }

    fn find_executable_in(proc_root: &Path, executable: &str) -> Result<Self, ReaderError> {
        Self::find_matching_in(proc_root, executable, Identity::ZerothArgument)
    }

    fn find_matching_in(
        proc_root: &Path,
        executable: &str,
        identity: Identity,
    ) -> Result<Self, ReaderError> {
        let matches = Self::matching_pids(proc_root, executable, identity)?;
        let pid = match matches.as_slice() {
            [] => return Err(ReaderError::NotRunning),
            [only] => *only,
            several => return Err(ReaderError::Ambiguous(several.to_vec())),
        };
        Self::describe(proc_root, executable, pid, identity)
    }

    fn matching_pids(
        proc_root: &Path,
        executable: &str,
        identity: Identity,
    ) -> Result<Vec<i32>, ReaderError> {
        let mut matches = Vec::new();
        for entry in fs::read_dir(proc_root)? {
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if pid <= 1 {
                continue;
            }
            let identified = match identity {
                Identity::ZerothArgument => read_argv(proc_root, pid)
                    .is_some_and(|argv| argv.first().is_some_and(|first| first == executable)),
                Identity::Executable => fs::read_link(proc_root.join(pid.to_string()).join("exe"))
                    .is_ok_and(|target| target == Path::new(executable)),
            };
            if identified {
                matches.push(pid);
            }
        }
        matches.sort_unstable();
        Ok(matches)
    }

    fn describe(
        proc_root: &Path,
        executable: &str,
        pid: i32,
        identity: Identity,
    ) -> Result<Self, ReaderError> {
        let argv = read_argv(proc_root, pid).ok_or(ReaderError::IdentityChanged(pid))?;
        let arguments = argv
            .into_iter()
            .skip(1)
            .map(OsString::from)
            .collect::<Vec<_>>();
        let environment = read_environment(proc_root, pid)?;
        let working_directory = fs::read_link(proc_root.join(pid.to_string()).join("cwd"))
            .unwrap_or_else(|_| PathBuf::from("/"));

        Ok(Self {
            executable: executable.to_owned(),
            pid,
            arguments,
            environment,
            working_directory,
            identity,
        })
    }

    /// Returns whether this exact process is still the reader.
    #[must_use]
    pub fn still_running(&self) -> bool {
        self.still_running_in(Path::new("/proc"))
    }

    fn still_running_in(&self, proc_root: &Path) -> bool {
        match self.identity {
            Identity::ZerothArgument => read_argv(proc_root, self.pid)
                .is_some_and(|argv| argv.first().is_some_and(|first| *first == self.executable)),
            Identity::Executable => fs::read_link(proc_root.join(self.pid.to_string()).join("exe"))
                .is_ok_and(|target| target == Path::new(&self.executable)),
        }
    }

    /// Asks the reader to exit, escalating to `SIGKILL` if it will not.
    ///
    /// Identity is re-checked immediately before each signal, so a reader that
    /// exits on its own is never mistaken for a recycled process id.
    ///
    /// # Errors
    ///
    /// Returns an error when the process is no longer the reader or refuses to
    /// exit even after `SIGKILL`.
    pub fn stop(&self, grace: Duration) -> Result<(), ReaderError> {
        if !self.still_running() {
            return Err(ReaderError::IdentityChanged(self.pid));
        }
        signal(self.pid, SIGTERM)?;
        if self.wait_for_exit(grace) {
            return Ok(());
        }
        // Only escalate if this is still the same process we verified.
        if !self.still_running() {
            return Ok(());
        }
        signal(self.pid, SIGKILL)?;
        if self.wait_for_exit(grace) {
            Ok(())
        } else {
            Err(ReaderError::WillNotStop(self.pid))
        }
    }

    fn wait_for_exit(&self, grace: Duration) -> bool {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if !self.still_running() {
                return true;
            }
            sleep(POLL_INTERVAL);
        }
        !self.still_running()
    }

    /// Builds the command that starts an identical reader.
    ///
    /// The reader is launched through `nohup` and detached from our standard
    /// streams so that it survives this process exiting and cannot be taken
    /// down by a hangup on the developer's SSH session. `setsid` does not exist
    /// on this firmware, so orphan reparenting to init is what keeps it alive.
    #[must_use]
    pub fn start_command(&self) -> Command {
        let mut command = Command::new("/bin/sh");
        // "$0" "$@" passes the executable and its arguments as data, so no
        // quoting or word splitting can alter what is executed.
        command
            .arg("-c")
            .arg(r#"nohup "$0" "$@" >/dev/null 2>&1 &"#)
            .arg(&self.executable)
            .args(&self.arguments)
            .current_dir(&self.working_directory)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Starts an identical reader and waits for it to appear.
    ///
    /// # Errors
    ///
    /// Returns an error when the reader cannot be spawned or never appears in
    /// the process table.
    pub fn start(&self, appear_within: Duration) -> Result<i32, ReaderError> {
        let status = self.start_command().status()?;
        if !status.success() {
            return Err(ReaderError::DidNotStart);
        }
        let deadline = Instant::now() + appear_within;
        loop {
            if let Ok(reader) = Self::find_executable_in(Path::new("/proc"), &self.executable) {
                return Ok(reader.pid);
            }
            if Instant::now() >= deadline {
                return Err(ReaderError::DidNotStart);
            }
            sleep(POLL_INTERVAL);
        }
    }

    /// Writes everything needed to restart this reader into `directory`.
    ///
    /// The environment and arguments are stored in the same NUL-separated form
    /// the kernel uses, so restoring them involves no shell quoting and no
    /// escaping rules that could silently alter a value.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory cannot be written.
    pub fn save(&self, directory: &Path) -> io::Result<()> {
        fs::create_dir_all(directory)?;
        let mut argv = Vec::new();
        for argument in &self.arguments {
            argv.extend_from_slice(argument.as_encoded_bytes());
            argv.push(0);
        }
        let mut environ = Vec::new();
        for (name, value) in &self.environment {
            environ.extend_from_slice(name.as_encoded_bytes());
            environ.push(b'=');
            environ.extend_from_slice(value.as_encoded_bytes());
            environ.push(0);
        }
        fs::write(directory.join("executable"), self.executable.as_bytes())?;
        fs::write(directory.join("argv"), argv)?;
        fs::write(directory.join("environ"), environ)?;
        fs::write(
            directory.join("cwd"),
            self.working_directory.as_os_str().as_encoded_bytes(),
        )?;
        Ok(())
    }

    /// Reads back a reader description written by [`Reader::save`].
    ///
    /// The pid is deliberately not restored: the saved process is gone by the
    /// time this is used, so the value would be meaningless and inviting it to
    /// be signalled would be dangerous.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory is missing or unreadable.
    pub fn load(directory: &Path) -> io::Result<Self> {
        let executable = fs::read_to_string(directory.join("executable"))
            .unwrap_or_else(|_| READER_EXECUTABLE.to_owned());
        let argv = fs::read(directory.join("argv"))?;
        let environ = fs::read(directory.join("environ"))?;
        let cwd = fs::read(directory.join("cwd"))?;
        let arguments = argv
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| OsString::from_vec(part.to_vec()))
            .collect::<Vec<_>>();
        let mut environment = BTreeMap::new();
        for entry in environ.split(|byte| *byte == 0) {
            if entry.is_empty() {
                continue;
            }
            let Some(split) = entry.iter().position(|byte| *byte == b'=') else {
                continue;
            };
            let (name, value) = entry.split_at(split);
            environment.insert(
                OsString::from_vec(name.to_vec()),
                OsString::from_vec(value[1..].to_vec()),
            );
        }
        Ok(Self {
            executable,
            pid: 0,
            arguments,
            environment,
            working_directory: PathBuf::from(OsString::from_vec(cwd)),
            // A saved description names the reader, which is always started
            // with its absolute path as its zeroth argument.
            identity: Identity::ZerothArgument,
        })
    }
}

/// A detached process that restarts the reader if we do not.
///
/// This exists for the one failure no in-process cleanup can cover: being
/// killed outright. `SIGKILL` runs no destructor and no signal handler, so if
/// the runtime is killed between stopping the reader and restarting it, nothing
/// inside this process can put the device back. A separate process that has
/// already been started can.
///
/// It re-executes the caller's own binary against a saved description rather
/// than reconstructing an environment in shell, because quoting an arbitrary
/// environment correctly in shell is a bug waiting to happen and this is the
/// code path that has to work when everything else has failed.
///
/// ## Why a heartbeat rather than a deadline
///
/// The first version slept for the length of the session and then acted. That
/// tied recovery to the session limit: a session allowed to run for an hour
/// left the reader dead for an hour if the runtime was killed one minute in.
/// Now the runtime reports progress and the watchdog only acts when that
/// progress stops, so recovery takes about two minutes however long the
/// session was allowed to be. It also catches the case a deadline cannot: a
/// runtime that is still alive but no longer running its loop.
pub struct Watchdog {
    cancel: PathBuf,
    beat: PathBuf,
    beats: AtomicU64,
}

/// How long the watchdog waits between checks.
///
/// Recovery therefore takes between one and two of these. It has to be
/// comfortably longer than the longest legitimate gap between heartbeats,
/// which is the run-up to the first one: opening the panel, taking the touch
/// device and stopping the reader, the last of which alone is allowed fifteen
/// seconds.
pub const WATCHDOG_CHECK: Duration = Duration::from_secs(60);

impl Watchdog {
    /// Arms a watchdog that restarts the reader once the heartbeat stops.
    ///
    /// `state` must already hold a description written by [`Reader::save`]. The
    /// caller's binary must support `--restart-from <state>`, and the caller
    /// must call [`Watchdog::beat`] more often than `check`.
    ///
    /// # Errors
    ///
    /// Returns an error when the watchdog script cannot be written or started.
    pub fn arm(state: &Path, check: Duration) -> Result<Self, ReaderError> {
        // Deliberately outside `state`. The cancel marker used to live inside
        // it, and the caller deletes that directory as its last act, which
        // removed the marker while the watchdog was still asleep and left it
        // certain to fire. A marker that outlives what it is cancelling is the
        // whole point of it.
        let cancel = sibling(state, "cancel");
        let beat = sibling(state, "beat");
        // A stale marker from a previous session that happened to be given the
        // same name would disarm this one before it ever ran.
        let _ignored = fs::remove_file(&cancel);
        let script = state.join("watchdog.sh");
        let executable = std::env::current_exe()?;
        let body = watchdog_script(&beat, &cancel, &executable, state, check);
        fs::write(&script, body)?;
        let armed = Self {
            cancel,
            beat,
            beats: AtomicU64::new(0),
        };
        // Before the script starts, so its first read never sees an absent file
        // and mistakes a runtime that has not started yet for one that died.
        armed.beat();
        Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"nohup /bin/sh "$0" >/dev/null 2>&1 &"#)
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(armed)
    }

    /// Reports that the runtime is still running its loop.
    ///
    /// Failing to write is deliberately ignored. The consequence of a missed
    /// heartbeat is the reader being restarted, which is the safe direction,
    /// and there is nothing useful a caller could do about it anyway.
    pub fn beat(&self) {
        let count = self.beats.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let _ignored = fs::write(&self.beat, format!("{count}\n"));
    }

    /// Tells the watchdog to do nothing.
    ///
    /// Creating the file is enough; the watchdog checks for it and exits. A
    /// failure to clean up afterwards can therefore never cause a second reader
    /// to be started.
    pub fn disarm(&self) {
        let _ignored = fs::write(&self.cancel, b"disarmed\n");
    }
}

/// A path beside `state` rather than inside it, so it survives the directory.
fn sibling(state: &Path, suffix: &str) -> PathBuf {
    let name = state.file_name().map_or_else(
        || String::from("kobo-session"),
        |name| name.to_string_lossy().into_owned(),
    );
    state
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join(format!("{name}.{suffix}"))
}

/// The watchdog's whole program, as text, so it can be read and tested.
///
/// Two consecutive reads of the same counter mean nothing moved in a whole
/// interval. Comparing counters rather than timestamps keeps this free of date
/// arithmetic, which busybox spells differently everywhere, and makes it
/// immune to the clock being set while a session is running.
///
/// It removes the heartbeat and the cancel marker on its way out because it is
/// the last thing that reads either. The session cannot: the marker has to
/// outlive the session that wrote it or a watchdog already sleeping would wake
/// up, find nothing cancelled and restart a reader that is already running.
/// Both files stayed in `/tmp` after every clean session until a real one was
/// run on hardware and the directory was looked at.
fn watchdog_script(
    beat: &Path,
    cancel: &Path,
    executable: &Path,
    state: &Path,
    check: Duration,
) -> String {
    format!(
        "#!/bin/sh\n\
         while :; do\n\
         before=$(cat '{beat}' 2>/dev/null)\n\
         sleep {seconds}\n\
         [ -e '{cancel}' ] && {{ rm -f '{cancel}' '{beat}'; exit 0; }}\n\
         after=$(cat '{beat}' 2>/dev/null)\n\
         [ \"$before\" = \"$after\" ] && break\n\
         done\n\
         [ -e '{cancel}' ] && {{ rm -f '{cancel}' '{beat}'; exit 0; }}\n\
         exec '{executable}' --restart-from '{state}'\n",
        beat = beat.display(),
        seconds = check.as_secs().max(1),
        cancel = cancel.display(),
        executable = executable.display(),
        state = state.display()
    )
}

fn read_argv(proc_root: &Path, pid: i32) -> Option<Vec<String>> {
    let bytes = fs::read(proc_root.join(pid.to_string()).join("cmdline")).ok()?;
    let argv = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect::<Vec<_>>();
    if argv.is_empty() {
        None
    } else {
        Some(argv)
    }
}

fn read_environment(proc_root: &Path, pid: i32) -> io::Result<BTreeMap<OsString, OsString>> {
    let bytes = fs::read(proc_root.join(pid.to_string()).join("environ"))?;
    let mut environment = BTreeMap::new();
    for entry in bytes.split(|byte| *byte == 0) {
        if entry.is_empty() {
            continue;
        }
        let Some(split) = entry.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let (name, value) = entry.split_at(split);
        environment.insert(
            OsString::from_vec(name.to_vec()),
            OsString::from_vec(value[1..].to_vec()),
        );
    }
    Ok(environment)
}

#[cfg(test)]
mod tests {
    use super::{
        read_argv, read_environment, sibling, watchdog_script, Reader, ReaderError,
        READER_EXECUTABLE,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// Builds a fake `/proc` so discovery can be tested without a device.
    fn fake_proc(label: &str, entries: &[(i32, &str, &[&str])]) -> PathBuf {
        // Tests run in parallel, so the label keeps each test's fake /proc
        // separate. A shape-derived name silently collides.
        let root =
            std::env::temp_dir().join(format!("kobo-reader-test-{}-{label}", std::process::id()));
        let _ignored = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create fake proc");
        for (pid, executable, arguments) in entries {
            let directory = root.join(pid.to_string());
            fs::create_dir_all(&directory).expect("create pid directory");
            let mut cmdline = Vec::new();
            cmdline.extend_from_slice(executable.as_bytes());
            cmdline.push(0);
            for argument in *arguments {
                cmdline.extend_from_slice(argument.as_bytes());
                cmdline.push(0);
            }
            fs::write(directory.join("cmdline"), cmdline).expect("write cmdline");
            fs::write(
                directory.join("environ"),
                b"A=1\0LD_LIBRARY_PATH=/usr/local/Kobo\0",
            )
            .expect("write environ");
        }
        root
    }

    #[test]
    fn finds_exactly_one_reader() {
        let root = fake_proc(
            "finds_one",
            &[
                (360, READER_EXECUTABLE, &["-platform", "kobo"]),
                (12, "/bin/sh", &[]),
            ],
        );
        let reader = Reader::find_in(&root).expect("reader found");
        assert_eq!(reader.pid(), 360);
        assert_eq!(
            reader.arguments(),
            [OsString::from("-platform"), OsString::from("kobo")]
        );
        let _ignored = fs::remove_dir_all(&root);
    }

    /// A leftover daemon accumulates one per session, so several is the state
    /// the reap exists for.
    ///
    /// Found on a device: eight orphaned `wmt_launcher` processes after eight
    /// hand-backs. Reaping through the single-match lookup would have refused
    /// every one of them as ambiguous from the second session onward, which is
    /// to say it would have stopped working exactly when it started mattering.
    #[test]
    fn every_leftover_daemon_is_found_and_not_refused_as_ambiguous() {
        let root = fake_proc(
            "leftovers",
            &[
                (700, "wmt_launcher", &["-p", "/lib/firmware"]),
                (701, "wmt_launcher", &["-p", "/lib/firmware"]),
                (702, "wmt_launcher", &["-p", "/lib/firmware"]),
            ],
        );
        for pid in [700, 701, 702] {
            std::os::unix::fs::symlink("/bin/sh", root.join(pid.to_string()).join("exe"))
                .expect("create exe link");
        }
        let leftovers =
            Reader::find_all_matching_in(&root, "/bin/sh", super::Identity::Executable);
        assert_eq!(
            leftovers.iter().map(Reader::pid).collect::<Vec<_>>(),
            vec![700, 701, 702],
            "the single-match lookup would have refused these as ambiguous"
        );
        // Each one is described well enough to be signalled, which is the only
        // reason to have found it.
        for leftover in &leftovers {
            assert!(leftover.still_running_in(&root));
        }
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_to_reap_is_an_empty_answer_rather_than_a_failure() {
        let root = fake_proc("no_leftovers", &[(360, READER_EXECUTABLE, &[])]);
        assert!(
            Reader::find_all_matching_in(&root, "/bin/sh", super::Identity::Executable).is_empty()
        );
        // A caller that cannot read /proc is in the same position as one with
        // nothing to stop, and must not be made to handle a second outcome.
        assert!(Reader::find_all_matching_in(
            Path::new("/definitely/not/a/proc"),
            "/bin/sh",
            super::Identity::Executable
        )
        .is_empty());
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_when_several_processes_claim_to_be_the_reader() {
        // Signalling either one could stop something that is not the reader, so
        // the only safe answer is to refuse.
        let root = fake_proc(
            "ambiguous",
            &[(360, READER_EXECUTABLE, &[]), (361, READER_EXECUTABLE, &[])],
        );
        match Reader::find_in(&root) {
            Err(ReaderError::Ambiguous(pids)) => assert_eq!(pids, vec![360, 361]),
            other => panic!("expected an ambiguous reader, got {other:?}"),
        }
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn refuses_when_no_reader_is_running() {
        let root = fake_proc("none", &[(12, "/bin/sh", &[])]);
        assert!(matches!(
            Reader::find_in(&root),
            Err(ReaderError::NotRunning)
        ));
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_process_merely_named_nickel_is_not_the_reader() {
        // A name search would match this; an exact path comparison does not.
        let root = fake_proc(
            "named_only",
            &[(400, "/tmp/nickel", &[]), (401, "nickel", &[])],
        );
        assert!(matches!(
            Reader::find_in(&root),
            Err(ReaderError::NotRunning)
        ));
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn init_is_never_considered() {
        let root = fake_proc(
            "init",
            &[(1, READER_EXECUTABLE, &[]), (360, READER_EXECUTABLE, &[])],
        );
        let reader = Reader::find_in(&root).expect("reader found");
        assert_eq!(reader.pid(), 360);
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn identity_check_notices_a_recycled_process_id() {
        let root = fake_proc("recycled", &[(360, READER_EXECUTABLE, &[])]);
        let reader = Reader::find_in(&root).expect("reader found");
        assert!(reader.still_running_in(&root));
        // The same pid now belongs to something else entirely.
        fs::write(root.join("360").join("cmdline"), b"/bin/sh\0").expect("rewrite cmdline");
        assert!(!reader.still_running_in(&root));
        let _ignored = fs::remove_dir_all(&root);
    }

    /// The defect this pins was found on a device: the supplicant was found
    /// by its `exe` link, because Nickel starts it with a bare zeroth
    /// argument, and `stop` then re-checked it by argv, concluded the identity
    /// had changed, and refused without sending a signal. The identity that
    /// found a process is the one every later check must use.
    #[test]
    fn a_daemon_found_by_its_exe_link_is_still_recognised_by_it() {
        let root = fake_proc("exe_link", &[(500, "wpa_supplicant", &["-B"])]);
        // The exe link points at the real binary however the process was
        // launched; /bin/sh exists on every machine the tests run on.
        std::os::unix::fs::symlink("/bin/sh", root.join("500").join("exe"))
            .expect("create exe link");
        let daemon = Reader::find_matching_in(&root, "/bin/sh", super::Identity::Executable)
            .expect("daemon found");
        assert_eq!(daemon.pid(), 500);
        assert!(
            daemon.still_running_in(&root),
            "a daemon that has not moved was declared a different process"
        );
        // Once the process is gone the same check must say so.
        fs::remove_file(root.join("500").join("exe")).expect("remove exe link");
        assert!(!daemon.still_running_in(&root));
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_captured_environment_is_reused_verbatim() {
        let root = fake_proc("environment", &[(360, READER_EXECUTABLE, &[])]);
        let reader = Reader::find_in(&root).expect("reader found");
        assert_eq!(reader.environment_len(), 2);
        let command = reader.start_command();
        let names = command
            .get_envs()
            .map(|(name, _)| name.to_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&OsString::from("LD_LIBRARY_PATH")));
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_start_command_passes_arguments_as_data() {
        let root = fake_proc(
            "arguments",
            &[(
                360,
                READER_EXECUTABLE,
                &["-platform", "kobo", "-skipFontLoad"],
            )],
        );
        let reader = Reader::find_in(&root).expect("reader found");
        let command = reader.start_command();
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // The executable and its arguments follow the script, so the shell
        // receives them as "$0" and "$@" rather than inside the script text.
        assert_eq!(arguments[0], "-c");
        assert_eq!(arguments[2], READER_EXECUTABLE);
        assert_eq!(arguments[3], "-platform");
        assert_eq!(arguments[5], "-skipFontLoad");
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_command_line_is_not_a_process_we_can_identify() {
        // Kernel threads have an empty cmdline and must never be selected.
        let root = fake_proc("empty_cmdline", &[(360, READER_EXECUTABLE, &[])]);
        fs::write(root.join("360").join("cmdline"), b"").expect("empty cmdline");
        assert!(read_argv(&root, 360).is_none());
        assert!(matches!(
            Reader::find_in(&root),
            Err(ReaderError::NotRunning)
        ));
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn environment_entries_without_a_separator_are_skipped() {
        let root = fake_proc("no_separator", &[(360, READER_EXECUTABLE, &[])]);
        fs::write(root.join("360").join("environ"), b"BROKEN\0GOOD=1\0").expect("write environ");
        let environment = read_environment(&root, 360).expect("read environment");
        assert_eq!(environment.len(), 1);
        assert_eq!(
            environment.get(&OsString::from("GOOD")),
            Some(&OsString::from("1"))
        );
        let _ignored = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_proc_directory_is_an_error_rather_than_a_panic() {
        assert!(Reader::find_in(Path::new("/nonexistent-proc")).is_err());
    }

    #[test]
    fn the_cancel_marker_lives_outside_the_directory_it_cancels() {
        // The runtime deletes the session directory as its last act. When the
        // marker lived inside it, disarming and then tidying up removed the
        // proof that the watchdog had been disarmed, so a watchdog that was
        // still asleep woke to find no marker and restarted a reader that was
        // already running.
        let state = Path::new("/tmp/kobo-session-1234");
        let cancel = sibling(state, "cancel");
        assert!(!cancel.starts_with(state));
        assert_eq!(cancel, PathBuf::from("/tmp/kobo-session-1234.cancel"));
    }

    #[test]
    fn the_watchdog_acts_only_after_two_readings_agree() {
        // One reading proves nothing: the runtime may simply not have reached
        // its next heartbeat. Two identical readings a whole interval apart
        // mean the loop is no longer running.
        let script = watchdog_script(
            Path::new("/tmp/s.beat"),
            Path::new("/tmp/s.cancel"),
            Path::new("/tmp/kobod"),
            Path::new("/tmp/s"),
            Duration::from_secs(60),
        );
        assert!(script.contains("before=$(cat '/tmp/s.beat'"));
        assert!(script.contains("after=$(cat '/tmp/s.beat'"));
        assert!(script.contains("sleep 60"));
        assert!(script.contains("[ \"$before\" = \"$after\" ] && break"));
        assert!(script.contains("exec '/tmp/kobod' --restart-from '/tmp/s'"));
    }

    #[test]
    fn the_watchdog_checks_for_the_cancel_marker_after_sleeping_and_before_acting() {
        // Checking only before the sleep would race with a session that ends
        // during it, and the cost of that race is a second reader.
        let script = watchdog_script(
            Path::new("/tmp/s.beat"),
            Path::new("/tmp/s.cancel"),
            Path::new("/tmp/kobod"),
            Path::new("/tmp/s"),
            Duration::from_secs(60),
        );
        let guard = "[ -e '/tmp/s.cancel' ]";
        let checks = script.matches(guard).count();
        assert_eq!(checks, 2, "once inside the loop and once before acting");
        let acts_at = script.find("exec '/tmp/kobod'").expect("acts");
        let last_check = script.rfind(guard).expect("checks");
        assert!(last_check < acts_at);
    }

    /// Both files sat in `/tmp` after every clean session, because the session
    /// cannot remove a marker a sleeping watchdog has yet to read. The script
    /// is the last thing that reads either, so it is the thing that sweeps.
    #[test]
    fn the_watchdog_removes_its_own_files_when_it_stands_down() {
        let script = watchdog_script(
            Path::new("/tmp/s.beat"),
            Path::new("/tmp/s.cancel"),
            Path::new("/tmp/kobod"),
            Path::new("/tmp/s"),
            Duration::from_secs(60),
        );
        assert_eq!(
            script
                .matches("rm -f '/tmp/s.cancel' '/tmp/s.beat'; exit 0;")
                .count(),
            2,
            "both stand-down paths have to sweep, or one of them leaks"
        );
        assert!(
            !script[script.find("exec '/tmp/kobod'").expect("acts")..].contains("rm -f"),
            "the recovery path must leave the state alone; kobod reads it"
        );
    }

    #[test]
    fn the_watchdog_never_sleeps_for_no_time_at_all() {
        // A zero interval would spin, and on a device with no fan and a small
        // battery a spinning shell loop is a real cost.
        let script = watchdog_script(
            Path::new("/tmp/s.beat"),
            Path::new("/tmp/s.cancel"),
            Path::new("/tmp/kobod"),
            Path::new("/tmp/s"),
            Duration::from_millis(1),
        );
        assert!(script.contains("sleep 1"));
    }
}
