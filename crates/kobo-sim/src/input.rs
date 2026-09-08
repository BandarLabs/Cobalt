//! Bounded synthetic evdev replay through Cobalt's real HAL decoders.
use kobo_abi::input as ev;
use kobo_hal::{gesture::HoldTracker, gpio, InputEvent32, TouchDecoder, TouchEvent};
use kobo_profile::PanelPose;
use std::io;

#[derive(Clone, Debug)]
pub struct Replay {
    decoder: TouchDecoder,
    holds: HoldTracker,
    forward_is_194: bool,
    raw_events: u64,
}
impl Replay {
    pub fn new(pose: &PanelPose<'_>) -> Self {
        Self {
            decoder: TouchDecoder::default(),
            holds: HoldTracker::default(),
            forward_is_194: pose.rotation() % 4 == pose.profile().reference_rotation % 4,
            raw_events: 0,
        }
    }
    pub fn touch(
        &mut self,
        source: &str,
        pose: &PanelPose<'_>,
        millis: u64,
    ) -> io::Result<Vec<(TouchEvent, bool)>> {
        let events = parse(source)?;
        let mut next = self.clone();
        let decoded = events
            .iter()
            .filter_map(|event| {
                next.raw_events = next.raw_events.saturating_add(1);
                let event = next.decoder.push(*event, pose)?;
                Some((event, next.holds.observe(event, millis)))
            })
            .collect::<Vec<_>>();
        if decoded
            .iter()
            .filter(|(event, _)| matches!(event, TouchEvent::Up { .. }))
            .count()
            > 1
        {
            return Err(io::Error::other(
                "split separate gestures into separate input steps so app callbacks can settle",
            ));
        }
        *self = next;
        Ok(decoded)
    }
    pub fn gpio(&mut self, source: &str) -> io::Result<Option<bool>> {
        let events = parse(source)?;
        if events.len() != 1 {
            return Err(io::Error::other("one GPIO event per step"));
        }
        self.raw_events = self.raw_events.saturating_add(1);
        Ok(match gpio::decode(events[0]) {
            Some(gpio::GpioEvent::Button {
                button: button @ (gpio::Button::Page193 | gpio::Button::Page194),
                pressed: true,
            }) => Some((button == gpio::Button::Page194) == self.forward_is_194),
            Some(gpio::GpioEvent::Orientation(gpio::Orientation::PortraitUp)) => {
                self.forward_is_194 = true;
                None
            }
            Some(gpio::GpioEvent::Orientation(gpio::Orientation::PortraitDown)) => {
                self.forward_is_194 = false;
                None
            }
            _ => None,
        })
    }
    pub fn resynchronize(&mut self, state: &str) -> io::Result<()> {
        let snapshot = match state {
            "released" => Some(ev::TouchSnapshot {
                slot: Some(0),
                active: false,
            }),
            "active" => Some(ev::TouchSnapshot {
                slot: Some(0),
                active: true,
            }),
            "unknown" => None,
            _ => {
                return Err(io::Error::other(
                    "resync expects released, active or unknown",
                ))
            }
        };
        if !self.decoder.needs_resynchronization() {
            return Err(io::Error::other(
                "resync needs a report boundary after lost input",
            ));
        }
        self.decoder.resynchronize(snapshot);
        Ok(())
    }
    pub fn tap(
        &mut self,
        x: u32,
        y: u32,
        pose: &PanelPose<'_>,
        millis: u64,
    ) -> io::Result<Vec<(TouchEvent, bool)>> {
        if !self.decoder.is_quiescent() {
            return Err(io::Error::other(
                "finish or resynchronize the active raw gesture before tapping",
            ));
        }
        let (x, y) = pose
            .display_to_touch(x, y)
            .ok_or_else(|| io::Error::other("touch is outside the selected profile"))?;
        self.touch(
            &format!("3 47 0;3 57 1;3 53 {x};3 54 {y};0 0 0;3 57 -1;0 0 0"),
            pose,
            millis,
        )
    }
    pub fn is_quiescent(&self) -> bool { self.decoder.is_quiescent() }
    pub fn json(&self) -> kobo_json::Value {
        kobo_json::ObjectBuilder::new()
            .set("source", "synthetic evdev replay")
            .set("rawEvents", self.raw_events.to_string())
            .set("quiescent", self.decoder.is_quiescent())
            .set(
                "needsResynchronization",
                self.decoder.needs_resynchronization(),
            )
            .set(
                "forwardKey",
                if self.forward_is_194 {
                    194_u32
                } else {
                    193_u32
                },
            )
            .build()
    }
}
fn parse(source: &str) -> io::Result<Vec<InputEvent32>> {
    if source.len() > 4096 {
        return Err(io::Error::other("input step exceeds 4096 bytes"));
    }
    let mut events = Vec::new();
    for record in source.split(';') {
        if events.len() == 64 {
            return Err(io::Error::other("input step exceeds 64 events"));
        }
        let values = record.split_whitespace().collect::<Vec<_>>();
        let event = match values.as_slice() {
            [kind, code, value] => kind
                .parse()
                .ok()
                .zip(code.parse().ok())
                .zip(value.parse().ok())
                .map(|((kind, code), value)| InputEvent32 { kind, code, value }),
            _ => None,
        }
        .ok_or_else(|| {
            io::Error::other("input events use TYPE CODE VALUE, separated by semicolons")
        })?;
        events.push(event);
    }
    Ok(events)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_decoder_handles_taps_holds_lost_input_and_explicit_recovery() {
        let pose = &super::super::POSE;
        let mut replay = Replay::new(pose);
        let tap = replay.tap(100, 200, pose, 0).unwrap();
        assert!(matches!(
            tap.as_slice(),
            [
                (TouchEvent::Down { .. }, false),
                (TouchEvent::Up { .. }, false)
            ]
        ));
        let (x, y) = pose.display_to_touch(100, 200).unwrap();
        replay
            .touch(&format!("3 47 0;3 57 2;3 53 {x};3 54 {y};0 0 0"), pose, 0)
            .unwrap();
        assert!(matches!(
            replay.touch("3 57 -1;0 0 0", pose, 500).unwrap().as_slice(),
            [(TouchEvent::Up { .. }, true)]
        ));
        let dropped = replay.touch("0 3 0;0 0 0", pose, 1000).unwrap();
        assert_eq!(dropped, vec![(TouchEvent::Cancel, false)]);
        assert!(replay.tap(100, 200, pose, 1000).is_err());
        replay.resynchronize("active").unwrap();
        assert!(replay.tap(100, 200, pose, 1000).is_err());
        replay.touch("0 0 0", pose, 1000).unwrap();
        replay.resynchronize("released").unwrap();
        assert_eq!(replay.tap(100, 200, pose, 1000).unwrap().len(), 2);
    }
    #[test]
    fn page_keys_use_hal_edges_and_portrait_mapping_and_bad_records_do_not_mutate() {
        let mut replay = Replay::new(&super::super::POSE);
        replay.gpio("4 3 24").unwrap(); // HAL PortraitUp.
        assert_eq!(replay.gpio("1 194 1").unwrap(), Some(true));
        assert_eq!(replay.gpio("1 194 0").unwrap(), None);
        assert_eq!(replay.gpio("1 194 2").unwrap(), None);
        replay.gpio("4 3 23").unwrap();
        assert_eq!(replay.gpio("1 194 1").unwrap(), Some(false));
        let before = replay.json();
        assert!(replay
            .touch("3 47 0;invalid", &super::super::POSE, 0)
            .is_err());
        assert_eq!(replay.json(), before);
    }
}
