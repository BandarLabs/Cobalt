//! Physical sleep control for a Cobalt-owned panel session.
//!
//! Nickel normally owns both halves of this operation: it listens to the PMIC
//! power-key input and writes `mem` to `/sys/power/state`. A Cobalt session has
//! deliberately stopped Nickel, so it must do both itself or the button becomes
//! inert and an idle session can only hand the reader back.

use crate::touch::InputEvent32;
use kobo_abi::input;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;

const INPUT_DIR: &str = "/dev/input";
const POWER_KEY_NAME: &str = "bd71828-pwrkey";
const POWER_STATE: &str = "/sys/power/state";
const EVENT_BYTES: usize = 16;
const READ_CHUNK_EVENTS: usize = 16;

#[derive(Debug)]
pub enum PowerError {
    NoPowerKey,
    SuspendUnsupported,
    SyncFailed,
    Io(io::Error),
}

impl fmt::Display for PowerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPowerKey => formatter.write_str("no bd71828-pwrkey input device was found"),
            Self::SuspendUnsupported => {
                formatter.write_str("the kernel does not advertise mem in /sys/power/state")
            }
            Self::SyncFailed => formatter.write_str("sync failed before suspend"),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for PowerError {}

impl From<io::Error> for PowerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A release of the physical power button.
///
/// Sleeping on release avoids carrying the press that requested sleep into the
/// wake transition as a pending wake interrupt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerButtonReleased;

pub struct PowerButton {
    releases: Option<Receiver<PowerButtonReleased>>,
}

impl PowerButton {
    /// Finds the PMIC power-key input by name and starts watching it.
    ///
    /// # Errors
    ///
    /// Returns an I/O error or [`PowerError::NoPowerKey`] when the named evdev
    /// source is unavailable.
    pub fn open() -> Result<Self, PowerError> {
        let path = find_power_key(Path::new(INPUT_DIR)).ok_or(PowerError::NoPowerKey)?;
        Self::open_path(&path)
    }

    fn open_path(path: &Path) -> Result<Self, PowerError> {
        let file = File::open(path)?;
        let (sender, releases) = mpsc::channel();
        thread::Builder::new()
            .name("power-button".to_owned())
            .spawn(move || read_power_key(file, &sender))?;
        Ok(Self {
            releases: Some(releases),
        })
    }

    /// Hands the release stream to a runtime multiplexing several event sources.
    pub fn take_events(&mut self) -> Option<Receiver<PowerButtonReleased>> {
        self.releases.take()
    }
}

/// The kernel system-suspend endpoint.
pub struct SystemSuspend {
    state: PathBuf,
}

impl SystemSuspend {
    /// Refuses construction unless suspend-to-RAM is advertised.
    ///
    /// # Errors
    ///
    /// Returns an I/O error or [`PowerError::SuspendUnsupported`] when the
    /// kernel does not advertise the `mem` state.
    pub fn open() -> Result<Self, PowerError> {
        Self::open_path(Path::new(POWER_STATE))
    }

    fn open_path(path: &Path) -> Result<Self, PowerError> {
        let states = fs::read_to_string(path)?;
        if !states.split_ascii_whitespace().any(|state| state == "mem") {
            return Err(PowerError::SuspendUnsupported);
        }
        Ok(Self {
            state: path.to_path_buf(),
        })
    }

    /// Flushes filesystems and enters suspend-to-RAM.
    ///
    /// The write returns only after the kernel has resumed userspace, so a
    /// successful return is also the resume boundary for the caller.
    ///
    /// # Errors
    ///
    /// Returns an error when filesystem synchronisation fails or the kernel
    /// rejects the suspend request.
    pub fn suspend(&self) -> Result<(), PowerError> {
        let status = Command::new("/bin/sync").status()?;
        if !status.success() {
            return Err(PowerError::SyncFailed);
        }
        self.write_state()
    }

    fn write_state(&self) -> Result<(), PowerError> {
        let mut state = OpenOptions::new().write(true).open(&self.state)?;
        state.write_all(b"mem\n")?;
        state.flush()?;
        Ok(())
    }
}

fn read_power_key(mut file: File, sender: &mpsc::Sender<PowerButtonReleased>) {
    let mut buffer = [0_u8; EVENT_BYTES * READ_CHUNK_EVENTS];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        for event in buffer[..read].chunks_exact(EVENT_BYTES) {
            if power_button_released(event) && sender.send(PowerButtonReleased).is_err() {
                return;
            }
        }
    }
}

fn power_button_released(bytes: &[u8]) -> bool {
    InputEvent32::decode(bytes).is_some_and(|event| {
        event.kind == input::EV_KEY && event.code == input::KEY_POWER && event.value == 0
    })
}

fn find_power_key(directory: &Path) -> Option<PathBuf> {
    let mut nodes = fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect::<Vec<_>>();
    nodes.sort();
    nodes.into_iter().find(|path| {
        File::open(path).is_ok_and(|file| {
            input::device_name(&file).is_ok_and(|name| name.trim() == POWER_KEY_NAME)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{power_button_released, PowerError, SystemSuspend};
    use std::fs;

    fn event(kind: u16, code: u16, value: i32) -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        bytes[8..10].copy_from_slice(&kind.to_le_bytes());
        bytes[10..12].copy_from_slice(&code.to_le_bytes());
        bytes[12..16].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    #[test]
    fn only_a_power_button_release_requests_sleep() {
        assert!(power_button_released(&event(1, 116, 0)));
        assert!(!power_button_released(&event(1, 116, 1)));
        assert!(!power_button_released(&event(1, 35, 0)));
        assert!(!power_button_released(&event(3, 116, 0)));
    }

    #[test]
    fn suspend_refuses_a_kernel_without_mem() {
        let path = std::env::temp_dir().join(format!("cobalt-power-state-{}", std::process::id()));
        fs::write(&path, b"freeze\n").expect("write states");
        assert!(matches!(
            SystemSuspend::open_path(&path),
            Err(PowerError::SuspendUnsupported)
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn the_suspend_write_is_exactly_mem() {
        let path = std::env::temp_dir().join(format!("cobalt-power-mem-{}", std::process::id()));
        fs::write(&path, b"mem\n").expect("write states");
        let suspend = SystemSuspend::open_path(&path).expect("mem is supported");
        suspend.write_state().expect("write suspend state");
        assert_eq!(fs::read(&path).expect("read state"), b"mem\n");
        let _ = fs::remove_file(path);
    }
}
