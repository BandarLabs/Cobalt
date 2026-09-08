//! Exclusive touch ownership.
//!
//! While an application is on screen the runtime must be the only reader of the
//! touch device. Otherwise the stock reader also sees every tap and navigates
//! its own library underneath us, repainting the panel we are drawing on.
//!
//! `EVIOCGRAB` is unusually safe for something this consequential: the grab
//! belongs to the open file description, so the kernel drops it when the file is
//! closed or the process dies for any reason at all, including `SIGKILL`. There
//! is no way to leak an exclusive grab past the lifetime of the process, and
//! nothing a reboot would need to undo. Contrast that with the reader itself,
//! which stays stopped until something restarts it.
//!
//! The one rule that needs care is *when* to grab. Taking the device while a
//! finger is down means the stock reader never sees the matching release and is
//! left believing a contact is still active. So a grab is only ever taken from a
//! quiescent state, and the same applies in reverse on release.

use crate::touch::{InputEvent32, TouchDecoder, TouchEvent};
use kobo_abi::input;
use kobo_profile::PanelPose;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

/// One evdev event is 16 bytes on this 32-bit target.
const EVENT_BYTES: usize = 16;
const READ_CHUNK_EVENTS: usize = 64;
/// How long the device must report no active contact before it may be grabbed.
const QUIESCENT_WINDOW: Duration = Duration::from_millis(250);
/// How long to wait for the panel to become quiescent before giving up.
const QUIESCENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum InputError {
    /// The device at this path is not the touch panel this profile describes.
    WrongDevice {
        expected: String,
        found: String,
    },
    /// A finger stayed on the panel, so a grab was never safe to take.
    NeverQuiescent,
    /// Another process already holds an exclusive grab.
    AlreadyGrabbed,
    Io(io::Error),
}

impl fmt::Display for InputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongDevice { expected, found } => write!(
                formatter,
                "touch device is {found:?}, but this profile requires {expected:?}"
            ),
            Self::NeverQuiescent => write!(
                formatter,
                "the panel never became quiescent; grabbing while a finger is down would strand the reader with a contact it never sees released"
            ),
            Self::AlreadyGrabbed => {
                write!(formatter, "another process already owns the touch device")
            }
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for InputError {}

impl From<io::Error> for InputError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A read-only view of the input decoder's latest contact state.
#[derive(Clone, Default)]
pub struct Quiescence(Arc<AtomicBool>);
impl Quiescence {
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
struct DecoderLifetime(Arc<AtomicBool>);
impl Drop for DecoderLifetime {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Exclusive ownership of the touch panel for the lifetime of this value.
pub struct TouchSession {
    device: File,
    events: Option<Receiver<TouchEvent>>,
    released: bool,
    grabbed: bool,
    quiet: Arc<AtomicBool>,
}

impl TouchSession {
    /// Opens the profile's touch device, waits for it to go quiet, and takes an
    /// exclusive grab.
    ///
    /// # Errors
    ///
    /// Returns an error when the device is not the expected panel, never
    /// becomes quiescent, or is already owned by another process.
    pub fn acquire(path: &Path, pose: PanelPose<'static>) -> Result<Self, InputError> {
        let device = File::open(path)?;
        let name = input::device_name(&device)?;
        if name != pose.profile().touch_name {
            return Err(InputError::WrongDevice {
                expected: pose.profile().touch_name.to_owned(),
                found: name,
            });
        }

        // Exactly one thread ever reads this device. An earlier version used a
        // second, temporary reader to detect quiescence and left it running;
        // the two threads then raced for events and each received a fraction of
        // every report. A decoder that sees some of a report and not the rest
        // never assembles a complete touch, so taps silently did nothing.
        let reader = device.try_clone()?;
        let (sender, events) = mpsc::channel();
        let quiescent = Arc::new(AtomicBool::new(true));
        let decoder_quiescent = Arc::clone(&quiescent);
        // Raw tracing is opt-in because it is the only way to tell a device
        // that reports nothing from a decoder that discards everything.
        let trace = std::env::var_os("KOBO_TOUCH_TRACE").is_some();
        thread::spawn(move || {
            let _health = DecoderLifetime(Arc::clone(&decoder_quiescent));
            let mut reader = reader;
            let mut decoder = TouchDecoder::default();
            let mut buffer = [0_u8; EVENT_BYTES * READ_CHUNK_EVENTS];
            loop {
                let Ok(read) = reader.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                for chunk in buffer[..read].chunks_exact(EVENT_BYTES) {
                    let Some(event) = InputEvent32::decode(chunk) else {
                        continue;
                    };
                    if trace {
                        println!(
                            "raw type={} code={} value={}",
                            event.kind, event.code, event.value
                        );
                    }
                    let touch = decoder.push(event, &pose);
                    if event.kind == input::EV_SYN
                        && matches!(event.code, input::SYN_REPORT | input::SYN_DROPPED)
                    {
                        decoder_quiescent.store(decoder.is_quiescent(), Ordering::Release);
                    }
                    if let Some(touch) = touch {
                        if sender.send(touch).is_err() {
                            return;
                        }
                    }
                    if decoder.needs_resynchronization() {
                        let snapshot = input::touch_snapshot(&reader);
                        if trace {
                            println!("touch resynchronization: {snapshot:?}");
                        }
                        decoder.resynchronize(snapshot.ok());
                    }
                    if event.kind == input::EV_SYN
                        && matches!(event.code, input::SYN_REPORT | input::SYN_DROPPED)
                    {
                        decoder_quiescent.store(decoder.is_quiescent(), Ordering::Release);
                    }
                }
            }
        });

        // Quiescence is simply the absence of any touch for a whole window,
        // observed on that same stream. Grabbing while a finger is down would
        // leave the stock reader waiting forever for a release it never sees.
        wait_for_quiescence(&events, &quiescent)?;

        // Measured on the Clara BW: EVIOCGRAB succeeds and then this kernel
        // stops delivering events to the grabbing client entirely. The same
        // binary reading the same node receives every report without it and
        // none with it, so the grab is off by default. It is not needed in the
        // first place, because a session only runs once the stock reader has
        // been stopped and nothing else reads the panel then. The switch stays
        // so other firmware can be retested rather than assumed.
        let grabbed = std::env::var_os("KOBO_TOUCH_GRAB").is_some();
        if grabbed {
            input::set_exclusive(&device, true).map_err(|error| {
                if error.raw_os_error() == Some(16) {
                    InputError::AlreadyGrabbed
                } else {
                    InputError::Io(error)
                }
            })?;
        }

        Ok(Self {
            device,
            events: Some(events),
            released: false,
            grabbed,
            quiet: quiescent,
        })
    }

    /// A live observation from the one input decoder, including dropped-report
    /// resynchronization. Hosts must treat false as an outstanding contact.
    #[must_use]
    pub fn quiescence(&self) -> Quiescence {
        Quiescence(Arc::clone(&self.quiet))
    }

    /// Waits up to `timeout` for the next touch.
    ///
    /// Returns `None` when nothing arrived in time, which is the normal idle
    /// case rather than an error.
    #[must_use]
    pub fn next_touch(&self, timeout: Duration) -> Option<TouchEvent> {
        self.events.as_ref()?.recv_timeout(timeout).ok()
    }

    /// Hands the touch stream to a caller that multiplexes several sources.
    ///
    /// A runtime waiting on both touches and application messages must block on
    /// a single channel; polling each in turn with a short timeout would keep
    /// the processor awake between events, which on a device that idles at zero
    /// power is a battery defect. Returns `None` if the stream was already
    /// taken.
    pub fn take_events(&mut self) -> Option<Receiver<TouchEvent>> {
        self.events.take()
    }

    /// Gives the touch device back to the stock reader.
    ///
    /// This is explicit rather than a `Drop` guard because release builds abort
    /// on panic and would never run one. Closing the file would release the
    /// grab anyway; doing it deliberately means the release is visible in the
    /// code that owns the handoff.
    ///
    /// # Errors
    ///
    /// Returns an error when the ungrab fails, which leaves the kernel to
    /// release the grab when the process exits.
    pub fn release(&mut self) -> Result<(), InputError> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        if self.grabbed {
            input::set_exclusive(&self.device, false)?;
        }
        Ok(())
    }
}

/// Blocks until no touch has been reported for [`QUIESCENT_WINDOW`].
fn wait_for_quiescence(
    events: &Receiver<TouchEvent>,
    quiescent: &AtomicBool,
) -> Result<(), InputError> {
    wait_for_quiescence_for(events, quiescent, QUIESCENT_WINDOW, QUIESCENT_TIMEOUT)
}

fn wait_for_quiescence_for(
    events: &Receiver<TouchEvent>,
    quiescent: &AtomicBool,
    window: Duration,
    timeout: Duration,
) -> Result<(), InputError> {
    let deadline = Instant::now() + timeout;
    loop {
        match events.recv_timeout(window) {
            Err(RecvTimeoutError::Timeout) if quiescent.load(Ordering::Acquire) => return Ok(()),
            Err(RecvTimeoutError::Disconnected) => {
                return Err(InputError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "touch reader stopped before quiescence was established",
                )))
            }
            Err(RecvTimeoutError::Timeout) | Ok(_) => {
                if Instant::now() >= deadline {
                    return Err(InputError::NeverQuiescent);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InputError, EVENT_BYTES, QUIESCENT_TIMEOUT, QUIESCENT_WINDOW};

    #[test]
    fn a_disconnected_decoder_never_reports_safe_input() {
        let quiet = super::Quiescence::default();
        assert!(!quiet.is_quiet());
        quiet.0.store(true, std::sync::atomic::Ordering::Release);
        let reader = super::DecoderLifetime(std::sync::Arc::clone(&quiet.0));
        assert!(quiet.is_quiet());
        drop(reader);
        assert!(!quiet.is_quiet());
    }

    #[test]
    fn an_evdev_event_is_sixteen_bytes_on_this_target() {
        // The device is 32-bit, so a timeval is two 32-bit words. Reading with
        // the host's 64-bit layout would misparse every event.
        assert_eq!(EVENT_BYTES, 16);
    }

    #[test]
    fn the_quiescent_window_is_shorter_than_the_timeout() {
        // Otherwise the first window would always exhaust the budget and a grab
        // could never be taken.
        assert!(QUIESCENT_WINDOW < QUIESCENT_TIMEOUT);
    }

    #[test]
    fn a_wrong_device_is_reported_with_both_names() {
        let error = InputError::WrongDevice {
            expected: "cyttsp5_mt".to_owned(),
            found: "something-else".to_owned(),
        };
        let message = error.to_string();
        assert!(message.contains("cyttsp5_mt"));
        assert!(message.contains("something-else"));
    }

    #[test]
    fn the_quiescence_error_explains_why_it_matters() {
        // This error exists to stop a specific bug, so the message says which.
        let message = InputError::NeverQuiescent.to_string();
        assert!(message.contains("released"));
    }

    #[test]
    fn a_busy_device_is_distinguished_from_other_errors() {
        // EBUSY means someone else holds the grab, which is actionable, unlike
        // a generic io error.
        assert_eq!(
            InputError::AlreadyGrabbed.to_string(),
            "another process already owns the touch device"
        );
    }
    #[test]
    fn silence_during_lost_input_does_not_count_as_quiescence() {
        let (_sender, receiver) = std::sync::mpsc::channel();
        let unknown = std::sync::atomic::AtomicBool::new(false);
        let result = super::wait_for_quiescence_for(
            &receiver,
            &unknown,
            std::time::Duration::from_millis(2),
            std::time::Duration::from_millis(10),
        );
        assert!(matches!(result, Err(InputError::NeverQuiescent)));
    }

    #[test]
    fn a_stopped_input_reader_is_not_a_quiet_panel() {
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(sender);
        let quiet = std::sync::atomic::AtomicBool::new(true);
        assert!(matches!(
            super::wait_for_quiescence(&receiver, &quiet),
            Err(InputError::Io(_))
        ));
    }
}
