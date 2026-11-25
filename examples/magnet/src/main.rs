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
    action_id, ActionId, Context, DenyReason, DeviceRequest, DeviceResult, DisplayMetrics, KoboApp,
    PictureHandle, Screen, ScreenBuilder, StoreResult, TilePicture,
};
use std::process::ExitCode;

/// Where the sweep and what it found are kept between openings.
const SAVED: &str = "magnet-sweep-v1";
const DIAGRAM: PictureHandle = PictureHandle(1);
const DIAGRAM_WIDTH: u32 = 360;
const DIAGRAM_HEIGHT: u32 = 480;

/// The four edges of the reader, as somebody holding one would name them.
///
/// The sensor is a single point behind a featureless bezel, and nothing in the
/// runtime knows where: the profiles describe the panel, not the magnet. So the
/// application does not claim to know either. It walks through the edges one at
/// a time and writes down which of them answered, which is the thing somebody
/// opened this to find out.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Edge {
    #[default]
    Top,
    Right,
    Bottom,
    Left,
}

impl Edge {
    const ALL: [Self; 4] = [Self::Top, Self::Right, Self::Bottom, Self::Left];

    const fn label(self) -> &'static str {
        match self {
            Self::Top => "top edge",
            Self::Right => "right edge",
            Self::Bottom => "bottom edge",
            Self::Left => "left edge",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Top => 0,
            Self::Right => 1,
            Self::Bottom => 2,
            Self::Left => 3,
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }
}

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
    /// The edge being swept now.
    sweeping: Edge,
    /// How many changes each edge has answered with, this run.
    per_edge: [u32; 4],
    /// The edge that answered last time this was used, which is the whole
    /// finding and the only part worth keeping.
    found: Option<Edge>,
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
            // Credited to the edge being swept, because that is the question:
            // not how many times it moved, but where it was when it did.
            self.per_edge[self.sweeping.index()] += 1;
            self.found = Some(self.sweeping);
        } else if magnet_present {
            // A magnet already against the bezel when the sensor first
            // answers is not a change, but it is an answer, and it is the
            // edge being swept that it answered at. Left uncredited, the
            // screen said "Magnet found" beside "Found on: nothing yet".
            self.found = Some(self.sweeping);
        }
        self.sensor = Sensor::Watching { magnet_present };
    }

    fn encode(&self) -> String {
        self.found
            .map_or_else(|| "none".to_owned(), |edge| edge.index().to_string())
    }

    fn decode(&mut self, text: &str) {
        // A magnet found this run, before the store answered, is newer than
        // whatever was written down last time, and the sweep is already on
        // its edge.
        if self.found.is_some() {
            return;
        }
        self.found = text
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| Edge::ALL.get(index).copied());
        // The sweep starts where the sensor was last found, so somebody
        // checking a reader they have used before begins at the answer.
        if let Some(edge) = self.found {
            self.sweeping = edge;
        }
    }

    /// The reader, drawn, with the edge being swept marked along it and a
    /// disc where the sensor answered.
    ///
    /// A diagram rather than a sentence because the question is where on a
    /// featureless bezel to hold a magnet, and "the right edge" read from a
    /// screen held in the other hand is one rotation away from being wrong.
    fn diagram(&self) -> Vec<u8> {
        let width = i32::try_from(DIAGRAM_WIDTH).expect("width fits");
        let height = i32::try_from(DIAGRAM_HEIGHT).expect("height fits");
        let mut pixels =
            vec![248u8; usize::try_from(width * height).expect("the diagram has an area")];
        let (left, top, right, bottom) = (40, 40, width - 40, height - 40);
        let inside = |x: i32, y: i32| (0..width).contains(&x) && (0..height).contains(&y);
        // Only ever called for coordinates `inside` has just admitted, so the
        // conversion cannot be negative.
        let at = |x: i32, y: i32| usize::try_from(y * width + x).unwrap_or(0);
        // The reader itself: a plain outline, with the panel inside it so the
        // bezel the magnet is held against is the part that reads as bezel.
        for x in left..=right {
            for thickness in 0..3 {
                for y in [top + thickness, bottom - thickness] {
                    if inside(x, y) {
                        pixels[at(x, y)] = 40;
                    }
                }
            }
        }
        for y in top..=bottom {
            for thickness in 0..3 {
                for x in [left + thickness, right - thickness] {
                    if inside(x, y) {
                        pixels[at(x, y)] = 40;
                    }
                }
            }
        }
        for y in top + 26..bottom - 26 {
            for x in left + 26..right - 26 {
                if inside(x, y) {
                    pixels[at(x, y)] = 232;
                }
            }
        }
        // The edge being swept, drawn along the bezel as a stripe.
        let stripe: Vec<(i32, i32)> = match self.sweeping {
            Edge::Top => (left + 14..=right - 14)
                .flat_map(|x| (0..10).map(move |step| (x, top + 8 + step)))
                .collect(),
            Edge::Bottom => (left + 14..=right - 14)
                .flat_map(|x| (0..10).map(move |step| (x, bottom - 8 - step)))
                .collect(),
            Edge::Left => (top + 14..=bottom - 14)
                .flat_map(|y| (0..10).map(move |step| (left + 8 + step, y)))
                .collect(),
            Edge::Right => (top + 14..=bottom - 14)
                .flat_map(|y| (0..10).map(move |step| (right - 8 - step, y)))
                .collect(),
        };
        for (x, y) in stripe {
            if inside(x, y) {
                pixels[at(x, y)] = 40;
            }
        }
        if let Some(found) = self.found {
            // Where it answered, drawn as a ring on that edge rather than a
            // second stripe: a stripe would read as a second thing to sweep.
            let (centre_x, centre_y) = match found {
                Edge::Top => ((left + right) / 2, top + 13),
                Edge::Bottom => ((left + right) / 2, bottom - 13),
                Edge::Left => (left + 13, (top + bottom) / 2),
                Edge::Right => (right - 13, (top + bottom) / 2),
            };
            for offset_y in -16..=16 {
                for offset_x in -16..=16 {
                    let (x, y) = (centre_x + offset_x, centre_y + offset_y);
                    let distance = offset_x * offset_x + offset_y * offset_y;
                    if !inside(x, y) {
                        continue;
                    }
                    if distance <= 16 * 16 {
                        pixels[at(x, y)] = 248;
                    }
                    if (11 * 11..=16 * 16).contains(&distance) {
                        pixels[at(x, y)] = 40;
                    }
                }
            }
        }
        pixels
    }

    /// The screen as a Clara draws it at the default text size.
    #[cfg(test)]
    fn screen(&self, picture: Option<TilePicture>) -> Screen {
        self.screen_at(picture, &kobo_sdk::CLARA_BW_METRICS)
    }

    /// The screen for `metrics`, which the guidance line is measured
    /// against so that it wraps the same with the magnet there or not.
    fn screen_at(&self, picture: Option<TilePicture>, metrics: &DisplayMetrics) -> Screen {
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
            Sensor::Watching { magnet_present } => {
                let mut screen = builder
                    // "Magnet found" rather than "Magnet", which only said
                    // the bar's title a second time in larger type.
                    .heading(if magnet_present {
                        "Magnet found"
                    } else {
                        "No magnet"
                    })
                    .secondary(self.guidance(metrics));
                if let Some(picture) = picture {
                    screen = screen.unframed_picture(picture, 46);
                }
                // One list, so its answers share a column. Set as two, the
                // first answer stood a word's width right of the three under
                // it, because each list lines its answers up on its own
                // labels.
                screen = screen.facts([
                    (
                        "Against the bezel",
                        if magnet_present { "Yes" } else { "Not now" }.to_owned(),
                    ),
                    ("Sweeping", self.sweeping.label().to_owned()),
                    (
                        "Changes here",
                        self.per_edge[self.sweeping.index()].to_string(),
                    ),
                    (
                        "Found on",
                        self.found.map_or_else(
                            || "nothing yet".to_owned(),
                            |edge| edge.label().to_owned(),
                        ),
                    ),
                ]);
                self.controls(screen)
            }
        }
    }

    /// What to do next, in one line, against the diagram.
    ///
    /// The count is the one for the edge the line tells you to hold the
    /// magnet against, the same number as "Changes here" below it. Counted
    /// over every edge, the line said "2 changes so far" about a right edge
    /// whose own count read 0. And "Nothing has answered yet" is kept for a
    /// reader where nothing ever has: once an edge is found, this run's or
    /// the one saved from last time, saying so sat beside "Found on".
    ///
    ///
    /// While the magnet is against the bezel there is nothing to do but read
    /// the answer, so the line gives it. It used to keep telling a reader
    /// holding a magnet that had just been found to hold a magnet there.
    ///
    /// The answer is shorter than the instruction, and the sweep and reset
    /// buttons sit under this line, so they would rise as the magnet arrived
    /// and fall as it went, under a finger already on its way. So the answer
    /// is set on as many lines as the instruction takes at this size, a
    /// sentence to a line.
    fn guidance(&self, metrics: &DisplayMetrics) -> String {
        let here = match self.per_edge[self.sweeping.index()] {
            1 => "1 change so far.".to_owned(),
            count => format!("{count} changes so far."),
        };
        let lead = format!(
            "Hold a magnet against the {} and move it slowly along.",
            self.sweeping.label()
        );
        let instruction = if self.found.is_none() {
            format!("{lead} Nothing has answered yet.")
        } else {
            format!("{lead} {here}")
        };
        if self.sensor
            != (Sensor::Watching {
                magnet_present: true,
            })
        {
            return instruction;
        }
        let found = format!("Found on the {}.", self.sweeping.label());
        match lines_of(&instruction, metrics) {
            0 | 1 => format!("{found} {here}"),
            lines => format!("{found}{}{here}", "\n".repeat(lines - 1)),
        }
    }

    /// Moving to the next edge is always offered; clearing a count is offered
    /// only once there is one. A control that does nothing is worse here than
    /// no control at all: pressing it costs a refresh and returns the same
    /// screen, which reads as the application having missed the tap.
    fn controls(&self, builder: ScreenBuilder) -> Screen {
        let builder = builder.button(
            NEXT_EDGE,
            format!("Sweep the {}", self.sweeping.next().label()),
        );
        if self.changes == 0 {
            builder.build()
        } else {
            builder.button(RESET, "Reset the count").build()
        }
    }
}

/// How many lines `text` takes as the line under the heading, at `metrics`.
fn lines_of(text: &str, metrics: &DisplayMetrics) -> usize {
    ScreenBuilder::new("magnet")
        .top_bar("Magnet")
        .secondary(text)
        .build()
        .layout_with(metrics, &kobo_sdk::Chrome::measuring(true))
        .nodes
        .iter()
        .find(|node| {
            node.text_lines
                .first()
                .is_some_and(|line| text.starts_with(line.as_str()))
        })
        .map_or(1, |node| node.text_lines.len())
}

const RESET: &str = "reset";
const NEXT_EDGE: &str = "next-edge";

impl Magnet {
    /// Draws, putting the diagram in front of the runtime first.
    fn show(&self, context: &mut Context) {
        let picture = matches!(self.sensor, Sensor::Watching { .. })
            .then(|| context.put_picture(DIAGRAM, DIAGRAM_WIDTH, DIAGRAM_HEIGHT, self.diagram()))
            .flatten();
        let drawn = self.screen_at(picture, &context.metrics());
        context.set_screen(drawn);
    }
}

impl KoboApp for Magnet {
    fn on_start(&mut self, context: &mut Context) {
        // Asked once, because edges are not the state: a magnet that was
        // already there when this opened produced no event and never will.
        context.device().read_cover();
        context.store().load(SAVED);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == SAVED {
                if let Some(text) = value.and_then(|bytes| String::from_utf8(bytes).ok()) {
                    self.decode(&text);
                }
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id(RESET) && self.changes != 0 {
            self.changes = 0;
            self.per_edge = [0; 4];
            self.show(context);
        } else if action == action_id(NEXT_EDGE) {
            self.sweeping = self.sweeping.next();
            self.show(context);
        }
    }

    fn on_cover_change(&mut self, context: &mut Context, magnet_present: bool) {
        let before = self.found;
        self.observe(magnet_present);
        if self.found != before {
            // Worth writing down the moment it is known: somebody calibrating
            // a reader has both hands full and will close the application by
            // putting the cover on it.
            context.store().save(SAVED, self.encode());
        }
        self.show(context);
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
            } => {
                let before = self.found;
                self.observe(magnet_present);
                // A magnet there from the start is a finding too, written
                // down as one met mid-sweep is.
                if self.found != before {
                    context.store().save(SAVED, self.encode());
                }
            }
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
        self.show(context);
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
    use super::{Edge, Magnet, Sensor, DIAGRAM, DIAGRAM_HEIGHT, DIAGRAM_WIDTH, NEXT_EDGE, RESET};
    use kobo_sdk::{action_id, TilePicture};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    /// The diagram, as the runtime would hand it back.
    fn drawn() -> TilePicture {
        TilePicture::new(DIAGRAM, DIAGRAM_WIDTH, DIAGRAM_HEIGHT)
    }

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
        let screen = Magnet::default().screen(Some(drawn()));
        let text = format!("{screen:?}");
        assert!(text.contains("Asking the sensor"), "{text}");
        assert!(!text.contains("No magnet"), "{text}");
    }

    #[test]
    fn a_magnet_against_the_bezel_is_named_as_found_rather_than_by_the_app_name() {
        let screen = watching(true).screen(Some(drawn()));
        let headings: Vec<String> = screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Heading { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(headings, ["Magnet found"]);
    }

    #[test]
    fn a_reader_without_a_sensor_says_so_and_offers_nothing_to_press() {
        let app = Magnet {
            sensor: Sensor::Unavailable("This reader has no hall sensor."),
            ..Magnet::default()
        };
        let layout = app
            .screen(Some(drawn()))
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            layout.rect_of_action(action_id(RESET)).is_none(),
            "a reset for a count that cannot move"
        );
    }

    #[test]
    fn nothing_is_offered_until_there_is_a_count_to_clear() {
        let quiet = watching(false)
            .screen(Some(drawn()))
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(quiet.rect_of_action(action_id(RESET)).is_none());
        let mut moved = watching(false);
        moved.observe(true);
        let busy = moved
            .screen(Some(drawn()))
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(busy.rect_of_action(action_id(RESET)).is_some());
    }

    #[test]
    fn both_states_fit_the_panel_and_keep_the_reset_in_one_place() {
        let mut app = watching(false);
        app.observe(true);
        app.observe(false);
        let absent = app
            .screen(Some(drawn()))
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        app.observe(true);
        let present = app
            .screen(Some(drawn()))
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

    /// The whole point of the application: which edge answered. The count is
    /// credited to the edge being swept when it moved, and that finding is
    /// written down rather than lost when the cover closes on it.
    #[test]
    fn a_change_is_credited_to_the_edge_being_swept_and_is_written_down() {
        let mut app = watching(false);
        app.sweeping = Edge::Right;
        app.observe(true);
        assert_eq!(app.found, Some(Edge::Right));
        assert_eq!(app.per_edge[Edge::Right.index()], 1);
        assert_eq!(app.per_edge[Edge::Top.index()], 0);

        let written = app.encode();
        let mut reopened = Magnet::default();
        reopened.decode(&written);
        assert_eq!(reopened.found, Some(Edge::Right));
        assert_eq!(
            reopened.sweeping,
            Edge::Right,
            "a reader that has been calibrated starts at the answer"
        );
        // Nonsense in the store is not a finding.
        let mut fresh = Magnet::default();
        fresh.decode("none");
        assert_eq!(fresh.found, None);
    }

    /// Found means found: the line gives the answer rather than telling a
    /// reader holding the magnet to hold the magnet there.
    #[test]
    fn a_found_magnet_gives_the_edge_and_the_count_instead_of_the_instruction() {
        let mut app = watching(false);
        app.observe(true);
        app.observe(false);
        app.observe(true);
        let text = format!("{:?}", app.screen(Some(drawn())));
        assert!(
            text.contains("Found on the top edge. 3 changes so far."),
            "{text}"
        );
        assert!(!text.contains("Hold a magnet"), "{text}");
        app.observe(false);
        let text = format!("{:?}", app.screen(Some(drawn())));
        assert!(
            text.contains("Hold a magnet against the top edge"),
            "{text}"
        );
    }

    /// A magnet already against the bezel when the application opens is an
    /// answer at the edge being swept, so "Magnet found" never sits beside
    /// "Found on: nothing yet". It is still not a change: nothing moved.
    #[test]
    fn a_magnet_there_from_the_start_is_found_on_the_edge_being_swept() {
        let app = watching(true);
        assert_eq!(app.found, Some(Edge::Top));
        assert_eq!(app.changes, 0);
        let text = format!("{:?}", app.screen(Some(drawn())));
        assert!(!text.contains("nothing yet"), "{text}");
        assert!(
            text.contains("Found on the top edge. 0 changes so far."),
            "{text}"
        );
        // The store answering after the sensor does not take it back.
        let mut later = watching(true);
        later.decode("1");
        assert_eq!(later.found, Some(Edge::Top));
        assert_eq!(later.sweeping, Edge::Top);
    }

    /// The controls stay put as the magnet comes and goes, on every edge and
    /// at every text size, because a finger is on its way to them while the
    /// other hand holds the magnet.
    #[test]
    fn the_controls_stay_put_as_the_magnet_comes_and_goes_at_every_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            for edge in Edge::ALL {
                let mut app = watching(false);
                app.sweeping = edge;
                app.observe(true);
                app.observe(false);
                let place = |app: &Magnet| {
                    let layout = app
                        .screen_at(Some(drawn()), &metrics)
                        .layout_with(&metrics, &Chrome::default());
                    (
                        layout.rect_of_action(action_id(NEXT_EDGE)),
                        layout.rect_of_action(action_id(RESET)),
                    )
                };
                let absent = place(&app);
                app.observe(true);
                assert_eq!(absent, place(&app), "{text_scale:?} {edge:?}");
            }
        }
    }

    /// The four edges are offered in order and come back round.
    #[test]
    fn the_sweep_walks_the_edges_and_says_which_one_is_next() {
        let mut runner = kobo_sdk::AppRunner::new(watching(false));
        for expected in [Edge::Right, Edge::Bottom, Edge::Left, Edge::Top] {
            runner.action(action_id(NEXT_EDGE));
            assert_eq!(runner.app().sweeping, expected);
        }
        let drawn = runner
            .app()
            .screen(Some(drawn()))
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(drawn.contains("top edge"), "{drawn}");
        assert!(drawn.contains("Sweep the right edge"), "{drawn}");
    }

    /// Every state this can be in, at every size the interface offers.
    #[test]
    fn every_state_fits_each_supported_text_size() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_ui::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let states = [
                Magnet::default(),
                Magnet {
                    sensor: Sensor::Unavailable("This reader has no hall sensor."),
                    ..Magnet::default()
                },
                watching(false),
                {
                    let mut swept = watching(false);
                    swept.sweeping = Edge::Right;
                    swept.observe(true);
                    swept
                },
            ];
            for app in states {
                let diagnostics = app
                    .screen(Some(drawn()))
                    .diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?} {:?}: {:?}",
                    app.sensor,
                    diagnostics.issues
                );
            }
        }
    }

    /// The diagram marks the edge being swept and, once there is one, the
    /// edge that answered.
    #[test]
    fn the_diagram_marks_the_swept_edge_and_the_one_that_answered() {
        let width = usize::try_from(DIAGRAM_WIDTH).expect("width");
        let ink = |app: &Magnet, x: usize, y: usize| app.diagram()[y * width + x];
        let mut app = watching(false);
        let height = usize::try_from(DIAGRAM_HEIGHT).expect("height");
        assert_eq!(ink(&app, width / 2, 50), 40, "the top edge is not marked");
        assert_ne!(
            ink(&app, width - 50, height / 2),
            40,
            "another edge was marked"
        );
        app.sweeping = Edge::Right;
        assert_eq!(
            ink(&app, width - 50, height / 2),
            40,
            "the swept edge moved but the diagram did not"
        );
        app.observe(true);
        assert_eq!(app.found, Some(Edge::Right));
        assert_eq!(
            ink(&app, width - 53, height / 2),
            248,
            "the answer is not ringed on its edge"
        );
    }

    #[test]
    fn the_count_is_reported_once_it_is_worth_reporting() {
        let mut app = watching(false);
        assert!(!format!("{:?}", app.screen(Some(drawn()))).contains("so far"));
        app.observe(true);
        assert!(format!("{:?}", app.screen(Some(drawn()))).contains("1 change so far"));
        app.observe(false);
        assert!(format!("{:?}", app.screen(Some(drawn()))).contains("2 changes so far"));
    }

    /// The line names an edge, so its count is that edge's: summed over the
    /// sweep, the right edge read "2 changes so far" beside "Changes here 0".
    #[test]
    fn the_count_in_the_line_is_the_count_for_the_edge_it_names() {
        let mut runner = kobo_sdk::AppRunner::new(watching(false));
        runner.app_mut().observe(true);
        runner.app_mut().observe(false);
        runner.action(action_id(NEXT_EDGE));
        let text = format!("{:?}", runner.app().screen(Some(drawn())));
        assert!(text.contains("right edge"), "{text}");
        assert!(text.contains("0 changes so far"), "{text}");
        assert!(!text.contains("2 changes so far"), "{text}");
    }

    /// A finding saved from last time is an answer, so a reopened reader no
    /// longer says nothing has answered while "Found on" names an edge.
    #[test]
    fn a_reader_found_before_does_not_say_nothing_has_answered() {
        let mut reopened = Magnet::default();
        reopened.decode("0");
        reopened.observe(false);
        let text = format!("{:?}", reopened.screen(Some(drawn())));
        assert!(text.contains("top edge"), "{text}");
        assert!(!text.contains("Nothing has answered yet"), "{text}");
        assert!(text.contains("0 changes so far"), "{text}");
    }

    /// Clearing the count keeps the finding, so the line keeps agreeing with
    /// it rather than going back to saying nothing has answered.
    #[test]
    fn a_reset_count_still_agrees_with_the_edge_that_was_found() {
        let mut runner = kobo_sdk::AppRunner::new(watching(false));
        runner.app_mut().observe(true);
        runner.action(action_id(RESET));
        let text = format!("{:?}", runner.app().screen(Some(drawn())));
        assert_eq!(runner.app().found, Some(Edge::Top));
        assert!(!text.contains("Nothing has answered yet"), "{text}");
        assert!(text.contains("0 changes so far"), "{text}");
    }
}
