//! A message sent in light, one letter at a time.
//!
//! The reader has a front light, and the front light is the only output on the
//! device that carries across a room. This turns it into a beacon: type a
//! message, and it goes out as Morse, the panel showing whichever letter the
//! light is sending at that moment.
//!
//! ## Why a beat is a whole second
//!
//! Morse is defined against a unit: a dot is one, a dash is three, and the
//! silences between are one, three and seven. Every one of those is a duration
//! the application has to wait out, and the only timer this platform offers an
//! application is `Task::Sleep`, which counts whole seconds. There is no way to
//! run work off the callback thread and no sub-second wake, so the unit here is
//! one second and cannot currently be anything else. That makes the beacon slow
//! -- `SOS` takes twenty-seven seconds -- and slow is the honest speed rather
//! than a setting that lies about what the runtime can do.
//!
//! Slow turns out to be the right answer anyway. The SDK's own guidance is that
//! flashing the front light is a photosensitivity hazard, and the hazard band
//! starts around three flashes a second. A one second unit puts a run of dots
//! at half a hertz, which is an order of magnitude clear of it.
//!
//! ## Why the letter is a picture
//!
//! The point of the screen here is to be read from across the room by somebody
//! who is not holding the device, so the letter wants the whole panel. The
//! largest type this platform sets is [`kobo_ui::FontSize::Heading`] at 5.4 mm,
//! which is a heading and not a signal, and pictures are never enlarged to fit
//! -- `fit_within` returns a small source untouched. So the letter is drawn
//! here, at the size it will be seen, out of a five by seven block alphabet.
//! Blocks rather than a real face because a glyph made of filled rectangles
//! costs a memset and needs no typesetter, and at this size the difference is
//! invisible.
//!
//! The picture is rebuilt when the *letter* changes, not when the light does.
//! That is what keeps the panel calm: a message of ten letters repaints ten
//! times across two minutes rather than once a second for the whole run.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, DeviceRequest, DeviceResult, KoboApp, PictureHandle,
    Screen, ScreenBuilder, Task, TaskId, TaskOutcome,
};
use kobo_ui::tone;
use std::process::ExitCode;
use std::time::Duration;

/// The one picture this application draws, reused for every letter.
///
/// One handle rather than one per letter: a full panel of grey is about a
/// megabyte, and a cache keyed by character would hold a dozen of those on the
/// runtime's side to save a redraw that happens once every few seconds.
const LETTER: PictureHandle = PictureHandle(1);

/// How tall the letter is while the beacon is sending, in tenths of a
/// millimetre.
///
/// This is the whole of the panel between the top bar and the controls, and it
/// is a fixed figure rather than a measurement because an application is never
/// handed the content rectangle -- only the panel and the bars it asked for.
/// The number is therefore pinned by
/// `the_letter_leaves_room_for_the_control_that_stops_it`, which fails the
/// moment it grows past what the layout will give it. A letter that pushed
/// Stop off the bottom edge would be a beacon with no way to end it.
const LETTER_SENDING_TENTHS_MM: i32 = 940;

/// How tall it is while nothing is going out.
///
/// Smaller, because the resting screen also carries the code about to be sent
/// and how long it will take, and those are what somebody is reading before
/// they press Send.
const LETTER_RESTING_TENTHS_MM: i32 = 700;

/// What the light does on a lit beat.
///
/// Full rather than a step up from wherever the reader had it: the beacon is
/// meant to be seen by somebody who is not holding the device, and a signal
/// that is merely brighter than the last one is not a signal at all.
const FULL: u8 = 100;

/// The longest message the beacon accepts, in characters.
///
/// At a second a beat, thirty characters is already several minutes of
/// standing still holding a reader up. The limit is here so the estimate on
/// screen stays a number somebody might agree to rather than one they scroll
/// past.
const MAX_MESSAGE: usize = 30;

/// One letter of the message, and the code that carries it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Signal {
    character: char,
    /// Dots and dashes. Empty for the space between words, which is a silence
    /// rather than a symbol.
    code: &'static str,
}

/// One second of the beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Beat {
    lit: bool,
    /// Which [`Signal`] this second belongs to.
    ///
    /// The silence after a letter is charged to the letter that just finished,
    /// not the one about to start, so the panel changes at the instant the new
    /// letter's first flash does. Charged forwards instead, the screen would
    /// announce a letter up to seven seconds before the light sent it.
    signal: usize,
}

/// The International Morse Code, and nothing beyond it.
///
/// Punctuation stops at the four marks that appear in a message somebody would
/// actually flash. The rest of the ITU table exists, but every entry added here
/// is another shape the block alphabet has to carry, and a message needing an
/// ampersand is not the message this is for.
const CODE: &[(char, &str)] = &[
    ('A', ".-"),
    ('B', "-..."),
    ('C', "-.-."),
    ('D', "-.."),
    ('E', "."),
    ('F', "..-."),
    ('G', "--."),
    ('H', "...."),
    ('I', ".."),
    ('J', ".---"),
    ('K', "-.-"),
    ('L', ".-.."),
    ('M', "--"),
    ('N', "-."),
    ('O', "---"),
    ('P', ".--."),
    ('Q', "--.-"),
    ('R', ".-."),
    ('S', "..."),
    ('T', "-"),
    ('U', "..-"),
    ('V', "...-"),
    ('W', ".--"),
    ('X', "-..-"),
    ('Y', "-.--"),
    ('Z', "--.."),
    ('0', "-----"),
    ('1', ".----"),
    ('2', "..---"),
    ('3', "...--"),
    ('4', "....-"),
    ('5', "....."),
    ('6', "-...."),
    ('7', "--..."),
    ('8', "---.."),
    ('9', "----."),
    ('.', ".-.-.-"),
    (',', "--..--"),
    ('?', "..--.."),
    ('/', "-..-."),
];

/// Turns a typed message into the letters that can be sent.
///
/// Anything with no code is dropped rather than sent as a pause, and the caller
/// is told what went missing: a beacon that silently skipped every apostrophe
/// would be transmitting a different message from the one on the screen.
fn encode(message: &str) -> (Vec<Signal>, Vec<char>) {
    let mut signals = Vec::new();
    let mut dropped = Vec::new();
    let mut pending_space = false;
    for character in message.chars().take(MAX_MESSAGE) {
        let upper = character.to_ascii_uppercase();
        if upper.is_whitespace() {
            // Held rather than pushed, so a run of spaces is one word gap and a
            // message ending in a space does not end in seven dark seconds.
            pending_space = !signals.is_empty();
            continue;
        }
        let Some((_, code)) = CODE.iter().find(|(letter, _)| *letter == upper) else {
            dropped.push(character);
            continue;
        };
        if pending_space {
            signals.push(Signal {
                character: ' ',
                code: "",
            });
            pending_space = false;
        }
        signals.push(Signal {
            character: upper,
            code,
        });
    }
    (signals, dropped)
}

/// Lays the letters out in seconds.
///
/// The proportions are the standard ones: a dot is one beat, a dash three, the
/// gap inside a letter one, between letters three, between words seven. The
/// gaps are emitted as part of the letter they follow, and the run is trimmed
/// so it ends on the last lit beat -- trailing darkness is indistinguishable
/// from the beacon having stopped, so it is time nobody can read.
fn beats(signals: &[Signal]) -> Vec<Beat> {
    let mut beats: Vec<Beat> = Vec::new();
    for (index, signal) in signals.iter().enumerate() {
        if signal.code.is_empty() {
            // A word gap is seven beats measured from the end of the last
            // letter, and three of them were already laid down by that letter.
            for _ in 0..4 {
                beats.push(Beat {
                    lit: false,
                    signal: index,
                });
            }
            continue;
        }
        for symbol in signal.code.chars() {
            let held = if symbol == '-' { 3 } else { 1 };
            for _ in 0..held {
                beats.push(Beat {
                    lit: true,
                    signal: index,
                });
            }
            beats.push(Beat {
                lit: false,
                signal: index,
            });
        }
        // One dark beat is already there from the symbol that just ended.
        for _ in 0..2 {
            beats.push(Beat {
                lit: false,
                signal: index,
            });
        }
    }
    while beats.last().is_some_and(|beat| !beat.lit) {
        beats.pop();
    }
    beats
}

/// A five by seven block alphabet, one `u8` a row, the fifth bit leftmost.
///
/// Only the characters [`CODE`] can send, plus the space between words, because
/// a shape with no code behind it could never reach the screen.
fn glyph(character: char) -> Option<[u8; 7]> {
    let rows = match character {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x19, 0x15, 0x13, 0x13, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => return None,
    };
    Some(rows)
}

/// Paints one letter at the size it will be seen, as grey bytes.
///
/// Returns the width, the height and the pixels. The cell is whatever divides
/// the room evenly, so the letter is always made of exact rectangles and never
/// of a scaled bitmap with one row of blocks a pixel taller than its neighbour.
fn paint(character: char, room: i32) -> Option<(u32, u32, Vec<u8>)> {
    let rows = glyph(character)?;
    let cell = usize::try_from((room / 7).max(1)).ok()?;
    let width = cell * 5;
    let height = cell * 7;
    let mut pixels = vec![tone::PAPER; width * height];
    for (index, bits) in rows.iter().enumerate() {
        for column in 0..5 {
            if bits & (0x10 >> column) == 0 {
                continue;
            }
            for y in index * cell..(index + 1) * cell {
                let start = y * width + column * cell;
                pixels[start..start + cell].fill(tone::INK);
            }
        }
    }
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return None;
    };
    Some((width, height, pixels))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    /// Typing the message.
    #[default]
    Writing,
    /// Sending it.
    Beacon,
}

#[derive(Debug)]
struct Morse {
    view: View,
    keyboard: Keyboard,
    signals: Vec<Signal>,
    beats: Vec<Beat>,
    /// The next beat to send.
    at: usize,
    running: bool,
    /// Whether the front light carries the message as well as the panel.
    ///
    /// On by default, because a beacon nobody can see from across the room is
    /// the feature switched off.
    light: bool,
    /// The brightness the reader had before this opened.
    ///
    /// Captured once and put back on the way out. Without it the beacon leaves
    /// the device at whatever the last beat happened to be, which for most
    /// messages is dark.
    found_light: Option<u8>,
    /// Which signal the panel is currently showing, so the picture is rebuilt
    /// on a change of letter rather than on every beat.
    showing: Option<usize>,
    tick: Option<TaskId>,
    trouble: Option<String>,
}

impl Default for Morse {
    fn default() -> Self {
        Self {
            view: View::default(),
            // Prefilled, so the first thing anybody opening this can do is send
            // something rather than think of something.
            keyboard: Keyboard::with_text("SOS"),
            signals: Vec::new(),
            beats: Vec::new(),
            at: 0,
            running: false,
            light: true,
            found_light: None,
            showing: None,
            tick: None,
            trouble: None,
        }
    }
}

const SEND: &str = "send";
const STOP: &str = "stop";
const AGAIN: &str = "again";
const EDIT: &str = "edit";
const TOGGLE_LIGHT: &str = "light";

impl Morse {
    /// The letters of the message, spelled out with their codes underneath.
    fn spelled(&self) -> String {
        self.signals
            .iter()
            .map(|signal| {
                if signal.code.is_empty() {
                    "/".to_owned()
                } else {
                    signal.code.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// How long the whole message takes, in words rather than a bare number.
    fn duration(&self) -> String {
        let seconds = self.beats.len();
        match seconds {
            0 => "nothing to send".to_owned(),
            1 => "1 second".to_owned(),
            seconds if seconds < 60 => format!("{seconds} seconds"),
            seconds => format!("{} min {} sec", seconds / 60, seconds % 60),
        }
    }

    fn writing(&self) -> Screen {
        let mut screen = ScreenBuilder::new("morse-writing").top_bar("Morse");
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        screen
            .typed(&self.keyboard, "A message to send in light")
            .keyboard(&self.keyboard, "Send")
            .build()
    }

    fn beacon(&self, context: &mut Context) -> Screen {
        // While it is sending, the progress goes in the top bar rather than
        // under the letter. It is the same fact either way, and a bar that
        // already exists costs the letter nothing, where a line of text under
        // it costs the letter that line on every panel.
        let title = if self.running {
            format!(
                "{} of {}",
                self.sent().min(self.signals.len().max(1)),
                self.signals.len()
            )
        } else {
            "Morse".to_owned()
        };
        let mut screen = ScreenBuilder::new("morse-beacon").top_bar(title);
        if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        }
        // Sending, the letter is the only thing on the screen and takes the
        // whole of it. Resting, it gives back the few lines the code and the
        // estimate need, because at that point the thing worth reading is what
        // is about to go out rather than what is going out now.
        let tenths = if self.running {
            LETTER_SENDING_TENTHS_MM
        } else {
            LETTER_RESTING_TENTHS_MM
        };
        let room = context.metrics().tenth_mm(tenths);
        let character = self
            .showing
            .and_then(|index| self.signals.get(index))
            .map_or(' ', |signal| signal.character);
        if let Some(picture) = paint(character, room)
            .and_then(|(width, height, grey)| context.put_picture(LETTER, width, height, grey))
        {
            let millimetres = u16::try_from(tenths / 10).unwrap_or(u16::MAX);
            screen = screen.picture(picture, millimetres);
        }
        if !self.running {
            screen = screen
                .text(self.spelled())
                .secondary(format!("{} to send.", self.duration()));
        }
        let light = if self.light { "Light on" } else { "Light off" };
        if self.running {
            screen.action_bar([(STOP, "Stop"), (TOGGLE_LIGHT, light)])
        } else {
            screen.action_bar([(AGAIN, "Send"), (EDIT, "Edit"), (TOGGLE_LIGHT, light)])
        }
        .build()
    }

    /// How many letters have gone out, counting the one in flight.
    fn sent(&self) -> usize {
        self.beats
            .get(self.at.min(self.beats.len().saturating_sub(1)))
            .map_or(0, |beat| beat.signal + 1)
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Writing => self.writing(),
            View::Beacon => self.beacon(context),
        };
        context.set_screen(screen);
    }

    /// Takes the typed message and works out what it will look like in light.
    fn compose(&mut self, message: &str) {
        let (signals, dropped) = encode(message);
        self.signals = signals;
        self.beats = beats(&self.signals);
        self.at = 0;
        self.showing = None;
        self.trouble = if dropped.is_empty() {
            None
        } else {
            let list: String = dropped.iter().collect();
            Some(format!("Morse has no code for {list}, so it was left out."))
        };
    }

    /// Starts sending, from the beginning.
    fn start(&mut self, context: &mut Context) {
        if self.beats.is_empty() {
            self.trouble = Some("There is nothing here to send.".to_owned());
            self.show(context);
            return;
        }
        self.at = 0;
        self.showing = None;
        self.running = true;
        // Whole messages take minutes, and a device that suspended halfway
        // through one would leave the light wherever the last beat put it.
        context
            .device()
            .keep_awake(Duration::from_secs(self.beats.len() as u64 + 30));
        self.play(context);
    }

    /// Sends the beat at the cursor and arranges to be woken for the next.
    fn play(&mut self, context: &mut Context) {
        let Some(beat) = self.beats.get(self.at).copied() else {
            self.finish(context);
            return;
        };
        if self.light {
            context
                .device()
                .set_frontlight(if beat.lit { FULL } else { 0 });
        }
        let changed = self.showing != Some(beat.signal);
        self.showing = Some(beat.signal);
        self.tick = context.spawn(Task::Sleep { seconds: 1 });
        if self.tick.is_none() {
            // Nothing else can advance the beacon, so it stops here rather than
            // standing with the light held on whatever the last beat was.
            self.trouble = Some("The beacon could not keep time, so it stopped.".to_owned());
            self.finish(context);
            return;
        }
        if changed {
            self.show(context);
        }
    }

    /// Ends the run, whether it finished or was stopped, and puts the device
    /// back the way it was found.
    fn finish(&mut self, context: &mut Context) {
        self.running = false;
        self.showing = None;
        if let Some(task) = self.tick.take() {
            context.cancel(task);
        }
        self.restore(context);
        context.device().allow_sleep();
        self.show(context);
    }

    /// Puts the front light back to the brightness the reader had.
    fn restore(&mut self, context: &mut Context) {
        if let Some(percent) = self.found_light {
            context.device().set_frontlight(percent);
        }
    }
}

impl KoboApp for Morse {
    fn on_start(&mut self, context: &mut Context) {
        // Asked before anything is flashed, because the answer is what gets put
        // back afterwards and the first beat would overwrite it.
        context.device().read_frontlight();
        self.compose(self.keyboard.text().to_owned().as_str());
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::Writing {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let typed = self.keyboard.text().trim().to_owned();
                    self.compose(&typed);
                    self.view = View::Beacon;
                    self.start(context);
                    return;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.show(context);
                    return;
                }
                None => {}
            }
        }

        if action == ActionId::BACK {
            match self.view {
                View::Writing => return,
                View::Beacon => {
                    if self.running {
                        self.finish(context);
                    }
                    self.view = View::Writing;
                }
            }
            self.show(context);
            return;
        }

        if action == action_id(TOGGLE_LIGHT) {
            self.light = !self.light;
            if self.light {
                // Caught up with the beat in flight rather than left dark until
                // the next one, which for a dash is three seconds away.
                if let Some(beat) = self.beats.get(self.at) {
                    let lit = beat.lit && self.running;
                    context.device().set_frontlight(if lit { FULL } else { 0 });
                }
            } else {
                self.restore(context);
            }
            self.show(context);
            return;
        }

        if action == action_id(STOP) {
            self.finish(context);
            return;
        }

        if action == action_id(SEND) || action == action_id(AGAIN) {
            self.trouble = None;
            self.view = View::Beacon;
            self.start(context);
            return;
        }

        if action == action_id(EDIT) {
            if self.running {
                self.finish(context);
            }
            self.view = View::Writing;
            self.show(context);
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.tick != Some(task) || !self.running {
            return;
        }
        self.tick = None;
        if matches!(outcome, TaskOutcome::Cancelled) {
            return;
        }
        self.at += 1;
        self.play(context);
    }

    fn on_device_result(
        &mut self,
        _context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        // Only the answer to the question asked at startup. Every beat sets the
        // light and every set is answered the same way, so taking the brightness
        // from any `Frontlight` result would record a beat as the reader's own
        // setting and put that back on the way out.
        if !matches!(request, DeviceRequest::ReadFrontlight) {
            return;
        }
        if let DeviceResult::Frontlight { percent } = result {
            self.found_light = Some(percent);
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        // The panel belongs to something else now, so the letter is going
        // nowhere and the light would be flashing over another application's
        // screen.
        if self.running {
            self.finish(context);
        }
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.restore(context);
        context.device().allow_sleep();
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("morse", Morse::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("morse: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        beats, encode, glyph, paint, Beat, Morse, AGAIN, CODE, MAX_MESSAGE, STOP, TOGGLE_LIGHT,
    };
    use kobo_sdk::{action_id, AppRunner, Command, DeviceRequest};
    use kobo_ui::{tone, Chrome, CLARA_BW_METRICS};

    /// Renders a run of beats as the light would show it, so a test can state
    /// the timing the way the specification does.
    fn lit(message: &str) -> String {
        beats(&encode(message).0)
            .iter()
            .map(|beat| if beat.lit { '#' } else { '.' })
            .collect()
    }

    #[test]
    fn a_letter_becomes_the_code_that_carries_it() {
        let (signals, dropped) = encode("sos");
        assert!(dropped.is_empty());
        let codes: Vec<_> = signals.iter().map(|signal| signal.code).collect();
        assert_eq!(codes, ["...", "---", "..."]);
    }

    #[test]
    fn a_message_is_read_the_same_whichever_case_it_was_typed_in() {
        assert_eq!(encode("SOS").0, encode("sos").0);
    }

    /// The proportions are the whole of Morse: get these wrong and the message
    /// is still dots and dashes, just not the ones that were meant.
    #[test]
    fn a_dash_is_held_three_times_as_long_as_a_dot() {
        assert_eq!(lit("e"), "#");
        assert_eq!(lit("t"), "###");
    }

    #[test]
    fn letters_are_separated_by_more_darkness_than_the_symbols_inside_them() {
        // I is two dots one beat apart; then three dark beats; then E.
        assert_eq!(lit("ie"), "#.#...#");
    }

    #[test]
    fn a_word_gap_is_the_longest_silence() {
        assert_eq!(lit("e e"), "#.......#");
    }

    #[test]
    fn a_run_of_spaces_is_still_one_word_gap() {
        assert_eq!(lit("e   e"), lit("e e"));
    }

    /// Darkness at the end is indistinguishable from the beacon having
    /// finished, so it is time spent saying nothing.
    #[test]
    fn the_beacon_ends_on_a_lit_beat() {
        for message in ["sos", "e", "hello world", "73"] {
            let run = beats(&encode(message).0);
            assert!(run.last().is_some_and(|beat| beat.lit), "{message}");
        }
    }

    #[test]
    fn a_message_that_opens_or_closes_with_a_space_does_not_send_one() {
        assert_eq!(lit("  e  "), lit("e"));
    }

    /// A character with no code must be reported, not quietly skipped: the
    /// message on the panel would otherwise not be the message in the light.
    #[test]
    fn a_character_morse_cannot_send_is_reported_rather_than_dropped_in_silence() {
        let (signals, dropped) = encode("it's");
        assert_eq!(dropped, ['\'']);
        let spelled: String = signals.iter().map(|signal| signal.character).collect();
        assert_eq!(spelled, "ITS");
    }

    #[test]
    fn a_long_message_is_cut_rather_than_sent_for_an_hour() {
        let (signals, _) = encode(&"e ".repeat(MAX_MESSAGE));
        assert!(signals.len() <= MAX_MESSAGE, "{}", signals.len());
    }

    /// Every code in the table needs a shape, or a message would send a letter
    /// in light that the panel could not name.
    #[test]
    fn every_letter_the_beacon_can_send_has_a_shape_to_draw() {
        for (character, _) in CODE {
            assert!(glyph(*character).is_some(), "no shape for {character}");
        }
        assert!(glyph(' ').is_some(), "the gap between words needs a blank");
    }

    #[test]
    fn a_painted_letter_is_the_size_it_says_it_is_and_has_ink_in_it() {
        let (width, height, pixels) = paint('A', 700).expect("a letter");
        assert_eq!(pixels.len(), (width * height) as usize);
        assert_eq!(width * 7, height * 5, "the cell is not square");
        assert!(pixels.contains(&tone::INK));
        assert!(pixels.contains(&tone::PAPER));
    }

    #[test]
    fn the_gap_between_words_is_painted_as_nothing_at_all() {
        let (_, _, pixels) = paint(' ', 700).expect("a blank");
        assert!(pixels.iter().all(|grey| *grey == tone::PAPER));
    }

    /// The screen names the letter the light is sending at that moment, so a
    /// beat belongs to the letter it came from and not the one coming next.
    #[test]
    fn the_silence_after_a_letter_belongs_to_the_letter_that_sent_it() {
        let run = beats(&encode("ie").0);
        let owners: Vec<usize> = run.iter().map(|beat: &Beat| beat.signal).collect();
        assert_eq!(owners, [0, 0, 0, 0, 0, 0, 1]);
    }

    /// The letter is the largest single object this platform draws, and it is
    /// drawn at a size chosen here rather than fitted by the renderer -- a
    /// picture that arrives too big is never shrunk. So the one thing that
    /// could go wrong is that it pushes Stop off the bottom of the panel,
    /// leaving a beacon running with no way to end it.
    #[test]
    fn the_letter_leaves_room_for_the_control_that_stops_it() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        let commands = runner.action(action_id(AGAIN));
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a beacon screen");
        let chrome = Chrome::measuring(true);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &chrome);
        let stop = layout
            .rect_of_action(action_id(STOP))
            .expect("a way to stop it");
        assert!(
            stop.y + stop.height <= CLARA_BW_METRICS.height,
            "Stop is off the panel at {stop:?}"
        );
        let issues = screen.diagnostics(&CLARA_BW_METRICS, &chrome);
        assert!(!issues.has_errors(), "the beacon does not fit: {issues:?}");
    }

    /// The resting screen carries the code and the estimate as well as the
    /// letter, so it is the one where the letter has to give room back.
    #[test]
    fn the_code_about_to_go_out_is_readable_before_it_does() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(AGAIN));
        // Stopped, so the beacon is resting and showing what it would send.
        let commands = runner.action(action_id(STOP));
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a screen");
        let chrome = Chrome::measuring(true);
        let issues = screen.diagnostics(&CLARA_BW_METRICS, &chrome);
        assert!(
            !issues.has_errors(),
            "the resting screen does not fit: {issues:?}"
        );
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("... --- ..."),
            "the code is not shown: {drawn}"
        );
        assert!(
            drawn.contains("27 seconds"),
            "the estimate is not shown: {drawn}"
        );
    }

    /// The brightness the reader chose is theirs, and a beacon that ended on a
    /// dark beat would otherwise leave the device black.
    #[test]
    fn the_light_is_put_back_where_it_was_found() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        // The only request outstanding is the one `on_start` made, so this is
        // the answer to it.
        runner.device_result(kobo_sdk::DeviceResult::Frontlight { percent: 42 });
        runner.app_mut().light = true;
        let commands = runner.action(action_id(TOGGLE_LIGHT));
        assert!(
            commands.iter().any(|command| matches!(
                command,
                Command::Device(DeviceRequest::SetFrontlight { percent: 42 })
            )),
            "{commands:?}"
        );
    }
}
