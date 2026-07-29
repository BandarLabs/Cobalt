//! The sleep-cover magnet, as an ordinary input the runtime can watch.
//!
//! The Kobo has a hall sensor behind one edge of the bezel. A magnet near it
//! closes a contact, and the kernel publishes that on the `gpio-keys` evdev
//! node as `EV_KEY` code 35: a press when the magnet arrives, a release when it
//! leaves. That is the whole of the hardware interface.
//!
//! Three properties of this node decide the shape of everything below.
//!
//! **It must never be grabbed.** `crate::input` takes `EVIOCGRAB` on the touch
//! panel, and explains at length why that is safe there. It is not safe here.
//! The stock reader holds this node open to know when to sleep, and taking it
//! exclusively would leave it believing the cover is in whatever state it last
//! saw. evdev broadcasts to every open client, so reading alongside the reader
//! costs nothing and breaks nothing.
//!
//! **Edges are not the state.** Opening the node while the magnet is already in
//! place produces no event, and neither does a magnet that arrives during
//! suspend. Anything that starts by waiting for an edge starts by being wrong
//! for an unbounded time. So a session asks the kernel for the current state
//! with `EVIOCGKEY` at open, and treats events only as changes to it.
//!
//! **The power button is elsewhere.** On this hardware `gpio-keys` carries
//! exactly one key, and `bd71828-pwrkey` is a separate node. There is no need
//! to filter anything off this stream, and code that assumed otherwise would be
//! guarding against a case that cannot arise here.

use crate::touch::InputEvent32;
use kobo_abi::input;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

/// One evdev event is 16 bytes on this 32-bit target. It is 24 on a 64-bit
/// host, so a decoder copied from a desktop example reads pure noise here.
const EVENT_BYTES: usize = 16;
const READ_CHUNK_EVENTS: usize = 16;

/// The name the kernel gives the node the hall sensor reports on.
const SENSOR_NAME: &str = "gpio-keys";

/// Where to look for it. The node number is stable on this firmware, but the
/// name is what actually identifies it, so the whole directory is searched
/// rather than trusting the number.
const INPUT_DIR: &str = "/dev/input";

/// Whether a magnet is in front of the sensor.
///
/// Named for the magnet rather than for the cover because a cover is only the
/// most common thing to hold one. The sensor cannot tell a cover from a fridge
/// magnet, and an API that claims otherwise is lying about what it measured.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Magnet {
    /// A magnet is in front of the sensor.
    Present,
    /// No magnet is in front of the sensor.
    #[default]
    Absent,
}

impl Magnet {
    /// The state a key press or release means.
    ///
    /// evdev sends 0 for a release, 1 for a press and 2 for an auto-repeat.
    /// A repeat is the same state as the press that started it, not a new one.
    const fn from_value(value: i32) -> Option<Self> {
        match value {
            0 => Some(Self::Absent),
            1 | 2 => Some(Self::Present),
            _ => None,
        }
    }

    #[must_use]
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

impl fmt::Display for Magnet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Present => "present",
            Self::Absent => "absent",
        })
    }
}

#[derive(Debug)]
pub enum CoverError {
    /// No input node reports itself as the hall sensor.
    NoSensor,
    Io(io::Error),
}

impl fmt::Display for CoverError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSensor => write!(
                formatter,
                "no {SENSOR_NAME} input device carries the cover key, so this reader has no hall sensor"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CoverError {}

impl From<io::Error> for CoverError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A running watch on the hall sensor.
///
/// Reading blocks, so the read lives on its own thread and changes arrive on a
/// channel. Only *changes* are sent: the sensor bounces while a magnet is moved
/// slowly past it, and a listener that acts on every edge would act several
/// times for one deliberate gesture.
#[derive(Debug)]
pub struct CoverSensor {
    magnet: Magnet,
    changes: Receiver<Magnet>,
}

impl CoverSensor {
    /// Starts watching, beginning from the state the kernel reports now.
    ///
    /// # Errors
    ///
    /// Returns [`CoverError::NoSensor`] when no input node carries the cover
    /// key, which is the honest answer on a reader without the hardware.
    pub fn open() -> Result<Self, CoverError> {
        let path = find_sensor(Path::new(INPUT_DIR)).ok_or(CoverError::NoSensor)?;
        Self::open_path(&path)
    }

    /// The same, against a named node, so a caller can point at a fake one.
    ///
    /// # Errors
    ///
    /// Returns the kernel error when the node cannot be opened or queried.
    pub fn open_path(path: &Path) -> Result<Self, CoverError> {
        let file = File::open(path)?;
        // Ask rather than wait. See the module comment: an edge that happened
        // before we opened is an edge we will never be told about.
        let magnet = if input::key_is_pressed(&file, input::KEY_COVER)? {
            Magnet::Present
        } else {
            Magnet::Absent
        };
        let (sender, changes) = mpsc::channel();
        thread::Builder::new()
            .name("cover".to_owned())
            .spawn(move || read_forever(file, magnet, &sender))?;
        Ok(Self { magnet, changes })
    }

    /// The last known state, without waiting.
    #[must_use]
    pub const fn magnet(&self) -> Magnet {
        self.magnet
    }

    /// Takes the next change, or `None` when nothing has changed.
    ///
    /// One change per call, in the order they happened.
    ///
    /// This used to drain the channel and keep only the newest state, on the
    /// reasoning that a slow caller wants to know where the magnet is now
    /// rather than replay where it has been. That reasoning is wrong, and
    /// wrong in the way that makes the sensor look broken.
    ///
    /// A magnet waved past the bezel arrives and leaves about two hundred
    /// milliseconds apart. Both changes land in the channel between two polls,
    /// draining keeps only the last one, the last one is `Absent`, and
    /// `Absent` is where the magnet already was. The change is filtered out as
    /// no change at all. Every wave was silently swallowed: the kernel logged
    /// the edges, a raw read of the node showed both events, and the
    /// application on the panel was never told about either.
    ///
    /// The sender only ever emits genuine transitions, so nothing here needs
    /// to collapse anything. The caller polls in a loop and drains a backlog
    /// within a few iterations, which is what an application counting edges,
    /// or waiting for a tap, actually needs.
    pub fn poll(&mut self) -> Option<Magnet> {
        while let Ok(magnet) = self.changes.try_recv() {
            if magnet != self.magnet {
                self.magnet = magnet;
                return Some(magnet);
            }
        }
        None
    }

    /// Waits up to `timeout` for the state to change.
    #[must_use]
    pub fn wait(&mut self, timeout: Duration) -> Option<Magnet> {
        match self.changes.recv_timeout(timeout) {
            Ok(magnet) => {
                let changed = magnet != self.magnet;
                self.magnet = magnet;
                changed.then_some(magnet)
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => None,
        }
    }
}

/// Reads until the node closes, sending only changes.
///
/// A send failure ends the loop: the only way the channel closes is the
/// [`CoverSensor`] being dropped, and there is then nobody left to tell.
fn read_forever(mut file: File, initial: Magnet, sender: &mpsc::Sender<Magnet>) {
    let mut magnet = initial;
    let mut buffer = [0_u8; EVENT_BYTES * READ_CHUNK_EVENTS];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        for event in buffer[..read].chunks_exact(EVENT_BYTES) {
            let Some(next) = cover_state(event) else {
                continue;
            };
            if next != magnet {
                magnet = next;
                if sender.send(magnet).is_err() {
                    return;
                }
            }
        }
    }
}

/// The state one raw event means, or `None` when it is about something else.
fn cover_state(bytes: &[u8]) -> Option<Magnet> {
    let event = InputEvent32::decode(bytes)?;
    if event.kind != input::EV_KEY || event.code != input::KEY_COVER {
        return None;
    }
    Magnet::from_value(event.value)
}

/// Finds the node that carries the cover key.
///
/// Identified by name rather than by number. `event0` is the sensor on this
/// firmware, but that is an observation about one device, and opening the wrong
/// node here would mean watching the touch panel for a magnet.
fn find_sensor(directory: &Path) -> Option<PathBuf> {
    let mut nodes = std::fs::read_dir(directory)
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
            input::device_name(&file).is_ok_and(|name| name.trim() == SENSOR_NAME)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{cover_state, CoverSensor, Magnet};
    use std::sync::mpsc;

    fn event(kind: u16, code: u16, value: i32) -> [u8; 16] {
        let mut bytes = [0_u8; 16];
        bytes[8..10].copy_from_slice(&kind.to_le_bytes());
        bytes[10..12].copy_from_slice(&code.to_le_bytes());
        bytes[12..16].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    #[test]
    fn a_press_is_a_magnet_arriving_and_a_release_is_it_leaving() {
        assert_eq!(cover_state(&event(1, 35, 1)), Some(Magnet::Present));
        assert_eq!(cover_state(&event(1, 35, 0)), Some(Magnet::Absent));
    }

    /// An auto-repeat is the kernel restating a press, not a second one. A
    /// decoder that treated it as new would report a magnet arriving twice
    /// without it ever having left.
    #[test]
    fn an_auto_repeat_is_the_same_state_as_the_press_that_started_it() {
        assert_eq!(cover_state(&event(1, 35, 2)), Some(Magnet::Present));
    }

    /// A wave is two changes, and both of them are the point.
    ///
    /// This is the bug that made the sensor look dead on hardware. A magnet
    /// walked past the bezel is present for about two hundred milliseconds,
    /// so the arrival and the departure both queue up between two polls.
    /// `poll` used to drain the queue and keep the newest, which is `Absent`,
    /// which is where the magnet already was, so it reported nothing. The
    /// kernel had logged both edges and a raw read of the node had shown both
    /// events; only the application heard nothing.
    #[test]
    fn a_magnet_that_arrives_and_leaves_between_polls_reports_both() {
        let (sender, changes) = mpsc::channel();
        let mut sensor = CoverSensor {
            magnet: Magnet::Absent,
            changes,
        };
        sender.send(Magnet::Present).expect("queue the arrival");
        sender.send(Magnet::Absent).expect("queue the departure");

        assert_eq!(sensor.poll(), Some(Magnet::Present), "the magnet arrived");
        assert_eq!(sensor.poll(), Some(Magnet::Absent), "and then it left");
        assert_eq!(sensor.poll(), None, "and nothing else happened");
        assert_eq!(sensor.magnet(), Magnet::Absent);
    }

    /// A restated state is not a change, whoever restates it.
    #[test]
    fn a_repeat_of_the_state_already_held_is_not_reported() {
        let (sender, changes) = mpsc::channel();
        let mut sensor = CoverSensor {
            magnet: Magnet::Absent,
            changes,
        };
        sender.send(Magnet::Absent).expect("queue a repeat");
        assert_eq!(sensor.poll(), None);
        sender.send(Magnet::Present).expect("queue a real change");
        assert_eq!(sensor.poll(), Some(Magnet::Present));
    }

    #[test]
    fn events_about_anything_else_are_ignored_rather_than_guessed_at() {
        assert_eq!(cover_state(&event(1, 116, 1)), None, "power button");
        assert_eq!(cover_state(&event(3, 35, 1)), None, "an absolute axis");
        assert_eq!(cover_state(&event(0, 0, 0)), None, "a report separator");
        assert_eq!(cover_state(&[0_u8; 24]), None, "a 64-bit sized event");
        assert_eq!(cover_state(&[0_u8; 8]), None, "a truncated event");
    }
}
