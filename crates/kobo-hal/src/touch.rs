use kobo_abi::input;
use kobo_profile::PanelPose;

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
    /// The event stream lost part of a gesture. Clear press/hold feedback;
    /// never interpret this as a release or activate a control.
    Cancel,
    Down {
        x: u32,
        y: u32,
    },
    Move {
        x: u32,
        y: u32,
    },
    Up {
        x: u32,
        y: u32,
    },
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Recovery {
    #[default]
    Ready,
    Discarding,
    Query,
}

/// How a touch controller tells one contact from another.
///
/// Every controller on a measured Kobo sends a tracking id with each contact,
/// and the decoder was written around that. Older and less common
/// controllers do not, and a decoder that waits for an id they never send
/// sees no touch at all. The kind is read from what the device declares it
/// can send rather than guessed from the first few events, because a tracked
/// controller whose finger was already down when the session began sends
/// positions with no id until it lifts, which looks exactly like an
/// untracked one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ContactReporting {
    /// Contacts carry `ABS_MT_TRACKING_ID`, with or without numbered slots.
    #[default]
    Tracked,
    /// Contacts are an anonymous list, each ended by `SYN_MT_REPORT`, and a
    /// frame with no contacts in it means every finger has lifted.
    Untracked,
    /// One contact on `ABS_X` and `ABS_Y`, down while `BTN_TOUCH` is 1.
    SingleTouch,
}

impl ContactReporting {
    /// Picks the reporting style from the codes the device declares.
    ///
    /// A tracking id wins whenever it is declared, so a controller that also
    /// mirrors its first contact onto `ABS_X` and `ABS_Y` is still decoded
    /// the way it always was.
    #[must_use]
    pub const fn from_capabilities(
        tracking_id: bool,
        multitouch_position: bool,
        single_position: bool,
    ) -> Option<Self> {
        if tracking_id {
            Some(Self::Tracked)
        } else if multitouch_position {
            Some(Self::Untracked)
        } else if single_position {
            Some(Self::SingleTouch)
        } else {
            None
        }
    }
}

/// Reads which way the device reports contacts from its declared codes.
///
/// A device that will not answer is treated as tracked, which is how every
/// measured Kobo reports and what the decoder did before it knew of anything
/// else.
#[must_use]
pub fn contact_reporting(device: &std::fs::File) -> ContactReporting {
    let declares = |code| input::supports(device, input::EV_ABS, code).unwrap_or(false);
    let button = input::supports(device, input::EV_KEY, input::BTN_TOUCH).unwrap_or(false);
    ContactReporting::from_capabilities(
        declares(input::ABS_MT_TRACKING_ID),
        declares(input::ABS_MT_POSITION_X) && declares(input::ABS_MT_POSITION_Y),
        declares(input::ABS_X) && declares(input::ABS_Y) && button,
    )
    .unwrap_or_default()
}

/// What one frame said, gathered until its `SYN_REPORT`.
#[derive(Clone, Copy, Debug, Default)]
struct Frame {
    /// The contact being assembled before its `SYN_MT_REPORT`.
    pending_x: Option<i32>,
    pending_y: Option<i32>,
    /// The first complete contact in the frame, which is the one followed.
    first: Option<(i32, i32)>,
    /// A `SYN_MT_REPORT` arrived.
    separator: bool,
    /// Any multi-touch axis arrived, including ones this decoder ignores.
    multitouch_data: bool,
    /// `BTN_TOUCH` went to 0.
    lifted: bool,
}

impl Frame {
    /// A frame that is nothing but a contact separator.
    ///
    /// That is how a controller without tracking ids says no finger is on
    /// the panel. Any multi-touch axis in the frame disqualifies it, so a
    /// frame that only updated pressure is never read as a lift.
    const fn says_no_contacts(&self) -> bool {
        self.separator && !self.multitouch_data
    }

    fn close_contact(&mut self) {
        if let (None, Some(x), Some(y)) = (self.first, self.pending_x, self.pending_y) {
            self.first = Some((x, y));
        }
        self.pending_x = None;
        self.pending_y = None;
    }
}

/// The single contact followed on a controller without tracking ids.
#[derive(Clone, Copy, Debug, Default)]
struct Untracked {
    /// Single-touch axes keep their last value across frames because the
    /// kernel drops a repeat of an unchanged one.
    single_x: Option<i32>,
    single_y: Option<i32>,
    button_down: bool,
    /// Where the followed contact was in the last frame, so a frame that
    /// restates the same point is not reported as a move.
    last: Option<(i32, i32)>,
    next_id: i32,
}

#[derive(Clone, Debug)]
pub struct TouchDecoder {
    slots: [SlotState; MAX_TOUCH_SLOTS],
    current_slot: Option<usize>,
    primary_slot: Option<usize>,
    blocked_until_quiescent: bool,
    unknown_slot_activity: bool,
    recovery: Recovery,
    reporting: ContactReporting,
    frame: Frame,
    untracked: Untracked,
}

impl Default for TouchDecoder {
    fn default() -> Self {
        Self::new(ContactReporting::Tracked)
    }
}

impl TouchDecoder {
    /// A decoder for a controller that reports contacts the given way.
    #[must_use]
    pub fn new(reporting: ContactReporting) -> Self {
        Self {
            slots: [SlotState::default(); MAX_TOUCH_SLOTS],
            current_slot: Some(0),
            primary_slot: None,
            blocked_until_quiescent: false,
            unknown_slot_activity: false,
            recovery: Recovery::Ready,
            reporting,
            frame: Frame::default(),
            untracked: Untracked::default(),
        }
    }

    pub fn push(&mut self, event: InputEvent32, pose: &PanelPose<'_>) -> Option<TouchEvent> {
        if (event.kind, event.code) == (input::EV_SYN, input::SYN_DROPPED) {
            *self = Self::new(self.reporting);
            self.recovery = Recovery::Discarding;
            return Some(TouchEvent::Cancel);
        }
        if self.recovery != Recovery::Ready {
            if (event.kind, event.code) == (input::EV_SYN, input::SYN_REPORT) {
                self.recovery = Recovery::Query;
            }
            return None;
        }
        self.observe(event);
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
                self.settle_frame();
                return self.finish_report(pose);
            }
            _ => {}
        }
        None
    }

    /// Notes what this event says about the frame it belongs to.
    ///
    /// Kept apart from the slot bookkeeping above, which is unchanged for
    /// tracked controllers. Nothing recorded here is acted on until the
    /// frame ends.
    fn observe(&mut self, event: InputEvent32) {
        match (event.kind, event.code) {
            (input::EV_ABS, code) if (input::ABS_MT_SLOT..=0x3d).contains(&code) => {
                self.frame.multitouch_data = true;
                match code {
                    input::ABS_MT_POSITION_X => self.frame.pending_x = Some(event.value),
                    input::ABS_MT_POSITION_Y => self.frame.pending_y = Some(event.value),
                    _ => {}
                }
            }
            (input::EV_ABS, input::ABS_X) => self.untracked.single_x = Some(event.value),
            (input::EV_ABS, input::ABS_Y) => self.untracked.single_y = Some(event.value),
            (input::EV_KEY, input::BTN_TOUCH) => {
                self.untracked.button_down = event.value != 0;
                if event.value == 0 {
                    self.frame.lifted = true;
                }
            }
            (input::EV_SYN, input::SYN_MT_REPORT) => {
                self.frame.separator = true;
                self.frame.close_contact();
            }
            _ => {}
        }
    }

    /// Turns a finished frame into slot state the report logic understands.
    ///
    /// A tracked controller only needs one thing from here: a frame that says
    /// there are no contacts ends any contact still open, for the controller
    /// that never sends a tracking id of -1. An untracked or single-touch
    /// controller has its one followed contact written into slot 0 with an
    /// invented tracking id, so press, move and release come out of exactly
    /// the same code as they do for every measured Kobo.
    fn settle_frame(&mut self) {
        let mut frame = std::mem::take(&mut self.frame);
        frame.close_contact();
        if frame.says_no_contacts() {
            self.release_all();
        }
        let contact = match self.reporting {
            ContactReporting::Tracked => return,
            ContactReporting::Untracked => frame.first.filter(|_| !frame.lifted),
            ContactReporting::SingleTouch => {
                match (self.untracked.single_x, self.untracked.single_y) {
                    (Some(x), Some(y)) if self.untracked.button_down => Some((x, y)),
                    _ => None,
                }
            }
        };
        self.follow(contact);
    }

    fn release_all(&mut self) {
        for slot in &mut self.slots {
            if slot.tracking_id.is_some() {
                slot.tracking_id = None;
                slot.set(SLOT_CHANGED, true);
            }
        }
    }

    /// Writes the followed contact of an untracked controller into slot 0.
    fn follow(&mut self, contact: Option<(i32, i32)>) {
        let Some((x, y)) = contact else {
            if self.slots[0].tracking_id.is_some() {
                self.slots[0].tracking_id = None;
                self.slots[0].set(SLOT_CHANGED, true);
            }
            self.untracked.last = None;
            return;
        };
        let began = self.slots[0].tracking_id.is_none();
        if began {
            // Any non-negative id will do. It only has to differ from the
            // previous contact's so the slot logic sees a new press.
            self.untracked.next_id = self.untracked.next_id.wrapping_add(1) & i32::MAX;
            self.update_tracking(0, self.untracked.next_id);
        }
        let moved = began || self.untracked.last != Some((x, y));
        let slot = &mut self.slots[0];
        slot.raw_x = x;
        slot.raw_y = y;
        slot.flags |= SLOT_X_FRESH | SLOT_Y_FRESH;
        slot.set(SLOT_CHANGED, moved);
        self.untracked.last = Some((x, y));
    }

    /// A hardware snapshot is needed only after a complete report boundary.
    #[must_use]
    pub fn needs_resynchronization(&self) -> bool {
        self.recovery == Recovery::Query
    }

    /// Cancel the old gesture until the kernel confirms all contacts released.
    /// Failed queries remain blocked and are retried at the next report boundary.
    pub fn resynchronize(&mut self, snapshot: Option<input::TouchSnapshot>) {
        if !self.needs_resynchronization() {
            return;
        }
        // Each frame from an untracked controller lists every contact on the
        // panel, so the next one is a complete picture and there is no
        // kernel state worth asking for. Waiting on a query those
        // controllers often cannot answer would leave touch dead after one
        // overflow.
        let snapshot = match (snapshot, self.reporting) {
            (None, ContactReporting::Untracked) => Some(input::TouchSnapshot {
                active: false,
                slot: Some(0),
            }),
            (snapshot, _) => snapshot,
        };
        match snapshot {
            Some(snapshot) if !snapshot.active => {
                *self = Self::new(self.reporting);
                self.current_slot = Some(usize::from(snapshot.slot.unwrap_or(0)));
                if self
                    .current_slot
                    .is_some_and(|slot| slot >= MAX_TOUCH_SLOTS)
                {
                    self.current_slot = None;
                    self.recovery = Recovery::Discarding;
                }
            }
            _ => self.recovery = Recovery::Discarding,
        }
    }

    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        self.recovery == Recovery::Ready
            && !self.unknown_slot_activity
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

    fn finish_report(&mut self, pose: &PanelPose<'_>) -> Option<TouchEvent> {
        let mut output = None;
        if let Some(slot_index) = self.primary_slot {
            let slot = &mut self.slots[slot_index];
            let active = slot.tracking_id.is_some();
            // Deliberately not conditioned on the contact still being down. A
            // release that arrives in the same report as its coordinates still
            // happened somewhere, and requiring `active` here threw those
            // coordinates away, which is how a quick tap came to register as
            // nothing at all.
            let point = if slot.has(SLOT_X_FRESH) && slot.has(SLOT_Y_FRESH) {
                pose.touch_to_display(slot.raw_x, slot.raw_y)
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
                // A contact that ended, whether or not its press was ever
                // announced. The second case is a tap short enough that the
                // driver reported the press and the release before this decoder
                // saw a report in between: there was no `Down` to pair with, but
                // a finger did touch the panel at a known place and dropping it
                // is the difference between a control that works and one the
                // reader has to tap twice.
                (_, false, true, _) => slot.last_point.map(|(x, y)| TouchEvent::Up { x, y }),
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
    use super::{ContactReporting, InputEvent32, TouchDecoder, TouchEvent, MAX_TOUCH_SLOTS};
    use kobo_abi::input;
    use kobo_profile::{PanelPose, CLARA_BW_391};

    /// The Clara BW at the pose its profile was measured in. Decoding is
    /// pose-relative now, and these tests assert display coordinates, so they
    /// have to say which pose they mean.
    const CLARA_BW_POSE: PanelPose<'static> = PanelPose::reference(&CLARA_BW_391);

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
                decoder.push(InputEvent32 { kind, code, value }, &CLARA_BW_POSE)
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
    fn a_tap_too_quick_to_be_seen_pressed_still_registers() {
        // The reader's complaint was that some taps on buttons did nothing. A
        // light, fast tap can have its press and its release land inside one
        // report: the panel reports coordinates and a tracking id, then
        // BTN_TOUCH going low, and only then the SYN. There is no report in
        // between for a Down to be emitted from, and the decoder used to
        // require one before it would emit an Up, so the whole tap vanished
        // and the control looked broken.
        let mut decoder = TouchDecoder::default();
        let events = feed(
            &mut decoder,
            &[
                (input::EV_ABS, input::ABS_MT_TRACKING_ID, 7),
                (input::EV_ABS, input::ABS_MT_POSITION_X, 374),
                (input::EV_ABS, input::ABS_MT_POSITION_Y, 267),
                (input::EV_KEY, input::BTN_TOUCH, 0),
                (input::EV_SYN, input::SYN_REPORT, 0),
            ],
        );
        let expected = CLARA_BW_POSE
            .touch_to_display(374, 267)
            .expect("the panel maps this point");
        assert_eq!(
            events,
            vec![TouchEvent::Up {
                x: expected.0,
                y: expected.1
            }],
            "a tap reported in a single frame is still a tap"
        );
        // And it leaves nothing behind, or the next contact starts confused.
        assert!(decoder.is_quiescent());
    }

    #[test]
    fn a_contact_that_never_gave_coordinates_reports_nothing() {
        // The other half of the same change: an Up is only invented when the
        // panel actually said where. A release with no position anywhere in the
        // stream must not become a tap at the origin.
        let mut decoder = TouchDecoder::default();
        let events = feed(
            &mut decoder,
            &[
                (input::EV_ABS, input::ABS_MT_TRACKING_ID, 7),
                (input::EV_KEY, input::BTN_TOUCH, 0),
                (input::EV_SYN, input::SYN_REPORT, 0),
            ],
        );
        assert!(
            events.is_empty(),
            "a contact with no position is not a tap anywhere: {events:?}"
        );
    }

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
            .find_map(|event| decoder.push(event, &CLARA_BW_POSE));
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
                .find_map(|event| decoder.push(event, &CLARA_BW_POSE)),
            Some(TouchEvent::Down { x: 871, y: 100 })
        );

        assert_eq!(
            decoder.push(absolute(input::ABS_MT_POSITION_X, 700), &CLARA_BW_POSE),
            None
        );
        assert_eq!(decoder.push(sync(), &CLARA_BW_POSE), None);

        let move_events = [
            absolute(input::ABS_MT_SLOT, 0),
            absolute(input::ABS_MT_POSITION_X, 110),
            absolute(input::ABS_MT_POSITION_Y, 210),
            sync(),
        ];
        assert_eq!(
            move_events
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_POSE)),
            Some(TouchEvent::Move { x: 861, y: 110 })
        );

        let release_primary = [absolute(input::ABS_MT_TRACKING_ID, -1), sync()];
        assert_eq!(
            release_primary
                .into_iter()
                .find_map(|event| decoder.push(event, &CLARA_BW_POSE)),
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
                .find_map(|event| decoder.push(event, &CLARA_BW_POSE)),
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
                &CLARA_BW_POSE
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
            assert_eq!(decoder.push(event, &CLARA_BW_POSE), None);
        }
        assert_eq!(
            decoder.push(sync(), &CLARA_BW_POSE),
            Some(TouchEvent::Down { x: 871, y: 100 })
        );

        assert_eq!(
            decoder.push(absolute(input::ABS_MT_TRACKING_ID, 2), &CLARA_BW_POSE),
            None
        );
        assert_eq!(decoder.push(sync(), &CLARA_BW_POSE), None);
        for event in [
            absolute(input::ABS_MT_POSITION_X, 300),
            absolute(input::ABS_MT_POSITION_Y, 400),
        ] {
            assert_eq!(decoder.push(event, &CLARA_BW_POSE), None);
        }
        assert_eq!(
            decoder.push(sync(), &CLARA_BW_POSE),
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
    #[test]
    fn dropped_reports_cancel_without_releasing_and_wait_for_a_kernel_snapshot() {
        let mut decoder = TouchDecoder::default();
        assert!(matches!(
            feed(&mut decoder, CAPTURED_PRESS).as_slice(),
            [TouchEvent::Down { .. }]
        ));
        assert_eq!(
            feed(&mut decoder, &[(input::EV_SYN, input::SYN_DROPPED, 0)]),
            [TouchEvent::Cancel]
        );
        assert!(!decoder.is_quiescent());
        assert!(!decoder.needs_resynchronization());
        // A release in the incomplete report cannot activate the old control.
        assert!(feed(
            &mut decoder,
            &[
                (input::EV_KEY, input::BTN_TOUCH, 0),
                (input::EV_SYN, input::SYN_REPORT, 0)
            ]
        )
        .is_empty());
        assert!(decoder.needs_resynchronization());
        decoder.resynchronize(Some(input::TouchSnapshot {
            active: true,
            slot: Some(0),
        }));
        assert!(!decoder.is_quiescent());
        assert!(feed(&mut decoder, CAPTURED_PRESS).is_empty());
        decoder.resynchronize(Some(input::TouchSnapshot {
            active: false,
            slot: Some(0),
        }));
        assert!(decoder.is_quiescent());
        assert!(matches!(
            feed(&mut decoder, CAPTURED_PRESS).as_slice(),
            [TouchEvent::Down { .. }]
        ));
    }

    #[test]
    fn failed_or_unsafe_resynchronization_does_not_guess_contact_state() {
        let mut decoder = TouchDecoder::default();
        feed(
            &mut decoder,
            &[
                (input::EV_SYN, input::SYN_DROPPED, 0),
                (input::EV_SYN, input::SYN_REPORT, 0),
            ],
        );
        decoder.resynchronize(None);
        assert!(!decoder.is_quiescent());
        assert!(feed(&mut decoder, CAPTURED_PRESS).is_empty());
        decoder.resynchronize(Some(input::TouchSnapshot {
            active: false,
            slot: Some(200),
        }));
        assert!(!decoder.is_quiescent());
        assert!(feed(&mut decoder, CAPTURED_PRESS).is_empty());
        decoder.resynchronize(Some(input::TouchSnapshot {
            active: false,
            slot: Some(7),
        }));
        assert!(decoder.is_quiescent());
        // The selected kernel slot is retained even when it is not re-emitted.
        assert!(matches!(
            feed(&mut decoder, CAPTURED_PRESS).as_slice(),
            [TouchEvent::Down { .. }]
        ));
    }
    /// Where the Clara BW pose puts a raw point, so the tests below assert
    /// decoding rather than restating the mapping.
    fn at(raw_x: i32, raw_y: i32) -> (u32, u32) {
        CLARA_BW_POSE
            .touch_to_display(raw_x, raw_y)
            .expect("the point is inside the panel")
    }

    fn down(raw_x: i32, raw_y: i32) -> TouchEvent {
        let (x, y) = at(raw_x, raw_y);
        TouchEvent::Down { x, y }
    }

    fn moved(raw_x: i32, raw_y: i32) -> TouchEvent {
        let (x, y) = at(raw_x, raw_y);
        TouchEvent::Move { x, y }
    }

    fn up(raw_x: i32, raw_y: i32) -> TouchEvent {
        let (x, y) = at(raw_x, raw_y);
        TouchEvent::Up { x, y }
    }

    const SEPARATOR: (u16, u16, i32) = (input::EV_SYN, input::SYN_MT_REPORT, 0);
    const REPORT: (u16, u16, i32) = (input::EV_SYN, input::SYN_REPORT, 0);

    fn contact(x: i32, y: i32) -> [(u16, u16, i32); 3] {
        [
            (input::EV_ABS, input::ABS_MT_POSITION_X, x),
            (input::EV_ABS, input::ABS_MT_POSITION_Y, y),
            SEPARATOR,
        ]
    }

    #[test]
    fn a_tap_from_a_controller_without_tracking_ids_is_a_press_and_a_release() {
        let mut decoder = TouchDecoder::new(ContactReporting::Untracked);
        let press = [contact(300, 400).as_slice(), &[REPORT]].concat();
        assert_eq!(feed(&mut decoder, &press), vec![down(300, 400)]);
        // The same contact restated in the next frame is not a move.
        assert!(feed(&mut decoder, &press).is_empty());
        let drag = [contact(320, 410).as_slice(), &[REPORT]].concat();
        assert_eq!(feed(&mut decoder, &drag), vec![moved(320, 410)]);
        // A frame holding only a separator is how this protocol says nothing
        // is touching the panel any more.
        assert_eq!(feed(&mut decoder, &[SEPARATOR, REPORT]), vec![up(320, 410)]);
        assert!(decoder.is_quiescent());
        assert_eq!(feed(&mut decoder, &press), vec![down(300, 400)]);
    }

    #[test]
    fn an_untracked_controller_that_lifts_with_btn_touch_alone_is_released() {
        let mut decoder = TouchDecoder::new(ContactReporting::Untracked);
        let press = [contact(300, 400).as_slice(), &[REPORT]].concat();
        feed(&mut decoder, &press);
        assert_eq!(
            feed(
                &mut decoder,
                &[(input::EV_KEY, input::BTN_TOUCH, 0), REPORT]
            ),
            vec![up(300, 400)]
        );
        assert!(decoder.is_quiescent());
    }

    #[test]
    fn with_two_fingers_on_an_untracked_controller_the_first_one_listed_is_followed() {
        let mut decoder = TouchDecoder::new(ContactReporting::Untracked);
        let frame = [
            contact(300, 400).as_slice(),
            contact(900, 800).as_slice(),
            &[REPORT],
        ]
        .concat();
        assert_eq!(feed(&mut decoder, &frame), vec![down(300, 400)]);
    }

    #[test]
    fn an_untracked_controller_that_omits_the_separator_on_its_only_contact_is_still_read() {
        let mut decoder = TouchDecoder::new(ContactReporting::Untracked);
        let frame = [
            (input::EV_ABS, input::ABS_MT_POSITION_X, 300),
            (input::EV_ABS, input::ABS_MT_POSITION_Y, 400),
            REPORT,
        ];
        assert_eq!(feed(&mut decoder, &frame), vec![down(300, 400)]);
    }

    #[test]
    fn a_slotted_controller_that_never_sends_minus_one_is_released_by_btn_touch() {
        let mut decoder = TouchDecoder::default();
        let press = [
            (input::EV_ABS, input::ABS_MT_SLOT, 0),
            (input::EV_ABS, input::ABS_MT_TRACKING_ID, 5),
            (input::EV_ABS, input::ABS_MT_POSITION_X, 300),
            (input::EV_ABS, input::ABS_MT_POSITION_Y, 400),
            (input::EV_KEY, input::BTN_TOUCH, 1),
            REPORT,
        ];
        assert_eq!(feed(&mut decoder, &press), vec![down(300, 400)]);
        assert_eq!(
            feed(
                &mut decoder,
                &[(input::EV_KEY, input::BTN_TOUCH, 0), REPORT]
            ),
            vec![up(300, 400)]
        );
        assert!(decoder.is_quiescent());
        // The next contact brings a new id and has to start a new press.
        let again = [
            (input::EV_ABS, input::ABS_MT_TRACKING_ID, 6),
            (input::EV_ABS, input::ABS_MT_POSITION_X, 500),
            (input::EV_ABS, input::ABS_MT_POSITION_Y, 600),
            (input::EV_KEY, input::BTN_TOUCH, 1),
            REPORT,
        ];
        assert_eq!(feed(&mut decoder, &again), vec![down(500, 600)]);
    }

    #[test]
    fn a_tracked_controller_that_never_sends_minus_one_is_released_by_an_empty_frame() {
        let mut decoder = TouchDecoder::default();
        let press = [
            (input::EV_ABS, input::ABS_MT_TRACKING_ID, 5),
            (input::EV_ABS, input::ABS_MT_POSITION_X, 300),
            (input::EV_ABS, input::ABS_MT_POSITION_Y, 400),
            SEPARATOR,
            REPORT,
        ];
        assert_eq!(feed(&mut decoder, &press), vec![down(300, 400)]);
        assert_eq!(feed(&mut decoder, &[SEPARATOR, REPORT]), vec![up(300, 400)]);
        assert!(decoder.is_quiescent());
    }

    #[test]
    fn a_frame_that_only_changes_pressure_is_not_read_as_a_lift() {
        let mut decoder = TouchDecoder::default();
        feed(&mut decoder, CAPTURED_PRESS);
        let pressure_only = [(input::EV_ABS, 58, 3), SEPARATOR, REPORT];
        assert!(feed(&mut decoder, &pressure_only).is_empty());
        assert!(!decoder.is_quiescent(), "the finger is still down");
    }

    #[test]
    fn a_single_touch_controller_reports_press_move_and_release_from_btn_touch() {
        let mut decoder = TouchDecoder::new(ContactReporting::SingleTouch);
        let press = [
            (input::EV_ABS, input::ABS_X, 300),
            (input::EV_ABS, input::ABS_Y, 400),
            (input::EV_KEY, input::BTN_TOUCH, 1),
            REPORT,
        ];
        assert_eq!(feed(&mut decoder, &press), vec![down(300, 400)]);
        // The kernel drops a repeat of an unchanged axis, so a move may carry
        // one axis alone.
        assert_eq!(
            feed(&mut decoder, &[(input::EV_ABS, input::ABS_X, 310), REPORT]),
            vec![moved(310, 400)]
        );
        assert_eq!(
            feed(
                &mut decoder,
                &[(input::EV_KEY, input::BTN_TOUCH, 0), REPORT]
            ),
            vec![up(310, 400)]
        );
        assert!(decoder.is_quiescent());
        // A second tap in the same place sends no position at all.
        assert_eq!(
            feed(
                &mut decoder,
                &[(input::EV_KEY, input::BTN_TOUCH, 1), REPORT]
            ),
            vec![down(310, 400)]
        );
    }

    #[test]
    fn a_single_touch_press_before_any_position_waits_for_one() {
        let mut decoder = TouchDecoder::new(ContactReporting::SingleTouch);
        assert!(feed(
            &mut decoder,
            &[(input::EV_KEY, input::BTN_TOUCH, 1), REPORT]
        )
        .is_empty());
        assert_eq!(
            feed(
                &mut decoder,
                &[
                    (input::EV_ABS, input::ABS_X, 300),
                    (input::EV_ABS, input::ABS_Y, 400),
                    REPORT
                ]
            ),
            vec![down(300, 400)]
        );
    }

    #[test]
    fn a_tracked_controller_ignores_its_mirrored_single_touch_axes() {
        // The Clara BW sends ABS_X and ABS_Y beside its multi-touch axes.
        // Reading them as a second source of contacts would change a measured
        // device, so the default decoder must not.
        let mut decoder = TouchDecoder::default();
        let frame = [
            (input::EV_ABS, input::ABS_X, 300),
            (input::EV_ABS, input::ABS_Y, 400),
            (input::EV_KEY, input::BTN_TOUCH, 1),
            REPORT,
        ];
        assert!(feed(&mut decoder, &frame).is_empty());
    }

    #[test]
    fn an_untracked_controller_recovers_from_an_overflow_without_a_kernel_query() {
        let mut decoder = TouchDecoder::new(ContactReporting::Untracked);
        feed(
            &mut decoder,
            &[contact(300, 400).as_slice(), &[REPORT]].concat(),
        );
        assert_eq!(
            feed(
                &mut decoder,
                &[(input::EV_SYN, input::SYN_DROPPED, 0), REPORT]
            ),
            vec![TouchEvent::Cancel]
        );
        assert!(decoder.needs_resynchronization());
        decoder.resynchronize(None);
        assert!(decoder.is_quiescent());
        assert_eq!(
            feed(
                &mut decoder,
                &[contact(500, 600).as_slice(), &[REPORT]].concat()
            ),
            vec![down(500, 600)]
        );
    }

    #[test]
    fn the_reporting_style_follows_what_the_device_declares() {
        assert_eq!(
            ContactReporting::from_capabilities(true, true, true),
            Some(ContactReporting::Tracked)
        );
        assert_eq!(
            ContactReporting::from_capabilities(false, true, true),
            Some(ContactReporting::Untracked)
        );
        assert_eq!(
            ContactReporting::from_capabilities(false, false, true),
            Some(ContactReporting::SingleTouch)
        );
        assert_eq!(
            ContactReporting::from_capabilities(false, false, false),
            None
        );
    }
}
