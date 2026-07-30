//! Synthetic taps, for driving a device nobody is standing in front of.
//!
//! # Why this exists
//!
//! Everything else in this workspace could be checked without hardware and
//! the one thing that mattered could not: whether the screen a reader is
//! looking at responds where it appears to. The simulator answers that for the
//! renderer, because it runs the same one. It cannot answer it for the panel's
//! own touch transform, for a digitiser whose axes are swapped relative to the
//! display, or for a control that is reachable at 1072x1448 in a browser and
//! four millimetres off the glass in a case.
//!
//! # Why it writes to the input device rather than to the application
//!
//! A tap injected into the runtime would be a tap that skipped exactly the
//! machinery worth testing: the digitiser's coordinate space, the profile's
//! `display_to_touch` transform, the multitouch protocol decoder, and the
//! hit-testing that turns a point into an action. So this writes real evdev
//! records to the real touch node, and everything downstream cannot tell the
//! difference -- which is the entire point.
//!
//! The reader that owns touch holds an exclusive `EVIOCGRAB`. That grab
//! excludes other *readers*; the kernel still delivers written events to the
//! grabbing reader, so nothing has to be stopped, unhooked or cooperated with.
//!
//! # What it refuses to do
//!
//! It is behind `device-write` and behind an unlock phrase, because a program
//! that can tap anything can tap the stock reader's factory-reset button.
//! Every point it is given must be on the screen, and it checks all of them
//! before it taps any of them, so a sequence with a bad point in the middle
//! taps nothing rather than stopping halfway through a tour. It holds each
//! contact for a fixed short interval and always lifts: a tap that failed
//! halfway through would leave the digitiser reporting a finger that is not
//! there, and the reader would not respond to a real one until it was
//! rebooted.
//!
//! # Why one invocation can tap more than once
//!
//! It used to tap once and exit, and driving an application meant one
//! invocation per tap. Each of those uploads this binary to the device,
//! verifies its checksum there and removes it again, which on a reader over
//! Wi-Fi costs seconds. A recording is at most five minutes long, so spending
//! several of them on transfers means the tour is mostly the tour not
//! happening.
//!
//! Taking the whole sequence at once costs one upload, and it also puts the
//! clock on the device. Waits between taps are then the waits that were asked
//! for, rather than those waits plus however long an SSH round trip took, so a
//! tap intended to land while a screen is up actually lands while it is up.

use kobo_abi::input;
use kobo_hal::probe_device;
use kobo_hal::touch::InputEvent32;
use kobo_profile::{DeviceProfile, CLARA_BW_391};
use std::fs::OpenOptions;
use std::io::Write;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

const UNLOCK_ENV: &str = "KOBO_TAP_UNLOCK";
const UNLOCK_PHRASE: &str = "OWNER_ATTENDED_SYNTHETIC_TOUCH";
const POINT_ENV: &str = "KOBO_TAP_POINT";

/// A sequence taps at most this many times, and runs for at most this long.
///
/// The duration matches the longest recording the doctor will make, because
/// the only reason to want a long sequence is to drive one, and a tour that
/// outlives its own recording is taps nobody will ever see.
const MAXIMUM_STEPS: usize = 200;
const MAXIMUM_SEQUENCE_MILLIS: u64 = 300_000;

/// Long enough to read as a press rather than as noise, short enough that it
/// can never be taken for a long-press gesture.
const CONTACT: Duration = Duration::from_millis(60);

/// The slot and tracking id a synthetic contact uses.
///
/// Slot 0 because a single finger is slot 0 on every panel this runs on, and a
/// tracking id that no real contact will be carrying at the same moment,
/// because the decoder keys a contact's identity off it.
const SLOT: i32 = 0;
const TRACKING_ID: i32 = 0x5eed;

fn main() -> ExitCode {
    match tap() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tap failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn tap() -> Result<(), String> {
    if std::env::var(UNLOCK_ENV).ok().as_deref() != Some(UNLOCK_PHRASE) {
        return Err(format!("{UNLOCK_ENV} must be set to the unlock phrase"));
    }
    let request = std::env::var(POINT_ENV)
        .map_err(|_| format!("{POINT_ENV} must be set to 'x,y' in display pixels"))?;
    let steps = parse_sequence(&request)?;

    let snapshot = probe_device().map_err(|error| format!("probe the device: {error}"))?;
    // The transform belongs to the hardware, so the profile has to have
    // matched before a point means anything at all. Tapping with the wrong
    // profile would land somewhere plausible and wrong, which is worse than
    // not tapping.
    let touch = snapshot
        .touch
        .as_ref()
        .ok_or("no touch device was discovered")?;
    // Every point is turned into events before the first one is written, so a
    // point that is off the screen is refused while the panel is still
    // untouched. Discovering it halfway through would leave an application
    // part-way into a journey with no way to finish it.
    let planned = plan(&CLARA_BW_391, &steps)?;

    let mut node = OpenOptions::new()
        .write(true)
        .open(&touch.path)
        .map_err(|error| format!("open {} for writing: {error}", touch.path))?;
    for (step, events) in steps.iter().zip(planned) {
        if step.wait > Duration::ZERO {
            sleep(step.wait);
        }
        // Split at the lift so the contact is actually held for a moment.
        // Writing press and release in one go produces a zero-length touch,
        // which some gesture recognisers discard as a spurious contact.
        let lift_at = events.len() - LIFT_EVENTS;
        write_events(&mut node, &events[..lift_at])?;
        sleep(CONTACT);
        write_events(&mut node, &events[lift_at..])?;
        println!("tapped {},{}", step.x, step.y);
    }
    Ok(())
}

/// One tap, and how long to wait before making it.
///
/// The wait comes first rather than last so that a sequence reads as what it
/// is: give the screen this long, then touch it there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    wait: Duration,
    x: u32,
    y: u32,
}

/// How many of the records at the end of the sequence lift the finger.
const LIFT_EVENTS: usize = 3;

/// The events for every step, or the first reason the sequence cannot be made.
///
/// Separate from writing them so that a whole tour is checked against the
/// panel before any of it reaches the digitiser. Stopping in the middle is the
/// one failure mode that leaves the device somewhere nobody asked for.
fn plan(profile: &DeviceProfile, steps: &[Step]) -> Result<Vec<Vec<InputEvent32>>, String> {
    steps
        .iter()
        .map(|step| press_and_lift(profile, step.x, step.y))
        .collect()
}

/// The evdev records for one complete tap, press first and lift last.
fn press_and_lift(profile: &DeviceProfile, x: u32, y: u32) -> Result<Vec<InputEvent32>, String> {
    let (raw_x, raw_y) = profile
        .display_to_touch(x, y)
        .ok_or_else(|| format!("{x},{y} is not on the screen"))?;
    Ok(vec![
        event(input::EV_ABS, input::ABS_MT_SLOT, SLOT),
        event(input::EV_ABS, input::ABS_MT_TRACKING_ID, TRACKING_ID),
        event(input::EV_ABS, input::ABS_MT_POSITION_X, raw_x),
        event(input::EV_ABS, input::ABS_MT_POSITION_Y, raw_y),
        event(input::EV_KEY, input::BTN_TOUCH, 1),
        event(input::EV_SYN, input::SYN_REPORT, 0),
        // The lift. `-1` is how the multitouch protocol says a contact ended.
        event(input::EV_ABS, input::ABS_MT_TRACKING_ID, -1),
        event(input::EV_KEY, input::BTN_TOUCH, 0),
        event(input::EV_SYN, input::SYN_REPORT, 0),
    ])
}

const fn event(kind: u16, code: u16, value: i32) -> InputEvent32 {
    InputEvent32 { kind, code, value }
}

/// Writes each record as the 16-byte struct the kernel reads back.
///
/// The timestamp is left at zero deliberately: the kernel stamps written
/// events with its own arrival time, and a host-supplied one would be in the
/// host's clock, which is not the device's.
fn write_events(node: &mut impl Write, events: &[InputEvent32]) -> Result<(), String> {
    for event in events {
        node.write_all(&encode(*event))
            .map_err(|error| format!("write a touch event: {error}"))?;
    }
    node.flush()
        .map_err(|error| format!("flush the touch events: {error}"))
}

fn encode(event: InputEvent32) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[8..10].copy_from_slice(&event.kind.to_le_bytes());
    bytes[10..12].copy_from_slice(&event.code.to_le_bytes());
    bytes[12..16].copy_from_slice(&event.value.to_le_bytes());
    bytes
}

/// Reads a whole sequence: whitespace-separated `x,y` or `wait:x,y` steps.
///
/// The wait is in milliseconds, which is what a driving script actually wants:
/// an e-ink refresh is somewhere near a second, so the interesting waits are
/// all a small number of them and none of them are round.
///
/// A bare `x,y` is one tap with no wait, which is what this took before it
/// took sequences, so every existing caller still means what it meant.
fn parse_sequence(request: &str) -> Result<Vec<Step>, String> {
    let steps = request
        .split_whitespace()
        .map(parse_step)
        .collect::<Result<Vec<_>, _>>()?;
    if steps.is_empty() {
        return Err(format!("{POINT_ENV} must name at least one point"));
    }
    if steps.len() > MAXIMUM_STEPS {
        return Err(format!(
            "{POINT_ENV} has {} taps, and {MAXIMUM_STEPS} is the most one run will make",
            steps.len()
        ));
    }
    // Bounded here as well as by the timeout the host wraps this in, because a
    // program that holds the touch node is a program that has to stop on its
    // own even if the host walks away.
    let total = steps
        .iter()
        .try_fold(0_u64, |total, step| {
            u64::try_from(step.wait.as_millis())
                .ok()
                .and_then(|wait| total.checked_add(wait))
        })
        .ok_or_else(|| format!("{POINT_ENV} asks to wait longer than this will ever wait"))?;
    if total > MAXIMUM_SEQUENCE_MILLIS {
        return Err(format!(
            "{POINT_ENV} waits {total}ms in total, and {MAXIMUM_SEQUENCE_MILLIS}ms is the longest run"
        ));
    }
    Ok(steps)
}

fn parse_step(token: &str) -> Result<Step, String> {
    let (wait, point) = match token.split_once(':') {
        Some((wait, point)) => (
            wait.trim()
                .parse::<u64>()
                .map_err(|_| format!("'{token}' does not start with a wait in milliseconds"))?,
            point,
        ),
        None => (0, token),
    };
    let (x, y) = parse_point(point)?;
    Ok(Step {
        wait: Duration::from_millis(wait),
        x,
        y,
    })
}

fn parse_point(request: &str) -> Result<(u32, u32), String> {
    let (x, y) = request
        .trim()
        .split_once(',')
        .ok_or_else(|| format!("{POINT_ENV} must be 'x,y'"))?;
    let x = x
        .trim()
        .parse()
        .map_err(|_| format!("{POINT_ENV} must be whole numbers"))?;
    let y = y
        .trim()
        .parse()
        .map_err(|_| format!("{POINT_ENV} must be whole numbers"))?;
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::{
        encode, parse_point, parse_sequence, plan, press_and_lift, Step, LIFT_EVENTS,
        MAXIMUM_SEQUENCE_MILLIS, MAXIMUM_STEPS,
    };
    use std::time::Duration;
    use kobo_abi::input;
    use kobo_hal::touch::{InputEvent32, TouchDecoder, TouchEvent};
    use kobo_profile::CLARA_BW_391;

    #[test]
    fn a_record_encodes_where_the_decoder_looks_for_it() {
        let bytes = encode(InputEvent32 {
            kind: input::EV_ABS,
            code: input::ABS_MT_POSITION_X,
            value: -1,
        });
        assert_eq!(
            InputEvent32::decode(&bytes),
            Some(InputEvent32 {
                kind: input::EV_ABS,
                code: input::ABS_MT_POSITION_X,
                value: -1,
            }),
            "the encoder and the decoder must agree byte for byte, or a tap lands nowhere"
        );
    }

    #[test]
    fn a_synthetic_tap_reads_back_as_a_press_and_a_lift_at_the_point_asked_for() {
        // The strongest check available without hardware: feed what would be
        // written into the decoder the device itself uses, and require that it
        // reports the same coordinates that went in.
        let events = press_and_lift(&CLARA_BW_391, 536, 900).expect("the middle of the screen");
        let mut decoder = TouchDecoder::default();
        let mut reported = Vec::new();
        for event in &events {
            if let Some(touch) = decoder.push(*event, &CLARA_BW_391) {
                reported.push(touch);
            }
        }
        assert_eq!(
            reported.first(),
            Some(&TouchEvent::Down { x: 536, y: 900 }),
            "the round trip through the profile's transform must be lossless"
        );
        assert!(
            matches!(reported.last(), Some(TouchEvent::Up { .. })),
            "a tap that does not lift leaves a phantom finger on the glass: {reported:?}"
        );
    }

    #[test]
    fn the_lift_is_the_last_thing_written() {
        let events = press_and_lift(&CLARA_BW_391, 100, 100).expect("on the screen");
        let lift = &events[events.len() - LIFT_EVENTS..];
        assert_eq!(lift[0].code, input::ABS_MT_TRACKING_ID);
        assert_eq!(lift[0].value, -1, "-1 is how a contact ends");
        assert_eq!(lift[2].code, input::SYN_REPORT);
    }

    #[test]
    fn a_point_off_the_screen_is_refused_rather_than_clamped_onto_the_edge() {
        assert!(press_and_lift(&CLARA_BW_391, 5000, 5000).is_err());
    }

    #[test]
    fn the_point_is_read_the_way_it_is_written() {
        assert_eq!(parse_point(" 12 , 34 "), Ok((12, 34)));
        assert!(parse_point("12").is_err());
        assert!(parse_point("12,-3").is_err());
    }

    #[test]
    fn one_point_on_its_own_is_still_one_tap_with_no_wait() {
        // Everything that called this before it took sequences passes a bare
        // point, and all of them must go on meaning what they meant.
        assert_eq!(
            parse_sequence("536,400"),
            Ok(vec![Step {
                wait: Duration::ZERO,
                x: 536,
                y: 400
            }])
        );
    }

    #[test]
    fn a_sequence_keeps_its_order_and_its_waits() {
        let steps = parse_sequence(" 1500:536,400  80,80\n2000:400,1380 ").expect("a sequence");
        assert_eq!(
            steps,
            vec![
                Step {
                    wait: Duration::from_millis(1500),
                    x: 536,
                    y: 400
                },
                Step {
                    wait: Duration::ZERO,
                    x: 80,
                    y: 80
                },
                Step {
                    wait: Duration::from_millis(2000),
                    x: 400,
                    y: 1380
                },
            ]
        );
    }

    #[test]
    fn a_sequence_that_would_outlive_its_own_recording_is_refused() {
        let long = format!("{MAXIMUM_SEQUENCE_MILLIS}:10,10 1:20,20");
        assert!(parse_sequence(&long).is_err());
        assert!(parse_sequence(&format!("{MAXIMUM_SEQUENCE_MILLIS}:10,10")).is_ok());
    }

    #[test]
    fn a_sequence_longer_than_the_step_limit_is_refused() {
        let many = std::iter::repeat_n("10,10", MAXIMUM_STEPS + 1)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(parse_sequence(&many).is_err());
    }

    #[test]
    fn nothing_at_all_is_refused_rather_than_taken_as_no_taps() {
        // An empty variable is far more likely to be a script that failed to
        // build its sequence than a caller asking for silence, and a run that
        // reports success having tapped nothing would hide it.
        assert!(parse_sequence("").is_err());
        assert!(parse_sequence("   \n ").is_err());
    }

    #[test]
    fn a_wait_that_is_not_a_number_is_refused_rather_than_read_as_a_point() {
        assert!(parse_sequence("soon:10,10").is_err());
        assert!(parse_sequence("-5:10,10").is_err());
    }

    #[test]
    fn one_bad_point_anywhere_stops_the_whole_sequence() {
        // The check that matters. A tour that taps three times and then finds
        // its fourth point is off the panel has left an application somewhere
        // nobody asked for it to be, with no way to finish the journey.
        let steps = parse_sequence("10,10 20,20 4000,20").expect("it parses");
        assert!(plan(&CLARA_BW_391, &steps).is_err());
        let good = parse_sequence("10,10 20,20 30,30").expect("it parses");
        assert!(plan(&CLARA_BW_391, &good).is_ok());
    }
}
