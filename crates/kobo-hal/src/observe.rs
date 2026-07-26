//! Read-only touch observation.
//!
//! The touch device is opened read-only and is deliberately never grabbed with
//! `EVIOCGRAB`, so the stock reader keeps receiving every event exactly as it
//! normally would. Nothing here writes to the device, the framebuffer, or any
//! file, which is why this is allowed to run while Nickel owns the screen.
//!
//! Its purpose is to prove the profile's touch-to-display transform against
//! physical touches at known locations. Dimensional agreement between the touch
//! ranges and the screen geometry only shows that the axes are swapped; it
//! cannot show which direction each axis runs. That needs a real finger.

use crate::touch::{InputEvent32, TouchDecoder, TouchEvent};
use kobo_abi::input;
use kobo_profile::DeviceProfile;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// One evdev event is 16 bytes on this 32-bit target.
const EVENT_BYTES: usize = 16;
/// Read a whole report's worth at a time without unbounded buffering.
const READ_CHUNK_EVENTS: usize = 64;
/// An upper bound on how long an observation may run, so a caller cannot ask
/// this to sit on the touch device indefinitely.
pub const MAXIMUM_OBSERVE_SECONDS: u64 = 120;

#[derive(Debug)]
pub struct ObserveError {
    context: &'static str,
    detail: String,
}

impl ObserveError {
    fn new(context: &'static str, error: &io::Error) -> Self {
        Self {
            context,
            detail: error.to_string(),
        }
    }

    fn message(context: &'static str, detail: impl Into<String>) -> Self {
        Self {
            context,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ObserveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.context, self.detail)
    }
}

/// A decoded touch together with the raw controller values that produced it.
///
/// Both are reported because the raw pair is the evidence and the display pair
/// is the claim under test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TouchObservation {
    pub event: TouchEvent,
    pub raw_x: i32,
    pub raw_y: i32,
}

impl fmt::Display for TouchObservation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, x, y) = match self.event {
            TouchEvent::Down { x, y } => ("down", x, y),
            TouchEvent::Move { x, y } => ("move", x, y),
            TouchEvent::Up { x, y } => ("up", x, y),
        };
        write!(
            formatter,
            "{kind} raw=({},{}) display=({x},{y})",
            self.raw_x, self.raw_y
        )
    }
}

/// Watches the touch device read-only for at most `duration`, reporting each
/// decoded touch to `sink` as it happens.
///
/// Returns the number of touches reported. The reading thread is left blocked
/// in `read` when the deadline passes; that is safe precisely because this path
/// holds no grab and owns no hardware state that must be unwound.
///
/// # Errors
///
/// Returns an error when the requested window exceeds
/// [`MAXIMUM_OBSERVE_SECONDS`], the touch device cannot be opened read-only,
/// the reader thread cannot be started, or the event stream is not a whole
/// number of decodable events.
pub fn observe_touch(
    path: &Path,
    profile: &DeviceProfile,
    duration: Duration,
    mut sink: impl FnMut(TouchObservation),
) -> Result<usize, ObserveError> {
    if duration > Duration::from_secs(MAXIMUM_OBSERVE_SECONDS) {
        return Err(ObserveError::message(
            "touch observation window",
            format!("at most {MAXIMUM_OBSERVE_SECONDS} seconds are allowed"),
        ));
    }
    let file = File::open(path)
        .map_err(|error| ObserveError::new("open touch input read-only", &error))?;

    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("kobo-touch-observe".to_owned())
        .spawn(move || read_events(file, &sender))
        .map_err(|error| ObserveError::new("start touch reader thread", &error))?;

    let deadline = Instant::now() + duration;
    let mut decoder = TouchDecoder::default();
    let mut last_raw: Option<(i32, i32)> = None;
    let mut reported = 0usize;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(Ok(event)) => {
                match (event.kind, event.code) {
                    (input::EV_ABS, input::ABS_MT_POSITION_X) => {
                        last_raw = Some((event.value, last_raw.map_or(0, |(_, y)| y)));
                    }
                    (input::EV_ABS, input::ABS_MT_POSITION_Y) => {
                        last_raw = Some((last_raw.map_or(0, |(x, _)| x), event.value));
                    }
                    _ => {}
                }
                if let Some(touch) = decoder.push(event, profile) {
                    let (raw_x, raw_y) = last_raw.unwrap_or((-1, -1));
                    reported += 1;
                    sink(TouchObservation {
                        event: touch,
                        raw_x,
                        raw_y,
                    });
                }
            }
            Ok(Err(error)) => return Err(ObserveError::message("read touch events", error)),
            // Either the window closed or the reader stopped; both mean there
            // is nothing further to observe.
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(reported)
}

/// Blocking reader. Sends decoded events until the receiver goes away.
fn read_events(mut file: File, sender: &mpsc::Sender<Result<InputEvent32, String>>) {
    let mut buffer = [0u8; EVENT_BYTES * READ_CHUNK_EVENTS];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => return,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                let _ignored = sender.send(Err(error.to_string()));
                return;
            }
        };
        // A short read that is not a whole number of events means the stream is
        // not what this code believes it is, so stop rather than guess.
        if read % EVENT_BYTES != 0 {
            let _ignored = sender.send(Err(format!(
                "read {read} bytes, which is not a whole number of {EVENT_BYTES}-byte events"
            )));
            return;
        }
        for chunk in buffer[..read].chunks_exact(EVENT_BYTES) {
            let Some(event) = InputEvent32::decode(chunk) else {
                let _ignored = sender.send(Err("could not decode an input event".to_owned()));
                return;
            };
            if sender.send(Ok(event)).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{observe_touch, TouchObservation, MAXIMUM_OBSERVE_SECONDS};
    use crate::touch::TouchEvent;
    use kobo_profile::CLARA_BW_391;
    use std::path::Path;
    use std::time::Duration;

    #[test]
    fn an_over_long_window_is_refused_before_the_device_is_opened() {
        let error = observe_touch(
            Path::new("/definitely/not/a/device"),
            &CLARA_BW_391,
            Duration::from_secs(MAXIMUM_OBSERVE_SECONDS + 1),
            |_| unreachable!("no touch can be reported"),
        )
        .expect_err("an over-long window must be refused");
        assert!(
            error.to_string().contains("at most"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_missing_touch_device_is_an_error_rather_than_a_hang() {
        let error = observe_touch(
            Path::new("/definitely/not/a/device"),
            &CLARA_BW_391,
            Duration::from_millis(1),
            |_| unreachable!("no touch can be reported"),
        )
        .expect_err("a missing device must fail");
        assert!(
            error.to_string().contains("open touch input read-only"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_observation_reports_both_the_evidence_and_the_claim() {
        let shown = TouchObservation {
            event: TouchEvent::Down { x: 10, y: 20 },
            raw_x: 20,
            raw_y: 1061,
        }
        .to_string();
        assert_eq!(shown, "down raw=(20,1061) display=(10,20)");
    }

    #[test]
    fn the_transform_under_test_maps_the_corners_as_documented() {
        // These are the four claims a physical touch has to confirm.
        let profile = &CLARA_BW_391;
        assert_eq!(profile.touch_to_display(0, 0), Some((1071, 0)));
        assert_eq!(profile.touch_to_display(0, 1071), Some((0, 0)));
        assert_eq!(profile.touch_to_display(1447, 0), Some((1071, 1447)));
        assert_eq!(profile.touch_to_display(1447, 1071), Some((0, 1447)));
    }
}
