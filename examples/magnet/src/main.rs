//! Where the magnet is, and nothing else.
//!
//! The reader has a hall sensor behind one edge of the bezel. It is the thing
//! a sleep cover closes against, and until now it was the runtime's private
//! business. This is the whole of the public surface, used honestly: ask once
//! for the state, then wait to be told when it changes.
//!
//! It is also the calibration screen. The sensor is a single point behind a
//! featureless bezel and nothing on the case says where, so the first thing
//! anybody wants is to walk a magnet along the edges and watch for the moment
//! it answers. The count is there for exactly that: a magnet moved slowly past
//! the threshold can bounce, and a number that jumps by six tells you that
//! before a gesture built on this does.

use kobo_sdk::{
    action_id, ActionId, Context, DenyReason, DeviceRequest, DeviceResult, Glyph, KoboApp, Screen,
    ScreenBuilder,
};
use std::process::ExitCode;

/// What the sensor has told us so far.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Sensor {
    /// Nothing has answered yet. Distinct from "no magnet", because a screen
    /// that says "no magnet" before it has asked is guessing.
    #[default]
    Unasked,
    Watching {
        magnet_present: bool,
    },
    /// This reader has no sensor, or this application may not watch it.
    Unavailable(&'static str),
}

#[derive(Clone, Debug, Default)]
struct Magnet {
    sensor: Sensor,
    /// Edges seen since the screen opened. Reset rather than cumulative,
    /// because the number is only useful against a run you just did.
    changes: u32,
}

impl Magnet {
    fn observe(&mut self, magnet_present: bool) {
        // A repeat is not a change. The runtime already settles bounce, but an
        // answer to `read_cover` can restate what an edge just said, and that
        // must not show up as movement nobody made.
        if self.sensor == (Sensor::Watching { magnet_present }) {
            return;
        }
        if matches!(self.sensor, Sensor::Watching { .. }) {
            self.changes += 1;
        }
        self.sensor = Sensor::Watching { magnet_present };
    }

    fn screen(&self) -> Screen {
        let builder = ScreenBuilder::new("magnet").top_bar("Magnet");
        match self.sensor {
            Sensor::Unasked => builder
                .splash(None, "Asking the sensor", "One moment.")
                .build(),
            Sensor::Unavailable(reason) => builder.splash(None, "No sensor", reason).build(),
            // The glyph appears with the magnet and goes with it. On this
            // panel a picture arriving is a far louder signal than a word
            // changing, and the whole point of the screen is to be readable
            // from across the room while your hands are busy holding a magnet
            // against the bezel.
            Sensor::Watching {
                magnet_present: true,
            } => self.reset_button(builder.splash(
                Some(Glyph::Magnet),
                "Magnet",
                self.summary("Something magnetic is against the bezel."),
            )),
            Sensor::Watching {
                magnet_present: false,
            } => self.reset_button(builder.splash(
                None,
                "No magnet",
                self.summary(
                    "Walk a magnet slowly along each edge until this changes. \
                     The sensor is one point behind the bezel and nothing marks it.",
                ),
            )),
        }
    }

    /// Offered only once there is a count to clear. A control that does
    /// nothing is worse here than no control at all: on this panel pressing it
    /// costs a refresh and returns the same screen, which reads as the
    /// application having missed the tap.
    fn reset_button(&self, builder: ScreenBuilder) -> Screen {
        if self.changes == 0 {
            builder.build()
        } else {
            builder.button(RESET, "Reset the count").build()
        }
    }

    fn summary(&self, lead: &str) -> String {
        match self.changes {
            0 => lead.to_owned(),
            1 => format!("{lead}\n\n1 change so far."),
            count => format!("{lead}\n\n{count} changes so far."),
        }
    }
}

const RESET: &str = "reset";

impl KoboApp for Magnet {
    fn on_start(&mut self, context: &mut Context) {
        // Asked once, because edges are not the state: a magnet that was
        // already there when this opened produced no event and never will.
        context.device().read_cover();
        context.set_screen(self.screen());
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(RESET) && self.changes != 0 {
            self.changes = 0;
            context.set_screen(self.screen());
        }
    }

    fn on_cover_change(&mut self, context: &mut Context, magnet_present: bool) {
        self.observe(magnet_present);
        context.set_screen(self.screen());
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if !matches!(request, DeviceRequest::ReadCover) {
            return;
        }
        match result {
            DeviceResult::Cover {
                available: true,
                magnet_present,
            } => self.observe(magnet_present),
            DeviceResult::Cover {
                available: false, ..
            } => self.sensor = Sensor::Unavailable("This reader has no hall sensor."),
            DeviceResult::Denied(DenyReason::Unsupported) => {
                self.sensor = Sensor::Unavailable("This build cannot read the hall sensor.");
            }
            DeviceResult::Denied(DenyReason::NotDeclared) => {
                self.sensor =
                    Sensor::Unavailable("This application did not ask for the cover sensor.");
            }
            DeviceResult::Denied(_) | DeviceResult::Failed(_) => {
                self.sensor = Sensor::Unavailable("The sensor could not be read.");
            }
            _ => return,
        }
        context.set_screen(self.screen());
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("magnet", Magnet::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("magnet: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Magnet, Sensor, RESET};
    use kobo_sdk::action_id;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn watching(magnet_present: bool) -> Magnet {
        let mut app = Magnet::default();
        app.observe(magnet_present);
        app
    }

    /// The first answer establishes the state. It is not a change, because
    /// nothing moved: the magnet was already wherever it was.
    #[test]
    fn the_first_answer_is_not_counted_as_a_change() {
        let app = watching(true);
        assert_eq!(app.changes, 0);
        assert_eq!(
            app.sensor,
            Sensor::Watching {
                magnet_present: true
            }
        );
    }

    #[test]
    fn only_real_movement_is_counted() {
        let mut app = watching(false);
        app.observe(false);
        app.observe(false);
        assert_eq!(app.changes, 0, "a restated state is not movement");
        app.observe(true);
        app.observe(false);
        assert_eq!(app.changes, 2);
    }

    /// Before anything has answered the screen must not claim there is no
    /// magnet, because it has not looked.
    #[test]
    fn nothing_is_claimed_before_the_sensor_answers() {
        let screen = Magnet::default().screen();
        let text = format!("{screen:?}");
        assert!(text.contains("Asking the sensor"), "{text}");
        assert!(!text.contains("No magnet"), "{text}");
    }

    #[test]
    fn a_reader_without_a_sensor_says_so_and_offers_nothing_to_press() {
        let app = Magnet {
            sensor: Sensor::Unavailable("This reader has no hall sensor."),
            changes: 0,
        };
        let layout = app
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            layout.rect_of_action(action_id(RESET)).is_none(),
            "a reset for a count that cannot move"
        );
    }

    #[test]
    fn nothing_is_offered_until_there_is_a_count_to_clear() {
        let quiet = watching(false)
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(quiet.rect_of_action(action_id(RESET)).is_none());
        let mut moved = watching(false);
        moved.observe(true);
        let busy = moved
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(busy.rect_of_action(action_id(RESET)).is_some());
    }

    #[test]
    fn both_states_fit_the_panel_and_keep_the_reset_in_one_place() {
        let mut app = watching(false);
        app.observe(true);
        app.observe(false);
        let absent = app
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        app.observe(true);
        let present = app
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let absent_reset = absent
            .rect_of_action(action_id(RESET))
            .expect("a reset button");
        let present_reset = present
            .rect_of_action(action_id(RESET))
            .expect("a reset button");
        assert!(
            absent_reset.y + absent_reset.height <= CLARA_BW_METRICS.height,
            "the reset button is off the bottom of the panel"
        );
        // The magnet arriving must not shift the only control on the screen,
        // or a finger already on its way lands somewhere else.
        assert_eq!(absent_reset, present_reset);
    }

    #[test]
    fn the_count_is_reported_once_it_is_worth_reporting() {
        let mut app = watching(false);
        assert!(!format!("{:?}", app.screen()).contains("so far"));
        app.observe(true);
        assert!(format!("{:?}", app.screen()).contains("1 change so far"));
        app.observe(false);
        assert!(format!("{:?}", app.screen()).contains("2 changes so far"));
    }
}
