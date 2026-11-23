//! What changed, said once after an update.
//!
//! A platform update is staged, swapped in the next time Cobalt opens, and
//! until now nothing on the panel said so: the owner found a new row in
//! Settings by accident or not at all. The launcher is the first thing drawn
//! after the swap, so it carries a short sheet over the grid naming what is
//! new, and never shows it again once it has been put away.
//!
//! The notes are a text file compiled into this binary, written by hand when
//! the version is bumped. They are shown only under the version they were
//! written for. A release cut without new notes shows nothing, because last
//! release's list under this release's number would be a lie.

use kobo_sdk::{
    Chrome, DisplayMetrics, Node, ParagraphPresentation, RichTextSpan, Screen, ScreenBuilder,
    Space, TextPresentation,
};

/// The action the sheet's one button carries, and the dialogue's way out.
pub const DONE: &str = "whats-new-done";

/// Where the last version whose notes were put away is kept: the version as
/// it is written, and nothing else.
pub const SEEN: &str = "whats-new-seen";

/// The notes for the release this tree is building towards.
const COMMITTED: &str = include_str!("../whats-new.txt");

/// The fewest and the most changes a file may list. One change is a sentence
/// and wants no sheet; six is a changelog, and nobody reads one on the way
/// to opening an app.
const FEWEST: usize = 2;
const MOST: usize = 5;

/// The most words a lead may run to. It is set bold, and a bold run longer
/// than this stops being a label and becomes a shouted half sentence.
const LEAD_WORDS: usize = 4;

/// The most characters a change may run to, lead and sentence together:
/// three lines of the sheet on a six inch panel at the default text size.
///
/// A ceiling on one change, not a promise about the file. Whether the whole
/// list fits is asked of the renderer, panel by panel, by the test that lays
/// the committed file out.
const CHANGE_CHARACTERS: usize = 110;

/// A space a line may not end at.
const BOUND: char = '\u{a0}';

/// One change: the line as the sheet sets it, and where its lead ends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Change {
    /// "Light: set the front light's brightness from Settings.", with the
    /// spaces [`bound`] ties.
    pub text: String,
    /// How many bytes of `text` are the lead, its colon included.
    pub lead: usize,
}

impl Change {
    /// The lead without its colon, as a route or a test names it.
    #[cfg(test)]
    pub fn lead(&self) -> &str {
        &self.text[..self.lead - 1]
    }

    /// The change as it was written, every space an ordinary one.
    #[cfg(test)]
    pub fn written(&self) -> String {
        self.text.replace(BOUND, " ")
    }
}

/// `sentence` with the two places a line must not break tied together.
///
/// A number stays with the word it counts: "bigger on 7" over "inch and 10
/// inch panels" was the first thing the sheet did on a six inch panel. And
/// the last two words stay together, so a change never ends on a line of
/// one word. Tied here, not in the file, because nobody writing a release
/// note should have to type a space they cannot see.
fn bound(sentence: &str) -> String {
    let words: Vec<&str> = sentence.split_whitespace().collect();
    let mut set = String::with_capacity(sentence.len() + 4);
    for (index, word) in words.iter().enumerate() {
        set.push_str(word);
        if index + 1 == words.len() {
            break;
        }
        let counts = word.bytes().all(|byte| byte.is_ascii_digit());
        let last_pair = index + 2 == words.len();
        set.push(if counts || last_pair { BOUND } else { ' ' });
    }
    set
}

/// A release's notes: the version they describe and what changed in it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Notes {
    pub version: String,
    pub changes: Vec<Change>,
}

/// Why a notes file was refused, with the line to look at.
///
/// Every rule is checked here and nowhere else, so the test over the
/// committed file and the launcher on a reader cannot disagree about what a
/// well-formed file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// Nothing but comments and blank lines.
    Empty,
    /// The first line is not a version: three numbers with dots between.
    Version(usize),
    /// A change has no colon, so nothing says where its lead ends.
    Colon(usize),
    /// A lead is missing or runs past [`LEAD_WORDS`] words.
    Lead(usize),
    /// Nothing follows the lead, or what follows does not end in a full stop.
    Sentence(usize),
    /// A change runs past [`CHANGE_CHARACTERS`] characters.
    Length(usize),
    /// The file lists this many changes, outside [`FEWEST`] to [`MOST`].
    Count(usize),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(formatter, "the file names no version and lists no changes"),
            Self::Version(line) => write!(
                formatter,
                "line {line}: the first line must be the version alone, such as 0.3.41"
            ),
            Self::Colon(line) => write!(
                formatter,
                "line {line}: a change is written `Lead words: one sentence.` and this has no colon"
            ),
            Self::Lead(line) => write!(
                formatter,
                "line {line}: the lead before the colon must be one to {LEAD_WORDS} words"
            ),
            Self::Sentence(line) => write!(
                formatter,
                "line {line}: the lead must be followed by a sentence that ends in a full stop"
            ),
            Self::Length(line) => write!(
                formatter,
                "line {line}: a change may run to {CHANGE_CHARACTERS} characters and this is longer"
            ),
            Self::Count(count) => write!(
                formatter,
                "the file lists {count} changes and must list {FEWEST} to {MOST}"
            ),
        }
    }
}

/// The three numbers of a version written `0.3.41`, or nothing for anything
/// else: a `v` in front, a fourth number, a suffix.
pub fn numbers(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.split('.');
    let mut numbers = [0; 3];
    for number in &mut numbers {
        let part = parts.next()?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

/// Reads a notes file.
///
/// The first line is the version, each line after it one change, and blank
/// lines and lines that start with `#` are skipped. Spaces round the lead
/// and the sentence are not part of either, so `Light :  set it.` and
/// `Light: set it.` are the same change.
///
/// # Errors
///
/// The first rule the file breaks, and the line that breaks it.
pub fn parse(source: &str) -> Result<Notes, Fault> {
    let mut lines = source
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'));
    let (number, version) = lines.next().ok_or(Fault::Empty)?;
    if numbers(version).is_none() {
        return Err(Fault::Version(number));
    }
    let mut changes = Vec::new();
    for (number, line) in lines {
        let (lead, sentence) = line.split_once(':').ok_or(Fault::Colon(number))?;
        let (lead, sentence) = (lead.trim(), sentence.trim());
        if lead.is_empty() || lead.split_whitespace().count() > LEAD_WORDS {
            return Err(Fault::Lead(number));
        }
        if sentence.len() < 2 || !sentence.ends_with('.') {
            return Err(Fault::Sentence(number));
        }
        let lead = lead.split_whitespace().collect::<Vec<_>>().join(" ");
        let text = format!("{lead}: {}", bound(sentence));
        if text.chars().count() > CHANGE_CHARACTERS {
            return Err(Fault::Length(number));
        }
        changes.push(Change {
            lead: lead.len() + 1,
            text,
        });
    }
    if !(FEWEST..=MOST).contains(&changes.len()) {
        return Err(Fault::Count(changes.len()));
    }
    Ok(Notes {
        version: version.to_owned(),
        changes,
    })
}

/// What this installation could tell its owner: the notes it was built with
/// and the version it is.
///
/// Both are inputs, so everything that decides whether a sheet is owed can
/// be asked about any pair of them. On a reader they are the committed file
/// and the package version.
#[derive(Clone, Debug)]
pub struct Release {
    notes: Option<Notes>,
    running: String,
}

impl Default for Release {
    /// The committed notes under the version this binary was built as. A
    /// file that does not parse is no notes; its test is what says why.
    fn default() -> Self {
        Self::new(parse(COMMITTED).ok(), env!("CARGO_PKG_VERSION"))
    }
}

impl Release {
    pub fn new(notes: Option<Notes>, running: impl Into<String>) -> Self {
        Self {
            notes,
            running: running.into(),
        }
    }

    /// The same notes under another running version.
    ///
    /// Between releases the tree is one version behind its notes, so the
    /// sheet a simulator is asked for would never be drawn. A debug build
    /// lets `KOBO_LAUNCHER_NOTES_VERSION` stand in for the version, and this
    /// is the whole of what that does.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn running_as(self, running: Option<String>) -> Self {
        Self {
            running: running.unwrap_or(self.running),
            ..self
        }
    }

    /// The notes written for the version that is running, if any were.
    pub fn notes(&self) -> Option<&Notes> {
        self.notes
            .as_ref()
            .filter(|notes| notes.version == self.running)
    }

    /// The notes the owner has yet to put away, given what the store holds
    /// under [`SEEN`]: nothing on a reader that has never shown a sheet.
    pub fn unseen(&self, seen: Option<&[u8]>) -> Option<&Notes> {
        self.notes()
            .filter(|notes| seen != Some(notes.version.as_bytes()))
    }
}

/// How a change is set.
///
/// Its lines after the first stand in from the margin, so the bold leads run
/// down the edge under the title by themselves and the eye finds where each
/// change starts. A dash or a dot in front of each did the same job with a
/// column of marks, and pushed the leads off the title's edge to make room
/// for them. The space after is what makes four paragraphs a list of four
/// things: without it a change of two lines and the one under it read as
/// three lines of one. A quarter of a line, which at the largest text size
/// is still a little less than the space the sheet leaves under its title.
const CHANGE: ParagraphPresentation = ParagraphPresentation {
    start_indent_em: 100,
    first_line_indent_em: -100,
    margin_after_em: 25,
    ..ParagraphPresentation::PLAIN
};

/// The sheet over `screen`, listing the first `shown` changes.
fn sheet(screen: ScreenBuilder, notes: &Notes, shown: usize) -> ScreenBuilder {
    // The version stays with the name it belongs to if the title wraps.
    let title = format!("What's new in Cobalt{BOUND}{}", notes.version);
    screen.dialog(title, DONE, |sheet| {
        // A dialogue sets its first paragraph a millimetre under its title,
        // which is right for a question and its one sentence. Here it left
        // the title closer to the first change than the changes are to each
        // other, so the title read as the first item of its own list.
        let mut sheet = sheet.spacer(Space::Tight);
        for change in notes.changes.iter().take(shown) {
            let lead = RichTextSpan {
                start: 0,
                end: change.lead,
                presentation: TextPresentation {
                    strong: true,
                    ..TextPresentation::default()
                },
            };
            sheet = sheet.rich_text(change.text.clone(), vec![lead], CHANGE);
        }
        // Outlined, as the launcher's other dialogue sets its answers. A
        // filled button is the loudest mark the panel makes, and putting
        // away a list of news is not what this screen is for.
        sheet.button(DONE, "Done")
    })
}

/// `screen` with the sheet over it, holding as many changes as there is
/// room for between the bars at this text size, or nothing when there is
/// not room for one.
///
/// A dialogue may be as tall as the panel, and one that tall is centred on
/// it: at the largest sizes the sheet lay across the top bar with the
/// launcher's name and the bell showing round its corners. Past the height
/// of the panel the renderer keeps the button and finds the room for it in
/// the paragraphs above, taking lines from the first of them first, so the
/// sheet opened on its second change. Its diagnostics do not look inside a
/// dialogue, so nothing reported either.
///
/// So the sheet is laid out here, as the runtime will lay it out, and
/// changes are given up from the end, whole, until what is left stands
/// clear of both bars and is drawn to its last word. A title and a button
/// with nothing between them say nothing, so when not even the first change
/// fits there is no sheet, and the notes stay owed until one can be shown.
pub fn over(screen: &ScreenBuilder, notes: &Notes, metrics: &DisplayMetrics) -> Option<Screen> {
    (1..=notes.changes.len())
        .rev()
        .map(|shown| sheet(screen.clone(), notes, shown).build())
        .find(|fitted| fits(fitted, metrics))
}

/// Whether the sheet on `screen` stands between the bars with a margin to
/// spare at each end, and the panel draws its title, every change on it and
/// its button whole.
///
/// Words are counted in letters, not lines: a line the renderer set is
/// trimmed of the space it broke at, and a run of bold is a piece of its
/// own, so the letters are the one thing a paragraph and its drawing can be
/// compared by.
fn fits(screen: &Screen, metrics: &DisplayMetrics) -> bool {
    let Some(sheet) = &screen.overlay else {
        return false;
    };
    let layout = screen.layout_with(metrics, &Chrome::measuring(false));
    let letters = |text: &str| {
        text.chars()
            .filter(|letter| !letter.is_whitespace())
            .count()
    };
    let drawn = |id| -> usize {
        layout
            .nodes
            .iter()
            .filter(|placed| placed.id == id)
            .flat_map(|placed| &placed.text_lines)
            .map(|run| letters(run))
            .sum()
    };
    // Everything laid out under the sheet's own identifier is its frame and
    // its title, but for the scrim, which covers the panel and draws nothing.
    let frame = layout
        .nodes
        .iter()
        .filter(|placed| placed.id == sheet.id && placed.rect.height < metrics.height)
        .map(|placed| (placed.rect.y, placed.rect.y + placed.rect.height))
        .reduce(|(top, foot), (y, end)| (top.min(y), foot.max(end)));
    let Some((top, foot)) = frame else {
        return false;
    };
    let clearance = metrics.screen_margin();
    let between = layout.content;
    top >= between.y + clearance
        && foot <= between.y + between.height - clearance
        && drawn(sheet.id) == letters(&sheet.title)
        && sheet.nodes.iter().all(|node| match node {
            Node::RichText { id, text, .. } => drawn(*id) == letters(text),
            Node::Button { id, .. } => layout.nodes.iter().any(|placed| placed.id == *id),
            _ => true,
        })
}

#[cfg(test)]
mod tests {
    use super::{numbers, parse, Fault, Release, CHANGE_CHARACTERS, COMMITTED, LEAD_WORDS, SEEN};

    const SAMPLE: &str = "\
# A comment, and a blank line after it.

0.3.41
Light: set the front light's brightness from Settings.
Larger readers: everything is drawn a little bigger.
";

    /// Every harness that starts a launcher on fresh state marks the notes
    /// as read through one script. A key renamed here and not there would
    /// bring the sheet back over every harness on the next release day, and
    /// nothing before that day would say so.
    #[test]
    fn the_script_harnesses_mark_the_notes_read_with_writes_the_key_the_launcher_reads() {
        let script = include_str!("../../../scripts/sim-notes-read.py");
        assert!(
            script.contains(&format!("SEEN = '{SEEN}'")),
            "scripts/sim-notes-read.py does not write the launcher's {SEEN:?} key"
        );
    }

    #[test]
    fn a_file_is_a_version_and_then_one_change_to_a_line() {
        let notes = parse(SAMPLE).expect("the sample is well formed");
        assert_eq!(notes.version, "0.3.41");
        assert_eq!(notes.changes.len(), 2);
        assert_eq!(
            notes.changes[0].written(),
            "Light: set the front light's brightness from Settings."
        );
        assert_eq!(notes.changes[0].lead(), "Light");
        assert_eq!(notes.changes[1].lead(), "Larger readers");
    }

    #[test]
    fn the_lead_ends_at_the_first_colon_and_carries_it() {
        let notes = parse("1.0.0\nClock: it reads 12:30 at half past.\nLight: it is on.\n")
            .expect("a colon inside the sentence is part of the sentence");
        let change = &notes.changes[0];
        assert_eq!(&change.text[..change.lead], "Clock:");
        assert_eq!(
            change.written()[change.lead..],
            *" it reads 12:30 at half past."
        );
    }

    #[test]
    fn stray_spaces_and_windows_line_endings_change_nothing() {
        let tidy = parse("0.3.41\nLight: set it.\nSound: hear it.\n");
        let untidy = parse("  0.3.41  \r\n\r\n  Light :   set it.  \r\nSound:set it.\r\n");
        assert_eq!(
            tidy.expect("tidy").changes[0],
            untidy.expect("untidy").changes[0]
        );
    }

    #[test]
    fn a_number_stays_with_its_word_and_the_last_two_words_stay_together() {
        let notes =
            parse("1.0.0\nLarger readers: bigger on 7 inch and 10 inch panels.\nLight: on.\n")
                .expect("well formed");
        assert_eq!(
            notes.changes[0].text,
            "Larger readers: bigger on 7\u{a0}inch and 10\u{a0}inch\u{a0}panels."
        );
        // One word has nothing to be tied to, and a lead is left alone: it
        // starts its line, so it only ever breaks when it is wider than one.
        assert_eq!(notes.changes[1].text, "Light: on.");
        let wide = parse("1.0.0\nA lead of 4: it is on.\nLight: on.\n").expect("well formed");
        assert_eq!(wide.changes[0].lead(), "A lead of 4");
    }

    #[test]
    fn a_file_that_breaks_a_rule_is_refused_with_the_line_that_breaks_it() {
        let long_lead = "0.3.41\nLight: set it.\nA lead of five whole words: is too long.\n";
        let long_change = format!(
            "0.3.41\nLight: set it.\nLong: {}.\n",
            "a".repeat(CHANGE_CHARACTERS)
        );
        for (source, fault) in [
            ("", Fault::Empty),
            ("# Only a comment.\n\n", Fault::Empty),
            ("Light: set it.\nSound: hear it.\n", Fault::Version(1)),
            (
                "v0.3.41\nLight: set it.\nSound: hear it.\n",
                Fault::Version(1),
            ),
            ("0.3\nLight: set it.\nSound: hear it.\n", Fault::Version(1)),
            (
                "0.3.41-beta\nLight: set it.\nSound: hear it.\n",
                Fault::Version(1),
            ),
            ("0.3.41\nLight: set it.\nNo lead here.\n", Fault::Colon(3)),
            (
                "0.3.41\n# One.\n: set it.\nSound: hear it.\n",
                Fault::Lead(3),
            ),
            (long_lead, Fault::Lead(3)),
            ("0.3.41\nLight:\nSound: hear it.\n", Fault::Sentence(2)),
            (
                "0.3.41\nLight: set it\nSound: hear it.\n",
                Fault::Sentence(2),
            ),
            (
                "0.3.41\nLight: set it!\nSound: hear it.\n",
                Fault::Sentence(2),
            ),
            (long_change.as_str(), Fault::Length(3)),
            ("0.3.41\n", Fault::Count(0)),
            ("0.3.41\nLight: set it.\n", Fault::Count(1)),
            (
                "1.0.0\nA: a.\nB: b.\nC: c.\nD: d.\nE: e.\nF: f.\n",
                Fault::Count(6),
            ),
        ] {
            assert_eq!(parse(source), Err(fault), "{source:?}");
            assert!(!fault.to_string().is_empty());
        }
    }

    #[test]
    fn a_version_is_three_numbers_and_nothing_else() {
        assert_eq!(numbers("0.3.41"), Some([0, 3, 41]));
        for wrong in [
            "", "0.3", "0.3.41.1", "v0.3.41", "0.3.x", "0..41", "0.3.41 ",
        ] {
            assert_eq!(numbers(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn notes_are_owed_once_and_only_under_the_version_they_describe() {
        let notes = parse(SAMPLE).ok();
        let updated = Release::new(notes.clone(), "0.3.41");
        // Never shown on this reader, or shown for an earlier release.
        assert!(updated.unseen(None).is_some());
        assert!(updated.unseen(Some(b"0.3.40")).is_some());
        assert!(updated.unseen(Some(b"")).is_some());
        // Put away.
        assert!(updated.unseen(Some(b"0.3.41")).is_none());

        // A release nobody wrote notes for says nothing, whatever was seen:
        // the file still describes the one before it.
        for running in ["0.3.40", "0.3.42", "0.4.0"] {
            let stale = Release::new(notes.clone(), running);
            assert!(stale.notes().is_none(), "{running}");
            assert!(stale.unseen(None).is_none(), "{running}");
            assert!(stale.unseen(Some(b"0.3.40")).is_none(), "{running}");
        }

        // And a file that did not parse is no notes at all.
        assert!(Release::new(None, "0.3.41").unseen(None).is_none());
    }

    #[test]
    fn the_committed_notes_follow_every_rule_of_the_format() {
        let notes = parse(COMMITTED)
            .unwrap_or_else(|fault| panic!("examples/launcher/whats-new.txt: {fault}"));
        assert!(
            (2..=5).contains(&notes.changes.len()),
            "whats-new.txt lists {} changes",
            notes.changes.len()
        );
        for change in &notes.changes {
            assert!(
                change.lead().split_whitespace().count() <= LEAD_WORDS,
                "the lead {:?} is longer than {LEAD_WORDS} words",
                change.lead()
            );
            assert!(
                change.text.chars().count() <= CHANGE_CHARACTERS,
                "{:?} is longer than {CHANGE_CHARACTERS} characters",
                change.text
            );
            // Copy on the panel is plain: no long dash and nothing shouted.
            assert!(
                !change.text.contains(['\u{2014}', '!']),
                "{:?} is not plain copy",
                change.text
            );
        }
        let mut leads: Vec<&str> = notes.changes.iter().map(super::Change::lead).collect();
        leads.sort_unstable();
        leads.dedup();
        assert_eq!(leads.len(), notes.changes.len(), "two changes share a lead");
    }

    /// The file is written before the version is bumped and shown after, so
    /// between releases it names the version that comes next. Once a bump
    /// leaves it behind it would never be shown again, and nothing on the
    /// panel would say that the release went out with no notes.
    #[test]
    fn the_committed_notes_are_for_this_version_or_the_one_being_prepared() {
        let notes = parse(COMMITTED)
            .unwrap_or_else(|fault| panic!("examples/launcher/whats-new.txt: {fault}"));
        let running = env!("CARGO_PKG_VERSION");
        let [major, minor, patch] =
            numbers(running).expect("the workspace version is three numbers");
        let next = format!("{major}.{minor}.{}", patch + 1);
        assert!(
            notes.version == running || notes.version == next,
            "examples/launcher/whats-new.txt describes {}, and this is {running}. \
             Rewrite the file for {running}: put {running} on its first line and \
             list what changed since {}. Owners are shown nothing until you do. \
             See docs/RELEASE-TRAIN.md.",
            notes.version,
            notes.version,
        );
    }
}
