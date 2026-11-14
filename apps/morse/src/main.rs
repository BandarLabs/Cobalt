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
//! here, at the size it will be seen, from the outlines of the interface's
//! bold face. It was a five by seven block alphabet, which at nine centimetres
//! tall was a staircase of black rectangles rather than a letter.
//!
//! The picture is rebuilt when the *letter* changes, not when the light does.
//! That is what keeps the panel calm: a message of ten letters repaints ten
//! times across two minutes rather than once a second for the whole run.

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, DeviceRequest, DeviceResult, KoboApp, PictureHandle,
    Position, RowLead, Screen, ScreenBuilder, Task, TaskId, TaskOutcome,
};
use kobo_ui::tone;
use std::process::ExitCode;
use std::time::Duration;

/// Letters to a row of the letter reference.
const REFERENCE_COLUMNS: usize = 3;

/// A code as it is printed in a manual: a raised dot and a long dash with a
/// space between, where the full stops and hyphens it is stored in read as
/// punctuation.
fn marks(code: &str) -> String {
    code.chars()
        .map(|mark| if mark == '-' { "\u{2014}" } else { "\u{2022}" })
        .collect::<Vec<_>>()
        .join(" ")
}

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
    /// so that the count in the top bar says a letter has gone out once it
    /// actually has. What the panel *draws* is chosen separately, by
    /// [`letter_at`], and looks the other way.
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

/// The letter the light is sending at a given second, or the one it is about
/// to send.
///
/// The whole point of the panel here is that somebody across the room can read
/// the letter the light is flashing, so the letter has to be up before the
/// flash is, not after. E-ink takes the better part of a second to settle, and
/// a panel repainted at the instant the light came on would spend that first
/// flash still showing the letter before it -- which is the one thing the
/// screen exists not to do. So during a silence the beacon draws what is
/// coming rather than what has just gone.
///
/// Looking forwards also keeps the gap between words off the screen entirely.
/// It is a silence, not a symbol, and it has no shape of its own; drawn, its
/// blank filled the panel with an empty frame the size of a letter.
fn letter_at(beats: &[Beat], at: usize) -> Option<usize> {
    beats
        .get(at..)?
        .iter()
        .find(|beat| beat.lit)
        .map(|beat| beat.signal)
}

/// A number of beats said the way somebody deciding whether to wait would say
/// it, since a beat is a second and the figure is only there to be weighed
/// against the reader's patience.
fn spoken(seconds: usize) -> String {
    match seconds {
        0 => "nothing to send".to_owned(),
        1 => "1 second".to_owned(),
        seconds if seconds < 60 => format!("{seconds} seconds"),
        seconds => format!("{} min {} sec", seconds / 60, seconds % 60),
    }
}

/// The line under the message: how long it takes, or that there is nothing
/// in the box yet. "Nothing to send to send." was the empty case read out
/// through the same sentence as a real estimate.
fn estimate(seconds: usize) -> String {
    match seconds {
        0 => "Nothing to send yet.".to_owned(),
        seconds => format!("{} to send.", spoken(seconds)),
    }
}

/// The warning for characters in the message that Morse has no code for, or
/// none when every one of them can be sent.
fn left_out(dropped: &[char], tense: &str) -> Option<String> {
    let mut list: Vec<char> = Vec::new();
    for character in dropped {
        if !list.contains(character) {
            list.push(*character);
        }
    }
    (!list.is_empty()).then(|| {
        let list: String = list.into_iter().collect();
        format!("Morse has no code for {list}, so it {tense} left out.")
    })
}

/// Paints one letter at the size it will be seen, `room` pixels tall, as
/// grey bytes: the width, the height and the pixels.
///
/// With `code`, the dots and dashes that carry the letter are drawn under it
/// as dots and dashes, a sixth of the height given to them and the letter a
/// little smaller to make the room. They were the full stops and hyphens of
/// the code typed into the top bar, which read as an ellipsis.
fn paint(character: char, room: i32, code: Option<&str>) -> Option<(u32, u32, Vec<u8>)> {
    // Only what the code can send, and the space between words: a shape
    // with no code behind it could never reach the screen.
    if character != ' ' && !CODE.iter().any(|(sent, _)| *sent == character) {
        return None;
    }
    let room = u32::try_from(room).ok()?;
    let strip = if code.is_some() { room / 6 } else { 0 };
    let (width, letter_height, letter) =
        kobo_text::display_letter(character, (room - strip) * 2 / 3)?;
    let height = letter_height + strip;
    let mut pixels = letter;
    pixels.resize(
        pixels.len() + usize::try_from(width * strip).ok()?,
        tone::PAPER,
    );
    if let Some(code) = code {
        let width = i64::from(width);
        let unit = i64::from((strip / 3).max(2));
        let marks: Vec<i64> = code
            .chars()
            .map(|mark| if mark == '-' { unit * 3 } else { unit })
            .collect();
        let span =
            marks.iter().sum::<i64>() + unit * i64::try_from(marks.len().saturating_sub(1)).ok()?;
        let mut x = (width - span) / 2;
        let middle = i64::from(letter_height + strip / 2);
        let radius = unit / 2;
        for length in marks {
            // A dot is a round mark and a dash is a bar with round ends, three
            // dots long, as the code is printed in a manual.
            for px in x..x + length {
                for py in middle - radius..=middle + radius {
                    let edge = (px - x - radius).min(x + length - 1 - radius - px);
                    let inside = edge >= 0 || {
                        let centre = if px - x < radius {
                            x + radius
                        } else {
                            x + length - 1 - radius
                        };
                        (px - centre).pow(2) + (py - middle).pow(2) <= radius * radius
                    };
                    if inside && (0..width).contains(&px) && py >= 0 && py < i64::from(height) {
                        let at = usize::try_from(py * width + px).ok()?;
                        pixels[at] = tone::INK;
                    }
                }
            }
            x += length + unit;
        }
    }
    Some((width, height, pixels))
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    /// Typing the message.
    #[default]
    Writing,
    /// Sending it.
    Beacon,
    /// Every character the beacon can send, with the code that carries it.
    Reference,
}

// The four flags are independent facts -- the beacon running, the light
// carrying the message, learning mode, an open menu -- rather than states of
// one thing, so a state machine would say less than they do.
#[allow(clippy::struct_excessive_bools)]
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
    /// Whether the beacon draws each letter's code under it as it sends.
    ///
    /// Off by default: somebody who reads Morse is watching the light, and the
    /// code of a letter they already know is clutter on every panel. On, the
    /// dots and dashes sit under the letter, so a learner can see the light
    /// say what the screen spells.
    learn: bool,
    /// Whether the writing screen's overflow menu is open.
    menu_open: bool,
    /// The page of the letter reference the reader is on.
    reference_page: usize,
    tick: Option<TaskId>,
    /// A problem with the last thing the reader asked for, shown on the
    /// screen it was raised on and dropped once they move on from it.
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
            learn: false,
            menu_open: false,
            reference_page: 0,
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
const LEARN: &str = "learn";
const MORE: &str = "more";
const REFERENCE: &str = "reference";
const PREVIOUS_PAGE: &str = "previous-page";
const NEXT_PAGE: &str = "next-page";

impl Morse {
    /// The message as it went out, each letter followed by its code in the
    /// dots and dashes of the letter reference. Bare codes in full stops and
    /// hyphens read as punctuation, and with no letters there was nothing to
    /// tell where one code ended and the next began.
    fn spelled(&self) -> String {
        self.signals
            .iter()
            .map(|signal| {
                if signal.code.is_empty() {
                    "/".to_owned()
                } else {
                    format!("{} {}", signal.character, marks(signal.code))
                }
            })
            .collect::<Vec<_>>()
            .join("   ")
    }

    /// The message, what it will cost, and the Learning switch, over the
    /// keys. Where a small panel at a large size cannot hold them all, the
    /// switch's summary gives way first and then the line about the speed,
    /// so the estimate, the switch and every key stay on the panel.
    fn writing(&self, context: &Context) -> Screen {
        let metrics = context.metrics();
        let shapes = [(true, true), (false, true), (false, false)];
        shapes
            .into_iter()
            .map(|(summary, speed)| self.writing_with(summary, speed))
            .find(|screen| {
                !screen
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
            })
            .unwrap_or_else(|| self.writing_with(false, false))
    }

    fn writing_with(&self, summary: bool, speed: bool) -> Screen {
        let mut screen = ScreenBuilder::new("morse-writing")
            .top_bar("Morse")
            .top_bar_overflow(MORE, self.menu_open, [(REFERENCE, "Letter reference")]);
        // Characters with no code are named while they are in the box, and
        // the warning goes as soon as they are deleted.
        let (signals, dropped) = encode(self.keyboard.text());
        if let Some(trouble) = self
            .trouble
            .clone()
            .or_else(|| left_out(&dropped, "will be"))
        {
            screen = screen.banner(BannerLevel::Attention, trouble);
        }
        // The estimate belongs here, beside the Send key, rather than on the
        // screen that follows it. Send starts the beacon there and then, so
        // anywhere later is somewhere the reader is already committed, and a
        // beacon that quietly commits somebody to four minutes is the thing
        // the estimate exists to prevent. It is measured from what is in the
        // box rather than from the composed message, because at this point
        // nothing has been composed.
        let screen = screen
            .typed(&self.keyboard, "A message to send in light")
            .secondary(estimate(beats(&signals).len()));
        // The speed is the same for every message, so it is said once, in
        // the units the light actually keeps. A reader who knows a dash is
        // three seconds can follow the beacon letter by letter; one who does
        // not learns it here, before anything has flashed.
        let screen = if speed {
            screen.secondary("A dot is one second of light, a dash three.")
        } else {
            screen
        };
        // The learning switch lives beside the message it changes: chosen
        // here, it is the send that follows which narrates its letters. A
        // switch rather than a lone chip, which read as a tag.
        let said = match (summary, self.learn) {
            (false, _) => "",
            (true, true) => "Each letter shows its code underneath.",
            (true, false) => "The bar counts letters only.",
        };
        screen
            .toggle(LEARN, "Learning", said, self.learn)
            .keyboard(&self.keyboard, "Send")
            .build()
    }

    fn beacon(&self, context: &mut Context) -> Screen {
        // While it is sending, the progress goes in the top bar rather than
        // under the letter. It is the same fact either way, and a bar that
        // already exists costs the letter nothing, where a line of text under
        // it costs the letter that line on every panel.
        // In learning mode the code is drawn under the letter as dots and
        // dashes, so the bar only counts the letters off.
        let title = if self.running {
            format!(
                "Letter {} of {}",
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
        // whole of it. Resting, there is no letter -- nothing is going out --
        // and drawing one anyway meant painting the space glyph, which on the
        // panel is an empty frame the size of the screen sitting where a letter
        // had just been. So the picture belongs to the run, and the resting
        // screen carries the code and the estimate in its place.
        if let Some(signal) = self.showing.and_then(|index| self.signals.get(index)) {
            let room = context.metrics().tenth_mm(LETTER_SENDING_TENTHS_MM);
            let code = (self.learn && !signal.code.is_empty()).then_some(signal.code);
            if let Some(picture) = paint(signal.character, room, code)
                .and_then(|(width, height, grey)| context.put_picture(LETTER, width, height, grey))
            {
                let millimetres = u16::try_from(LETTER_SENDING_TENTHS_MM / 10).unwrap_or(u16::MAX);
                screen = screen.unframed_picture(picture, millimetres);
            }
        }
        if !self.running {
            screen = screen
                .text(self.spelled())
                .secondary(estimate(self.beats.len()));
        }
        // A bar cannot hold a switch, so the entry names what the tap does.
        // "Light on" stated the current value, which read as an order as
        // easily as a report. A third of a six-inch panel at the largest
        // size cannot hold "Turn light on" in two lines, so there, and only
        // there, the entry says the same thing in fewer letters.
        let bar = |short: bool| {
            let light = match (self.light, short) {
                (true, false) => "Turn light off",
                (false, false) => "Turn light on",
                (true, true) => "Go dark",
                (false, true) => "Light up",
            };
            if self.running {
                screen
                    .clone()
                    .action_bar([(STOP, "Stop"), (TOGGLE_LIGHT, light)])
            } else {
                // The message is already composed, so this key repeats it
                // rather than sending something new; the label says so, or a
                // repeat looks like a fresh send that happens to say the same
                // thing.
                screen.clone().action_bar([
                    (AGAIN, "Send again"),
                    (EDIT, "Edit"),
                    (TOGGLE_LIGHT, light),
                ])
            }
            .build()
        };
        let metrics = context.metrics();
        let full = bar(false);
        if full
            .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
            .has_errors()
        {
            bar(true)
        } else {
            full
        }
    }

    /// Every character the beacon can send, with the code that carries it.
    ///
    /// The chart is a paged list rather than one long screen, because a panel
    /// that ran out of room would drop the tail of the alphabet with nothing
    /// to say so. Paging is measured against this panel at this text size, so
    /// the largest setting simply turns more pages.
    fn reference(&self, context: &Context) -> Screen {
        // The whole chart on one page where it fits, three letters to a row,
        // as a printed card sets it: forty letters in seven pages of six
        // was a chart nobody could look anything up in.
        let metrics = context.metrics();
        let chart = Self::reference_chart();
        if !chart
            .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
            .has_errors()
        {
            return chart;
        }
        let entries: Vec<(String, String)> = CODE
            .iter()
            .map(|(letter, code)| (letter.to_string(), marks(code)))
            .collect();
        let measuring: Vec<(&str, &str, &str)> = entries
            .iter()
            .map(|(letter, code)| (letter.as_str(), code.as_str(), ""))
            .collect();
        let pages =
            context.paginate_rows_below_section(&measuring, true, Position::AtTheFoot, None);
        let page = self.reference_page.min(pages.len().saturating_sub(1));
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        ScreenBuilder::new("morse-reference")
            .top_bar("Letter reference")
            .section_with_value("The letters Morse can send", format!("{}", CODE.len()))
            .rows(visible.iter().map(|&index| {
                let (letter, code) = &entries[index];
                (
                    format!("letter-{letter}"),
                    letter.clone(),
                    code.clone(),
                    RowLead::Number(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                )
            }))
            .page_turns(PREVIOUS_PAGE, NEXT_PAGE)
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            )
            .build()
    }

    fn reference_chart() -> Screen {
        let rows = CODE
            .chunks(REFERENCE_COLUMNS)
            .map(|row| kobo_ui::TableRow {
                header: false,
                cells: row
                    .iter()
                    .flat_map(|(letter, code)| [letter.to_string(), marks(code)])
                    .chain(std::iter::repeat_n(
                        String::new(),
                        (REFERENCE_COLUMNS - row.len()) * 2,
                    ))
                    .collect(),
            })
            .collect();
        ScreenBuilder::new("morse-reference")
            .top_bar("Letter reference")
            .section_with_value("The letters Morse can send", format!("{}", CODE.len()))
            .table(rows, Vec::new())
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
            View::Writing => self.writing(context),
            View::Beacon => self.beacon(context),
            View::Reference => self.reference(context),
        };
        context.set_screen(screen);
    }

    /// A key on the writing screen's keyboard.
    fn on_key(&mut self, context: &mut Context, pressed: Pressed) {
        self.menu_open = false;
        match pressed {
            Pressed::Submitted => {
                let typed = self.keyboard.text().trim().to_owned();
                self.compose(&typed);
                if self.beats.is_empty() {
                    // Nothing would flash, so the reader stays with the box
                    // they need to type into rather than being sent to a
                    // beacon with nothing on it.
                    self.trouble = Some("There is nothing here to send.".to_owned());
                    self.show(context);
                    return;
                }
                self.view = View::Beacon;
                self.start(context);
            }
            Pressed::Edited => {
                // The message changed, so a warning about the old one no
                // longer describes what is in the box.
                self.trouble = None;
                self.show(context);
            }
            Pressed::Shifted => self.show(context),
        }
    }

    /// Takes the typed message and works out what it will look like in light.
    fn compose(&mut self, message: &str) {
        let (signals, dropped) = encode(message);
        self.signals = signals;
        self.beats = beats(&self.signals);
        self.at = 0;
        self.showing = None;
        self.trouble = left_out(&dropped, "was");
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
        // The panel goes first and the light second, so that a letter is
        // already up when its own flash begins. The two are queued in the order
        // they are asked for, and the paint is the slow one.
        let letter = letter_at(&self.beats, self.at);
        if self.showing != letter {
            self.showing = letter;
            self.show(context);
        }
        if self.light {
            context
                .device()
                .set_frontlight(if beat.lit { FULL } else { 0 });
        }
        self.tick = context.spawn(Task::Sleep { seconds: 1 });
        if self.tick.is_none() {
            // Nothing else can advance the beacon, so it stops here rather than
            // standing with the light held on whatever the last beat was.
            self.trouble = Some("The beacon could not keep time, so it stopped.".to_owned());
            self.finish(context);
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
            if let Some(pressed) = self.keyboard.press(action) {
                self.on_key(context, pressed);
                return;
            }
        }

        if action == action_id(MORE) {
            self.menu_open = true;
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            // An open menu takes the first Back, the way a scrim tap reads.
            if self.menu_open {
                self.menu_open = false;
                self.show(context);
                return;
            }
            match self.view {
                View::Writing => return,
                View::Beacon => {
                    if self.running {
                        self.finish(context);
                    }
                    self.trouble = None;
                    self.view = View::Writing;
                }
                View::Reference => self.view = View::Writing,
            }
            self.show(context);
            return;
        }

        // Any tap past this point lands on the screen itself, so a menu left
        // open is closed by it rather than held over whatever happens next.
        self.menu_open = false;

        if action == action_id(REFERENCE) {
            self.reference_page = 0;
            self.view = View::Reference;
            self.show(context);
            return;
        }

        if action == action_id(LEARN) {
            self.learn = !self.learn;
            self.show(context);
            return;
        }

        if action == action_id(PREVIOUS_PAGE) || action == action_id(NEXT_PAGE) {
            if self.view == View::Reference {
                if action == action_id(NEXT_PAGE) {
                    // Clamped against the real page count when the screen is
                    // drawn, so a tap at the end stays on the last page.
                    self.reference_page += 1;
                } else {
                    self.reference_page = self.reference_page.saturating_sub(1);
                }
                self.show(context);
            }
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
            // The beacon's warnings were about the message it sent; the
            // writing screen names any problem with the text as it stands.
            self.trouble = None;
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
        beats, encode, letter_at, marks, paint, Beat, Morse, View, AGAIN, CODE, EDIT, LEARN,
        MAX_MESSAGE, MORE, NEXT_PAGE, REFERENCE, STOP, TOGGLE_LIGHT,
    };
    use kobo_sdk::keyboard::Keyboard;
    use kobo_sdk::{action_id, ActionId, AppRunner, Command, DeviceRequest, TaskOutcome};
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
            assert!(
                paint(*character, 700, None).is_some(),
                "no shape for {character}"
            );
        }
        assert!(
            paint(' ', 700, None).is_some(),
            "the gap between words needs a blank"
        );
    }

    #[test]
    fn a_painted_letter_is_the_size_it_says_it_is_and_has_ink_in_it() {
        let (width, height, pixels) = paint('A', 700, None).expect("a letter");
        assert_eq!(pixels.len(), (width * height) as usize);
        assert!((698..=700).contains(&height), "{height} pixels tall");
        assert!(pixels.contains(&tone::INK));
        assert!(pixels.contains(&tone::PAPER));
        // Drawn from outlines, so its edges are shaded rather than stepped.
        assert!(pixels
            .iter()
            .any(|grey| *grey > tone::INK && *grey < tone::PAPER));
    }

    #[test]
    fn learning_draws_the_code_as_dots_and_dashes_under_the_letter() {
        let (width, height, plain) = paint('A', 700, None).expect("a letter");
        let (coded_width, coded_height, coded) = paint('A', 700, Some(".-")).expect("a letter");
        assert_eq!(
            height, coded_height,
            "the code took more than the letter gave up"
        );
        // The foot of the picture is empty without the code and carries one
        // short and one long run of ink with it.
        let row = usize::try_from(height - 700 / 12).unwrap();
        let runs = |pixels: &[u8], width: u32| {
            let line = &pixels[row * width as usize..(row + 1) * width as usize];
            let mut runs = Vec::new();
            let mut run = 0;
            for grey in line.iter().chain([&tone::PAPER]) {
                if *grey == tone::INK {
                    run += 1;
                } else if run > 0 {
                    runs.push(run);
                    run = 0;
                }
            }
            runs
        };
        assert!(runs(&plain, width).is_empty());
        let marks = runs(&coded, coded_width);
        assert_eq!(marks.len(), 2, "{marks:?}");
        assert!(
            marks[1] > marks[0] * 2,
            "the dash is no longer than the dot: {marks:?}"
        );
    }

    #[test]
    fn the_gap_between_words_is_painted_as_nothing_at_all() {
        let (_, _, pixels) = paint(' ', 700, None).expect("a blank");
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
            drawn.contains(&format!("S {}", marks("..."))),
            "the code is not shown: {drawn}"
        );
        assert!(
            drawn.contains("27 seconds"),
            "the estimate is not shown: {drawn}"
        );
    }

    /// Resting, nothing is going out, and the space glyph the beacon used to
    /// fall back on paints as an empty frame filling the panel. On the device
    /// that read as a bug -- a blank box sitting where a letter had been -- so
    /// the picture is drawn only while a letter is actually being sent.
    #[test]
    fn a_resting_beacon_draws_no_letter_at_all() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(AGAIN));
        let commands = runner.action(action_id(STOP));
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a screen");
        let drawn = format!("{screen:?}");
        assert!(
            !drawn.contains("Picture"),
            "the resting beacon still draws a letter: {drawn}"
        );
    }

    /// The point of the panel is that the letter can be read while the light is
    /// flashing it. Every lit second must therefore find its own letter on the
    /// screen and not the one before it.
    #[test]
    fn the_letter_on_the_panel_is_the_one_the_light_is_flashing() {
        for message in ["sos", "hello world", "e t e", "73"] {
            let run = beats(&encode(message).0);
            for (at, beat) in run.iter().enumerate() {
                if beat.lit {
                    assert_eq!(
                        letter_at(&run, at),
                        Some(beat.signal),
                        "{message} at second {at}"
                    );
                }
            }
        }
    }

    /// A silence is when the panel gets ahead of the light, which is what buys
    /// the e-ink time to settle before the flash it belongs to.
    #[test]
    fn a_silence_shows_the_letter_that_is_coming_rather_than_the_one_that_went() {
        // I is two dots, then three dark beats, then E: "#.#...#".
        let run = beats(&encode("ie").0);
        assert_eq!(letter_at(&run, 1), Some(0), "the gap inside I is still I");
        for at in 3..6 {
            assert_eq!(letter_at(&run, at), Some(1), "the gap before E is E");
        }
    }

    /// The gap between words is a silence rather than a symbol and has no shape
    /// of its own. Drawn anyway, its blank filled the panel with an empty frame
    /// the size of a letter, which is the bug the resting screen had.
    #[test]
    fn the_gap_between_words_never_reaches_the_panel() {
        let (signals, _) = encode("e e");
        let space = signals
            .iter()
            .position(|signal| signal.code.is_empty())
            .expect("a word gap");
        let run = beats(&signals);
        for at in 0..run.len() {
            assert_ne!(letter_at(&run, at), Some(space), "at second {at}");
        }
    }

    /// Names the letter in a picture by painting every candidate and comparing,
    /// so a test reads the panel the way somebody across the room does rather
    /// than trusting the application's own account of what it drew.
    fn letter_in(grey: &[u8], width: u32, height: u32) -> Option<char> {
        let room = CLARA_BW_METRICS.tenth_mm(super::LETTER_SENDING_TENTHS_MM);
        CODE.iter()
            .map(|(character, _)| *character)
            .chain([' '])
            .find(|character| {
                paint(*character, room, None).is_some_and(|(drawn_width, drawn_height, pixels)| {
                    drawn_width == width && drawn_height == height && pixels == grey
                })
            })
    }

    /// Walks a message a second at a time, reporting what the light is doing
    /// and which letter the panel is carrying at each one.
    ///
    /// The picture is only sent when the letter changes, so what is on the
    /// panel at any second is the last picture sent -- which is exactly the
    /// thing that can drift out of step with the light.
    fn watched(message: &str) -> Vec<(bool, Option<char>)> {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.app_mut().compose(message);
        let run = beats(&encode(message).0);
        let mut commands = runner.action(action_id(AGAIN));
        let mut panel = None;
        let mut seen = Vec::new();
        for beat in &run {
            for command in &commands {
                if let Command::PutPicture {
                    width,
                    height,
                    pixels,
                    ..
                } = command
                {
                    panel = letter_in(pixels, *width, *height);
                }
            }
            seen.push((beat.lit, panel));
            let Some(task) = commands.iter().rev().find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            }) else {
                break;
            };
            commands = runner.task_outcome(task, TaskOutcome::Completed(Vec::new()));
        }
        seen
    }

    /// The reader across the room is reading the panel and the light together,
    /// so every second the light is lit, the letter on the panel has to be the
    /// letter that light is spelling.
    #[test]
    fn no_letter_is_ever_flashed_while_another_is_on_the_panel() {
        for message in ["e t", "sos", "hi there"] {
            let signals = encode(message).0;
            let run = beats(&signals);
            for (second, ((lit, panel), beat)) in watched(message).iter().zip(&run).enumerate() {
                if *lit {
                    assert_eq!(
                        *panel,
                        Some(signals[beat.signal].character),
                        "{message} at second {second}"
                    );
                }
            }
        }
    }

    /// Send starts the beacon there and then, so the screen the Send key is on
    /// is the last one where the estimate can still change anybody's mind.
    #[test]
    fn the_estimate_is_on_the_screen_the_send_key_is_on() {
        let mut runner = AppRunner::new(Morse::default());
        let commands = runner.start();
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a writing screen");
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("27 seconds"),
            "the estimate is not on the writing screen: {drawn}"
        );
        let issues = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(
            !issues.has_errors(),
            "the writing screen does not fit: {issues:?}"
        );
    }

    /// The speed is a fact about the beacon rather than about the message, so
    /// it is stated where the message is weighed: beside the estimate, on the
    /// screen the Send key is on.
    #[test]
    fn the_speed_is_on_the_screen_the_send_key_is_on() {
        let mut runner = AppRunner::new(Morse::default());
        let commands = runner.start();
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a writing screen");
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("one second of light"),
            "the speed is not on the writing screen: {drawn}"
        );
    }

    /// The resting key sends the same message out again, so it says that is
    /// what it does. Labeled as a fresh send, a repeat would read as a
    /// mistake: the same letters going out twice with nothing to say why.
    #[test]
    fn the_resting_key_says_it_repeats_the_message() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(AGAIN));
        let commands = runner.action(action_id(STOP));
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a resting screen");
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("Send again"),
            "the resting key does not say it repeats: {drawn}"
        );
    }

    /// The chart is the whole alphabet the beacon can send, paged so a panel
    /// never drops the tail of it. Walked to the end, every code is there and
    /// every page fits.
    #[test]
    fn the_reference_lists_every_letter_the_beacon_can_send() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(MORE));
        let mut drawn = String::new();
        let mut commands = runner.action(action_id(REFERENCE));
        // Paged to the end: a page turn past the last page redraws it
        // unchanged, the runner elides a screen it is already showing, and
        // the empty answer says the walk is done. The cap only guards the
        // loop against a chart that never ends.
        for _ in 0..40 {
            let Some(screen) = commands.iter().rev().find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            }) else {
                break;
            };
            let issues = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
            assert!(
                !issues.has_errors(),
                "a page of the reference does not fit: {issues:?}"
            );
            drawn.push_str(&format!("{screen:?}"));
            commands = runner.action(action_id(NEXT_PAGE));
        }
        // Printed with dots and dashes, not the full stops and hyphens the
        // codes are kept in.
        for code in [".-", "-.--", "-----", ".-.-.-", "-..-."] {
            assert!(drawn.contains(&marks(code)), "the code {code} is missing");
        }
        assert!(
            !drawn.contains("\".-\""),
            "a code is printed as punctuation"
        );
    }

    /// Back from the chart returns to the message, with the typing untouched.
    #[test]
    fn the_reference_leads_back_to_the_message() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(MORE));
        runner.action(action_id(REFERENCE));
        let commands = runner.action(ActionId::BACK);
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a writing screen");
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("one second of light"),
            "back from the chart did not return to the message: {drawn}"
        );
    }

    /// Learning mode draws the code under the letter, so the bar only counts
    /// the letters off, the same with it as without.
    #[test]
    fn learning_mode_leaves_the_code_to_the_picture_and_the_count_to_the_bar() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(LEARN));
        let commands = runner.action(action_id(AGAIN));
        let screen = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("a beacon screen");
        let drawn = format!("{screen:?}");
        assert!(
            drawn.contains("Letter 1 of 3") && !drawn.contains("S ..."),
            "the bar does not count the letters off on their own: {drawn}"
        );
    }

    /// Without learning mode the bar counts letters off and says nothing
    /// more: somebody reading the light fluently has no use for the code.
    #[test]
    fn the_bar_stays_a_plain_count_until_asked() {
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
        let drawn = format!("{screen:?}");
        assert!(
            !drawn.contains("S ..."),
            "the bar names codes nobody asked for: {drawn}"
        );
        assert!(
            drawn.contains("1 of 3"),
            "the count is missing from the bar: {drawn}"
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

    /// The writing screens Morse draws at this size, with Learning off and
    /// on, as the runtime would be handed them.
    fn writing_at(metrics: kobo_sdk::DisplayMetrics) -> Vec<kobo_sdk::Screen> {
        let mut runner = AppRunner::with_metrics(Morse::default(), metrics);
        let first = runner.start();
        let toggled = runner.action(action_id(LEARN));
        [first, toggled]
            .iter()
            .map(|commands| {
                commands
                    .iter()
                    .rev()
                    .find_map(|command| match command {
                        Command::SetScreen(screen) => Some(screen.clone()),
                        _ => None,
                    })
                    .expect("a writing screen")
            })
            .collect()
    }

    /// Learning is a switch, not a lone chip that read as a tag, and the
    /// writing screen keeps it, the estimate and every key on the panel at
    /// every text size. Morse never asks for landscape, so it is only ever
    /// drawn upright.
    #[test]
    fn learning_is_a_switch_and_the_writing_screen_fits_every_size() {
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (758, 1024, 212), (1264, 1680, 300)]
        {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                for (screen, on) in writing_at(metrics).into_iter().zip([false, true]) {
                    let switch = screen.nodes.iter().find_map(|node| match node {
                        kobo_ui::Node::Toggle { label, on, .. } if label == "Learning" => Some(*on),
                        _ => None,
                    });
                    assert_eq!(switch, Some(on), "{metrics:?}");
                    assert!(!format!("{screen:?}").contains("Chips"), "{metrics:?}");
                    let chrome = Chrome::measuring(true);
                    let issues = screen.diagnostics(&metrics, &chrome);
                    assert!(!issues.has_errors(), "{metrics:?}: {issues:?}");
                    let layout = screen.layout_with(&metrics, &chrome);
                    for name in [LEARN, kobo_ui::keyboard::ENTER, "kb.backspace"] {
                        assert!(
                            layout.rect_of_action(action_id(name)).is_some(),
                            "{metrics:?}: {name} is off the panel"
                        );
                    }
                    assert!(
                        format!("{screen:?}").contains("27 seconds"),
                        "{metrics:?}: the estimate is gone"
                    );
                    // The summary and the speed give way only where the panel
                    // forces it; a Clara at the default size has both.
                    if pixels_per_inch == 300 && text_scale == kobo_ui::TextScale::Default {
                        let drawn = format!("{screen:?}");
                        assert!(drawn.contains("one second of light"), "{metrics:?}");
                        let summary = if on {
                            "Each letter shows its code underneath."
                        } else {
                            "The bar counts letters only."
                        };
                        assert!(drawn.contains(summary), "{metrics:?}");
                    }
                }
            }
        }
    }

    /// A bar cannot hold a switch, so the light's entry names what the tap
    /// does, and both words fit beside Stop, and beside Send again and Edit,
    /// at every text size.
    #[test]
    fn the_light_entry_names_what_the_tap_does_and_fits_every_bar() {
        let last_screen = |commands: Vec<Command>| {
            commands
                .into_iter()
                .rev()
                .find_map(|command| match command {
                    Command::SetScreen(screen) => Some(screen),
                    _ => None,
                })
                .expect("a beacon screen")
        };
        for (width, height, pixels_per_inch) in [(1072, 1448, 300), (758, 1024, 212)] {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let mut runner = AppRunner::with_metrics(Morse::default(), metrics);
                runner.start();
                let running = last_screen(runner.action(action_id(AGAIN)));
                let dark = last_screen(runner.action(action_id(TOGGLE_LIGHT)));
                let resting = last_screen(runner.action(action_id(STOP)));
                for (screen, (label, short), others) in [
                    (running, ("Turn light off", "Go dark"), &[STOP][..]),
                    (dark, ("Turn light on", "Light up"), &[STOP][..]),
                    (resting, ("Turn light on", "Light up"), &[AGAIN, "edit"][..]),
                ] {
                    let bar = screen.nav_bar.as_ref().expect("an action bar");
                    let entry = bar
                        .destinations
                        .iter()
                        .find(|entry| entry.action == action_id(TOGGLE_LIGHT))
                        .expect("the light entry");
                    // The shorter words are only for a panel smaller than
                    // a Clara, where the longer ones cannot fit the bar.
                    if pixels_per_inch == 300 {
                        assert_eq!(entry.label, label, "{metrics:?}");
                    } else {
                        assert!(entry.label == label || entry.label == short, "{metrics:?}");
                    }
                    let chrome = Chrome::measuring(true);
                    let issues = screen.diagnostics(&metrics, &chrome);
                    assert!(!issues.has_errors(), "{metrics:?} {label}: {issues:?}");
                    let layout = screen.layout_with(&metrics, &chrome);
                    for name in others.iter().copied().chain([TOGGLE_LIGHT]) {
                        assert!(
                            layout.rect_of_action(action_id(name)).is_some(),
                            "{metrics:?}: {name} is off the panel"
                        );
                    }
                }
            }
        }
    }

    /// The last screen a batch of commands set, drawn as text to search.
    fn last_screen(commands: &[Command]) -> String {
        commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(format!("{screen:?}")),
                _ => None,
            })
            .expect("a screen")
    }

    /// The Learning switch's summary says what Learning does now: the code
    /// is drawn under the letter, not named in the bar.
    #[test]
    fn the_learning_summary_says_the_code_is_drawn_under_each_letter() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        let drawn = last_screen(&runner.action(action_id(LEARN)));
        assert!(
            drawn.contains("Each letter shows its code underneath.")
                && !drawn.contains("The bar names"),
            "{drawn}"
        );
    }

    /// An empty box says there is nothing to send once, not "nothing to send
    /// to send".
    #[test]
    fn an_empty_message_says_there_is_nothing_to_send_once() {
        let mut runner = AppRunner::new(Morse::default());
        runner.app_mut().keyboard = Keyboard::with_text("S");
        runner.start();
        let drawn = last_screen(&runner.action(ActionId::from_name("kb.backspace")));
        assert!(drawn.contains("Nothing to send yet."), "{drawn}");
        assert!(!drawn.contains("to send to send"), "{drawn}");
    }

    /// Send with an empty box stays on the writing screen with a warning, and
    /// the warning goes as soon as a letter is typed.
    #[test]
    fn sending_an_empty_message_stays_on_the_message_and_typing_clears_the_warning() {
        let mut runner = AppRunner::new(Morse::default());
        runner.app_mut().keyboard = Keyboard::with_text("");
        runner.start();
        let drawn = last_screen(&runner.action(ActionId::from_name(kobo_ui::keyboard::ENTER)));
        assert_eq!(runner.app_mut().view, View::Writing);
        assert!(!runner.app_mut().running);
        assert!(drawn.contains("There is nothing here to send."), "{drawn}");
        runner.app_mut().keyboard = Keyboard::with_text("TT");
        let drawn = last_screen(&runner.action(ActionId::from_name("kb.backspace")));
        assert!(!drawn.contains("There is nothing here to send."), "{drawn}");
    }

    /// A character with no code is named while it is in the box, and the
    /// warning goes when it is deleted, including after a send and Edit.
    #[test]
    fn the_warning_about_a_character_with_no_code_goes_when_the_character_does() {
        let mut runner = AppRunner::new(Morse::default());
        runner.app_mut().keyboard = Keyboard::with_text("T!!");
        runner.start();
        let drawn = last_screen(&runner.action(ActionId::from_name("kb.backspace")));
        assert!(
            drawn.contains("Morse has no code for !, so it will be left out."),
            "{drawn}"
        );
        let drawn = last_screen(&runner.action(ActionId::from_name(kobo_ui::keyboard::ENTER)));
        assert!(drawn.contains("so it was left out."), "{drawn}");
        runner.action(action_id(STOP));
        runner.action(action_id(EDIT));
        let drawn = last_screen(&runner.action(ActionId::from_name("kb.backspace")));
        assert!(!drawn.contains("Morse has no code"), "{drawn}");
    }

    /// Resting after a send, the message is spelled in the same dots and
    /// dashes as the letter reference, each code after its letter.
    #[test]
    fn the_resting_screen_spells_the_message_in_dots_and_dashes() {
        let mut runner = AppRunner::new(Morse::default());
        runner.start();
        runner.action(action_id(AGAIN));
        let drawn = last_screen(&runner.action(action_id(STOP)));
        let spelled = format!(
            "S {}   O {}   S {}",
            marks("..."),
            marks("---"),
            marks("...")
        );
        assert!(drawn.contains(&spelled), "{drawn}");
        assert!(!drawn.contains("... --- ..."), "{drawn}");
    }
}
