use kobo_abi::input;
use kobo_profile::DeviceProfile;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputEvent32 {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent32 {
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 16 {
            return None;
        }
        Some(Self {
            kind: u16::from_le_bytes([bytes[8], bytes[9]]),
            code: u16::from_le_bytes([bytes[10], bytes[11]]),
            value: i32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TouchEvent {
    Down { x: u32, y: u32 },
    Move { x: u32, y: u32 },
    Up { x: u32, y: u32 },
}

const MAX_TOUCH_SLOTS: usize = 32;
const SLOT_X_FRESH: u8 = 1 << 0;
const SLOT_Y_FRESH: u8 = 1 << 1;
const SLOT_REPORTED: u8 = 1 << 2;
const SLOT_CHANGED: u8 = 1 << 3;

#[derive(Clone, Copy, Debug, Default)]
struct SlotState {
    raw_x: i32,
    raw_y: i32,
    tracking_id: Option<i32>,
    flags: u8,
    last_point: Option<(u32, u32)>,
}

impl SlotState {
    const fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }
}

#[derive(Debug)]
pub struct TouchDecoder {
    slots: [SlotState; MAX_TOUCH_SLOTS],
    current_slot: Option<usize>,
    primary_slot: Option<usize>,
    blocked_until_quiescent: bool,
    unknown_slot_activity: bool,
}

impl Default for TouchDecoder {
    fn default() -> Self {
        Self {
            slots: [SlotState::default(); MAX_TOUCH_SLOTS],
            current_slot: Some(0),
            primary_slot: None,
            blocked_until_quiescent: false,
            unknown_slot_activity: false,
        }
    }
}

impl TouchDecoder {
    pub fn push(&mut self, event: InputEvent32, profile: &DeviceProfile) -> Option<TouchEvent> {
        match (event.kind, event.code) {
            (input::EV_ABS, input::ABS_MT_SLOT) => {
                self.current_slot = usize::try_from(event.value)
                    .ok()
                    .filter(|slot| *slot < MAX_TOUCH_SLOTS);
                if self.current_slot.is_none() {
                    self.unknown_slot_activity = true;
                }
            }
            (input::EV_ABS, input::ABS_MT_POSITION_X) => {
                if let Some(slot) = self.current_slot {
                    self.slots[slot].raw_x = event.value;
                    self.slots[slot].flags |= SLOT_X_FRESH | SLOT_CHANGED;
                }
            }
            (input::EV_ABS, input::ABS_MT_POSITION_Y) => {
                if let Some(slot) = self.current_slot {
                    self.slots[slot].raw_y = event.value;
                    self.slots[slot].flags |= SLOT_Y_FRESH | SLOT_CHANGED;
                }
            }
            (input::EV_ABS, input::ABS_MT_TRACKING_ID) => {
                if let Some(slot) = self.current_slot {
                    self.update_tracking(slot, event.value);
                } else {
                    self.unknown_slot_activity = true;
                }
            }
            // A captured report from the real panel contains no `ABS_MT_SLOT`
            // and does contain `BTN_TOUCH`, so relying only on a `-1` tracking
            // id to end a contact leaves a finger held down forever if this
            // driver ever omits it. `BTN_TOUCH` going low means no contact
            // remains anywhere, so every slot is released; doing it twice is
            // harmless when both are sent.
            (input::EV_KEY, input::BTN_TOUCH) if event.value == 0 => {
                for slot in &mut self.slots {
                    if slot.tracking_id.is_some() {
                        slot.tracking_id = None;
                        slot.set(SLOT_CHANGED, true);
                    }
                }
            }
            (input::EV_SYN, input::SYN_REPORT) => {
                return self.finish_report(profile);
            }
            _ => {}
        }
        None
    }

    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        !self.unknown_slot_activity
            && self.primary_slot.is_none()
            && self.slots.iter().all(|slot| slot.tracking_id.is_none())
    }

    fn update_tracking(&mut self, slot_index: usize, tracking_id: i32) {
        let any_active = self.slots.iter().any(|slot| slot.tracking_id.is_some());
        let slot = &mut self.slots[slot_index];
        if tracking_id < 0 {
            slot.tracking_id = None;
        } else {
            let is_new_contact = slot.tracking_id != Some(tracking_id);
            slot.tracking_id = Some(tracking_id);
            if is_new_contact {
                slot.set(SLOT_REPORTED | SLOT_X_FRESH | SLOT_Y_FRESH, false);
                slot.last_point = None;
            }
            if self.primary_slot.is_none() && !self.blocked_until_quiescent && !any_active {
                self.primary_slot = Some(slot_index);
            }
        }
        slot.set(SLOT_CHANGED, true);
    }

    fn finish_report(&mut self, profile: &DeviceProfile) -> Option<TouchEvent> {
        let mut output = None;
        if let Some(slot_index) = self.primary_slot {
            let slot = &mut self.slots[slot_index];
            let active = slot.tracking_id.is_some();
            let point = if active && slot.has(SLOT_X_FRESH) && slot.has(SLOT_Y_FRESH) {
                profile.touch_to_display(slot.raw_x, slot.raw_y)
            } else {
                None
            };
            if let Some(point) = point {
                slot.last_point = Some(point);
            }

            output = match (
                slot.has(SLOT_REPORTED),
                active,
                slot.has(SLOT_CHANGED),
                point,
            ) {
                (false, true, true, Some((x, y))) => {
                    slot.set(SLOT_REPORTED, true);
                    Some(TouchEvent::Down { x, y })
                }
                (true, true, true, Some((x, y))) => Some(TouchEvent::Move { x, y }),
                (true, false, true, _) => slot.last_point.map(|(x, y)| TouchEvent::Up { x, y }),
                _ => None,
            };
            slot.set(SLOT_CHANGED, false);

            if !active {
                slot.set(
                    SLOT_REPORTED | SLOT_X_FRESH | SLOT_Y_FRESH | SLOT_CHANGED,
                    false,
                );
                slot.last_point = None;
                self.primary_slot = None;
            }
        }

        let any_active = self.slots.iter().any(|slot| slot.tracking_id.is_some());
        if self.primary_slot.is_none() {
            self.blocked_until_quiescent = any_active;
        }
        if !any_active {
            self.blocked_until_quiescent = false;
            for slot in &mut self.slots {
                if !slot.has(SLOT_REPORTED) {
                    slot.set(SLOT_CHANGED | SLOT_X_FRESH | SLOT_Y_FRESH, false);
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{InputEvent32, TouchDecoder, TouchEvent, MAX_TOUCH_SLOTS};
    use kobo_abi::input;
    use kobo_profile::CLARA_BW_391;

    /// Every event of one real press, captured byte for byte from
    /// `/dev/input/event1` on the Clara BW. It contains no `ABS_MT_SLOT` and
    /// does contain `BTN_TOUCH` and `SYN_MT_REPORT`, which is why the decoder
    /// cannot assume a textbook protocol B stream.
    const CAPTURED_PRESS: &[(u16, u16, i32)] = &[
        (1, 325, 1),
        (1, 330, 1),
        (3, 57, 0),
        (3, 59, 0),
        (3, 53, 374),
        (3, 54, 267),
        (3, 58, 1),
        (3, 0, 374),
        (3, 1, 267),
        (3, 24, 1024),
        (3, 48, 0),
        (3, 49, 0),
        (3, 52, 0),
        (0, 2, 0),
        (0, 0, 0),
    ];

    fn feed(decoder: &mut TouchDecoder, report: &[(u16, u16, i32)]) -> Vec<TouchEvent> {
        report
            .iter()
            .filter_map(|&(kind, code, value)| {
                decoder.push(InputEvent32 { kind, code, value }, &CLARA_BW_391)
            })
            .collect()
    }

    #[test]
    fn decodes_the_press_captured_from_the_device() {
        let mut decoder = TouchDecoder::default();
        assert_eq!(
            feed(&mut decoder, CAPTURED_PRESS),
            vec![TouchEvent::Down { x: 804, y: 374 }],
            "raw (374,267) maps to display (1071-267, 374)"
        );
    }

    /// The release this panel sends carries no coordinates, so the position
    /// has to come from the press. Without `BTN_TOUCH` the contact would never
    /// end and every later tap would be ignored, which is exactly what was
    /// observed on the device.
    #[test]
    fn a_release_without_a_tracking_id_still_ends_the_contact() {
        let mut decoder = TouchDecoder::default();
        feed(&mut decoder, CAPTURED_PRESS);
        let release = [(1, 330, 0), (1, 325, 0), (0, 2, 0), (0, 0, 0)];
        assert_eq!(
            feed(&mut decoder, &release),
            vec![TouchEvent::Up { x: 804, y: 374 }]
        );
        assert!(decoder.is_quiescent(), "the panel is idle after a release");
    }

    /// A second tap must behave exactly like the first. This is the regression
    /// that matters: one press decoded and then nothing ever again.
    #[test]
    fn a_second_tap_is_decoded_like_the_first() {
        let mut decoder = TouchDecoder::default();
        let release = [(1, 330, 0), (1, 325, 0), (0, 2, 0), (0, 0, 0)];
        feed(&mut decoder, CAPTURED_PRESS);
        feed(&mut decoder, &release);
        assert_eq!(
            feed(&mut decoder, CAPTURED_PRESS),
            vec![TouchEvent::Down { x: 804, y: 374 }]
        );
        assert_eq!(
            feed(&mut decoder, &release),
            vec![TouchEvent::Up { x: 804, y: 374 }]
        );
    }

    /// Both release styles arriving together must not produce two events.
    #[test]
    fn a_redundant_release_is_idempotent() {
        let mut decoder = TouchDecoder::default();
        feed(&mut decoder, CAPTURED_PRESS);
        let release = [(1, 330, 0), (3, 57, -1), (0, 0, 0)];
        assert_eq!(
            feed(&mut decoder, &release),
            vec![TouchEvent::Up { x: 804, y: 374 }]
        );
    }

    #[test]
    fn decodes_measured_touch_mapping() {
        let mut decoder = TouchDecoder::default();
        let events = [
            InputEvent32 {
                kind: input::EV_ABS,
                code: input::ABS_MT_TRACKING_ID,
                value: 7,
            },
            InputEvent32 {
                kind: input::EV_ABS,
                code: input::ABS_MT_POSITION_X,
                value: 100,
            },
            InputEvent32 {
                kind: input::EV_ABS,
                code: input::ABS_MT_POSITION_Y,
                value: 200,
            },
            InputEvent32 {
                kind: input::EV_SYN,
                code: input::SYN_REPORT,
                value: 0,
            },
        ];
        let output = events
            .into_iter()
            .find_map(|event| decoder.push(event, &CLARA_BW_391));
        assert_eq!(output, Some(TouchEvent::Down { x: 871, y: 100 }));
    }

    #[test]
    fn interleaved_slots_do_not_mix_coordinates() {
        let mut decoder = TouchDecoder::default();
        let events = [
            absolute(input::ABS_MT_SLOT, 0),
            absolute(input::ABS_MT_TRACKING_ID, 7),
            absolute(input::ABS_MT_POSITION_X, 100),
            absolute(input::ABS_MT_POSITION_Y, 200),
            absolute(input::ABS_MT_SLOT, 1),
            absolute(input::ABS_MT_TRACKING_ID, 8),
            absolute(input::ABS_MT_POSITION_X, 500),
            absolute(input::ABS_MT_POSITION_Y, 600),
            sync(),
        ];
        assert_eq!(
            events
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_391)),
            Some(TouchEvent::Down { x: 871, y: 100 })
        );

        assert_eq!(
            decoder.push(absolute(input::ABS_MT_POSITION_X, 700), &CLARA_BW_391),
            None
        );
        assert_eq!(decoder.push(sync(), &CLARA_BW_391), None);

        let move_events = [
            absolute(input::ABS_MT_SLOT, 0),
            absolute(input::ABS_MT_POSITION_X, 110),
            absolute(input::ABS_MT_POSITION_Y, 210),
            sync(),
        ];
        assert_eq!(
            move_events
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_391)),
            Some(TouchEvent::Move { x: 861, y: 110 })
        );

        let release_primary = [absolute(input::ABS_MT_TRACKING_ID, -1), sync()];
        assert_eq!(
            release_primary
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_391)),
            Some(TouchEvent::Up { x: 861, y: 110 })
        );
        assert!(!decoder.is_quiescent());

        let release_secondary = [
            absolute(input::ABS_MT_SLOT, 1),
            absolute(input::ABS_MT_TRACKING_ID, -1),
            sync(),
        ];
        assert_eq!(
            release_secondary
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_391)),
            None
        );
        assert!(decoder.is_quiescent());
    }

    #[test]
    fn unknown_slots_fail_quiescence_closed() {
        let mut decoder = TouchDecoder::default();
        assert_eq!(
            decoder.push(
                absolute(
                    input::ABS_MT_SLOT,
                    i32::try_from(MAX_TOUCH_SLOTS).expect("slot count")
                ),
                &CLARA_BW_391
            ),
            None
        );
        assert!(!decoder.is_quiescent());
    }

    #[test]
    fn replacement_tracking_id_requires_fresh_coordinates() {
        let mut decoder = TouchDecoder::default();
        for event in [
            absolute(input::ABS_MT_TRACKING_ID, 1),
            absolute(input::ABS_MT_POSITION_X, 100),
            absolute(input::ABS_MT_POSITION_Y, 200),
        ] {
            assert_eq!(decoder.push(event, &CLARA_BW_391), None);
        }
        assert_eq!(
            decoder.push(sync(), &CLARA_BW_391),
            Some(TouchEvent::Down { x: 871, y: 100 })
        );

        assert_eq!(
            decoder.push(absolute(input::ABS_MT_TRACKING_ID, 2), &CLARA_BW_391),
            None
        );
        assert_eq!(decoder.push(sync(), &CLARA_BW_391), None);
        for event in [
            absolute(input::ABS_MT_POSITION_X, 300),
            absolute(input::ABS_MT_POSITION_Y, 400),
        ] {
            assert_eq!(decoder.push(event, &CLARA_BW_391), None);
        }
        assert_eq!(
            decoder.push(sync(), &CLARA_BW_391),
            Some(TouchEvent::Down { x: 671, y: 300 })
        );
    }

    const fn absolute(code: u16, value: i32) -> InputEvent32 {
        InputEvent32 {
            kind: input::EV_ABS,
            code,
            value,
        }
    }

    const fn sync() -> InputEvent32 {
        InputEvent32 {
            kind: input::EV_SYN,
            code: input::SYN_REPORT,
            value: 0,
        }
    }
}
