//! The physical buttons and the orientation channel.
//!
//! On NTX hardware the `gpio-keys` node carries three unrelated things: the
//! page-turn keys, the power button, and the kernel's digested accelerometer
//! verdicts (`EV_MSC`/`MSC_RAW` with one value per orientation). One session
//! reads the node and hands all of them to the runtime, which decides what
//! each means.
//!
//! Unlike the touch panel this device is never grabbed. Inside a panel
//! session nothing else consumes buttons — the stock reader is stopped, and
//! while it runs it holds its own exclusive grab, which is why these keys
//! appear dead outside a session (measured on the Libra 2: two captures under
//! the reader saw nothing, the same capture inside a session saw everything).
//! Not grabbing also avoids the Clara BW kernel's grab bug, where `EVIOCGRAB`
//! succeeds and then starves the grabbing client, and it leaves the
//! sleep-cover watcher — a second, independent open of the same node — its
//! own full copy of every event.

use crate::touch::InputEvent32;
use kobo_abi::input;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// One evdev event is 16 bytes on this 32-bit target.
const EVENT_BYTES: usize = 16;
const READ_CHUNK_EVENTS: usize = 64;

/// The device name this module owns. Every NTX Kobo ships it.
const DEVICE_NAME: &str = "gpio-keys";

/// A physical button, named by what it is rather than what it means.
///
/// Which page key means "forward" depends on how the reader is held, so that
/// assignment happens where the pose is known, not here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    /// The page key the kernel reports as code 193.
    Page193,
    /// The page key the kernel reports as code 194.
    Page194,
    Power,
}

/// The kernel's verdict on how the device is held, straight from the NTX
/// `MSC_RAW` channel. Five of these six were captured from a Libra 2 on
/// 2026-08-23 by turning it about: both portrait poses, both landscape poses
/// and face-up. Face-down completes the contiguous run and is the one pose a
/// reader held in the hands never reaches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Orientation {
    PortraitDown,
    PortraitUp,
    LandscapeRight,
    LandscapeLeft,
    FaceDown,
    FaceUp,
}

impl Orientation {
    #[must_use]
    const fn from_msc_raw(value: i32) -> Option<Self> {
        match value {
            0x17 => Some(Self::PortraitDown),
            0x18 => Some(Self::PortraitUp),
            0x19 => Some(Self::LandscapeRight),
            0x1a => Some(Self::LandscapeLeft),
            0x1b => Some(Self::FaceDown),
            0x1c => Some(Self::FaceUp),
            _ => None,
        }
    }
}

/// Everything the button device reports that the runtime cares about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpioEvent {
    Button { button: Button, pressed: bool },
    Orientation(Orientation),
}

/// Turns one raw event into a typed one, or nothing.
///
/// The sleep-cover key (code 35) is deliberately not claimed: the cover
/// watcher owns it through its own open of the same node. Auto-repeat
/// (value 2) is dropped for now — no capture has produced one, and
/// hold-to-turn is a decision for the runtime once it exists.
#[must_use]
pub fn decode(event: InputEvent32) -> Option<GpioEvent> {
    match (event.kind, event.code) {
        (input::EV_KEY, input::KEY_PAGE_193 | input::KEY_PAGE_194 | input::KEY_POWER) => {
            let button = match event.code {
                input::KEY_PAGE_193 => Button::Page193,
                input::KEY_PAGE_194 => Button::Page194,
                _ => Button::Power,
            };
            match event.value {
                0 => Some(GpioEvent::Button {
                    button,
                    pressed: false,
                }),
                1 => Some(GpioEvent::Button {
                    button,
                    pressed: true,
                }),
                _ => None,
            }
        }
        (input::EV_MSC, input::MSC_RAW) => {
            Orientation::from_msc_raw(event.value).map(GpioEvent::Orientation)
        }
        _ => None,
    }
}

/// Finds the `gpio-keys` event node, or nothing on hardware without one.
#[must_use]
pub fn discover_buttons_path() -> Option<PathBuf> {
    let content = std::fs::read_to_string("/proc/bus/input/devices").ok()?;
    discover_buttons_path_from(&content)
}

fn discover_buttons_path_from(content: &str) -> Option<PathBuf> {
    content.split("\n\n").find_map(|block| {
        let name_matches = block
            .lines()
            .find(|line| line.starts_with("N: Name="))
            .is_some_and(|line| line.contains(DEVICE_NAME));
        if !name_matches {
            return None;
        }
        let handlers = block
            .lines()
            .find(|line| line.starts_with("H: Handlers="))?;
        let event = handlers
            .strip_prefix("H: Handlers=")?
            .split_whitespace()
            .find(|handler| handler.starts_with("event"))?;
        Some(Path::new("/dev/input").join(event))
    })
}

#[derive(Debug)]
pub enum GpioError {
    /// The device at this path is not the button device.
    WrongDevice {
        found: String,
    },
    Io(io::Error),
}

impl fmt::Display for GpioError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDevice { found } => write!(
                formatter,
                "button device is {found:?}, but this module requires {DEVICE_NAME:?}"
            ),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for GpioError {}

impl From<io::Error> for GpioError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A reader on the button device for the lifetime of a panel session.
///
/// Nothing to release: no grab is taken, so dropping this (or the process
/// dying) leaves the device exactly as it was found.
pub struct GpioSession {
    events: Option<Receiver<GpioEvent>>,
}

impl GpioSession {
    /// Opens the button device and starts decoding.
    ///
    /// # Errors
    ///
    /// Returns an error when the device cannot be opened or is not
    /// `gpio-keys`.
    pub fn acquire(path: &Path) -> Result<Self, GpioError> {
        let device = File::open(path)?;
        let name = input::device_name(&device)?;
        if name != DEVICE_NAME {
            return Err(GpioError::WrongDevice { found: name });
        }
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = device;
            let mut buffer = [0_u8; EVENT_BYTES * READ_CHUNK_EVENTS];
            loop {
                let Ok(read) = reader.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                for chunk in buffer[..read].chunks_exact(EVENT_BYTES) {
                    let Some(event) = InputEvent32::decode(chunk).and_then(decode) else {
                        continue;
                    };
                    if sender.send(event).is_err() {
                        return;
                    }
                }
            }
        });
        Ok(Self {
            events: Some(events),
        })
    }

    /// Hands the event stream to a caller that multiplexes several sources.
    ///
    /// Returns `None` if the stream was already taken.
    pub fn take_events(&mut self) -> Option<Receiver<GpioEvent>> {
        self.events.take()
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, discover_buttons_path_from, Button, GpioEvent, Orientation};
    use crate::touch::InputEvent32;
    use std::path::Path;

    const fn key(code: u16, value: i32) -> InputEvent32 {
        InputEvent32 {
            kind: 1,
            code,
            value,
        }
    }

    #[test]
    fn a_page_key_press_and_release_decode_as_that_button() {
        // Codes as captured on the Libra 2, 2026-08-23.
        assert_eq!(
            decode(key(193, 1)),
            Some(GpioEvent::Button {
                button: Button::Page193,
                pressed: true
            })
        );
        assert_eq!(
            decode(key(194, 0)),
            Some(GpioEvent::Button {
                button: Button::Page194,
                pressed: false
            })
        );
    }

    #[test]
    fn the_power_button_decodes() {
        assert_eq!(
            decode(key(116, 1)),
            Some(GpioEvent::Button {
                button: Button::Power,
                pressed: true
            })
        );
    }

    #[test]
    fn auto_repeat_is_dropped_until_someone_wants_it() {
        assert_eq!(decode(key(193, 2)), None);
    }

    #[test]
    fn the_sleep_cover_key_is_left_to_the_cover_watcher() {
        assert_eq!(decode(key(35, 1)), None);
    }

    #[test]
    fn every_captured_orientation_value_decodes() {
        // The five values the Libra 2 produced under rotation, plus
        // face-down, which that capture never reached.
        let cases = [
            (0x17, Orientation::PortraitDown),
            (0x18, Orientation::PortraitUp),
            (0x19, Orientation::LandscapeRight),
            (0x1a, Orientation::LandscapeLeft),
            (0x1b, Orientation::FaceDown),
            (0x1c, Orientation::FaceUp),
        ];
        for (value, expected) in cases {
            assert_eq!(
                decode(InputEvent32 {
                    kind: 4,
                    code: 3,
                    value
                }),
                Some(GpioEvent::Orientation(expected))
            );
        }
    }

    #[test]
    fn an_unknown_orientation_value_is_dropped_not_guessed() {
        assert_eq!(
            decode(InputEvent32 {
                kind: 4,
                code: 3,
                value: 99
            }),
            None
        );
    }

    #[test]
    fn syn_reports_decode_to_nothing() {
        assert_eq!(
            decode(InputEvent32 {
                kind: 0,
                code: 0,
                value: 0
            }),
            None
        );
    }

    #[test]
    fn the_button_node_is_found_by_name() {
        let fixture = "I: Bus=0019 Vendor=0000 Product=0000 Version=0000\n\
N: Name=\"gpio-keys\"\n\
H: Handlers=event0\n\
B: EV=b\n\n\
N: Name=\"Elan Touchscreen\"\n\
H: Handlers=event1\n";
        assert_eq!(
            discover_buttons_path_from(fixture).as_deref(),
            Some(Path::new("/dev/input/event0"))
        );
    }
}
