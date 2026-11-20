//! A word held under a finger: what a dictionary says of it, and what the
//! reader keeps of it.
//!
//! # Why this lives in the reader and not in an application
//!
//! The first application to answer a held word wrote the whole of it for
//! itself: the screen a definition is read on, the highlight, the field a
//! note is typed into, and the bookkeeping that carries one word through all
//! three. The built-in library never got any of it, so the one reader every
//! owner has could not say what a word meant. A hold is something the page
//! does, the same on every page of every application that shows one, and the
//! page is drawn here.
//!
//! # Why the reader asks nothing of the device itself
//!
//! Looking a word up is a request to the runtime, and a request needs a
//! `Context` the reader is never handed: it is a view over a document, and it
//! is told things rather than fetching them. So [`Reader::hold`] answers with
//! the word to ask about, whatever owns the context asks, and the answer comes
//! back through [`Reader::define`]. The reader in between is drawing a screen
//! that says it is looking, which is true for exactly as long as it is drawn.

use kobo_sdk::keyboard::Typing;
use kobo_sdk::{DictionaryEntry, Screen, ScreenBuilder, TextHit};
use kobo_ui::{DisplayMetrics, Face};

use crate::{
    action, AnnotationId, Chrome, Locator, Outcome, Reader, TextPosition, TextRange,
    MAX_ANNOTATIONS,
};

/// How much of a definition is offered to the panel, in characters, longest
/// first.
///
/// A dictionary entry may run to four kilobytes and the screen it is read on
/// also has to hold the word, where it came from and the way back. The layout
/// drops whatever does not fit without saying so, which for a definition means
/// a sense that stops mid-clause. Each of these is tried in turn and the first
/// that shows whole is kept, so what is cut is cut at a word and marked.
const DEFINITION_LENGTHS: [usize; 6] = [900, 640, 440, 300, 200, 120];

/// The word a finger was held on, for as long as its screen is up.
#[derive(Clone, Debug)]
pub(crate) struct Held {
    range: TextRange,
    word: String,
    /// What the dictionaries said. `None` until they have answered, which is
    /// a different thing from their having nothing to say.
    entries: Option<Vec<DictionaryEntry>>,
    /// Which of several definitions is on screen.
    showing: usize,
    /// The highlight a note is being typed for.
    noting: Option<AnnotationId>,
}

impl Reader {
    /// Takes a word held on the page, and answers with the word to look up.
    ///
    /// `None` when the hold did not land on words of this book, which leaves
    /// the caller free to treat it as the plain hold it also is.
    pub fn hold(&mut self, hit: TextHit, panel: &DisplayMetrics) -> Option<String> {
        let block = Locator::try_from(hit.context).ok()?;
        let range = TextRange {
            start: TextPosition {
                block,
                offset: hit.start,
            },
            end: TextPosition {
                block,
                offset: hit.end,
            },
        };
        let word = self.text_in(range)?.trim().to_owned();
        if word.is_empty() {
            return None;
        }
        self.word = Some(Held {
            range,
            word: word.clone(),
            entries: None,
            showing: 0,
            noting: None,
        });
        self.set_chrome(Chrome::Word, panel);
        Some(word)
    }

    /// Takes what the dictionaries said about `word`.
    ///
    /// Answers whether the screen changed. A reply for a word the reader has
    /// since let go of is dropped: the screen it was for is no longer up, and
    /// drawing it over the page somebody went back to would take that page
    /// away again.
    pub fn define(&mut self, word: &str, entries: Vec<DictionaryEntry>) -> bool {
        let Some(held) = self.word.as_mut().filter(|held| held.word == word) else {
            return false;
        };
        if self.chrome != Chrome::Word {
            return false;
        }
        held.entries = Some(entries);
        held.showing = 0;
        true
    }

    /// The highlight already sitting on exactly the held word, if there is
    /// one.
    ///
    /// A hold names the same word by the same offsets every time, so equality
    /// is enough to find it, and finding it is what lets the second hold on a
    /// highlighted word offer to take the highlight off rather than lay
    /// another over it.
    fn kept(&self, range: TextRange) -> Option<AnnotationId> {
        self.memory
            .annotations
            .values()
            .find(|annotation| annotation.range == range)
            .map(|annotation| annotation.id)
    }

    /// Applies one of the word screen's own actions.
    pub(crate) fn word_action(&mut self, name: &str, panel: &DisplayMetrics) -> Outcome {
        let Some(held) = self.word.clone() else {
            return Outcome::Elsewhere;
        };
        match name {
            action::WORD_BACK | action::WORD_NEXT => {
                let last = held
                    .entries
                    .as_ref()
                    .map_or(0, |entries| entries.len().saturating_sub(1));
                let showing = if name == action::WORD_NEXT {
                    held.showing.saturating_add(1).min(last)
                } else {
                    held.showing.saturating_sub(1)
                };
                if let Some(held) = self.word.as_mut() {
                    held.showing = showing;
                }
                Outcome::Repaint
            }
            action::WORD_CLEAR => {
                if let Some(id) = self.kept(held.range) {
                    let _ = self.remove_annotation(id, panel);
                }
                self.set_chrome(Chrome::Hidden, panel);
                Outcome::Save
            }
            action::WORD_HIGHLIGHT => {
                if self.keep(held.range, panel).is_none() {
                    return self.refuse_highlight(panel);
                }
                self.set_chrome(Chrome::Hidden, panel);
                Outcome::Save
            }
            action::WORD_NOTE => {
                let Some(id) = self.keep(held.range, panel) else {
                    return self.refuse_highlight(panel);
                };
                // The highlight is made before the first key is pressed, so a
                // reader who backs out of the keyboard has still marked the
                // word, which is what the field tells them will happen.
                let written = self
                    .memory
                    .annotations
                    .get(&id)
                    .and_then(|annotation| annotation.note.clone())
                    .unwrap_or_default();
                self.note.open_with(written);
                if let Some(held) = self.word.as_mut() {
                    held.noting = Some(id);
                }
                self.set_chrome(Chrome::Note, panel);
                Outcome::Save
            }
            _ => Outcome::Elsewhere,
        }
    }

    /// The highlight on `range`, made now if the word had none.
    fn keep(&mut self, range: TextRange, panel: &DisplayMetrics) -> Option<AnnotationId> {
        self.kept(range)
            .or_else(|| self.annotate(range, None, panel).ok())
    }

    /// Says the word could not be highlighted, back on the page.
    ///
    /// The only refusal a held word can meet is the ceiling on how many a
    /// book keeps, because the word came from the page and so is certainly
    /// there to mark.
    fn refuse_highlight(&mut self, panel: &DisplayMetrics) -> Outcome {
        self.set_chrome(Chrome::Hidden, panel);
        self.report(format!(
            "This book already keeps {MAX_ANNOTATIONS} highlights, which is as many as it can."
        ));
        Outcome::Repaint
    }

    /// Offers `action` to the note field while it is open.
    ///
    /// `None` when the field is closed or the action is not the field's, so
    /// everything else the reader answers is still reached.
    pub(crate) fn note_typed(
        &mut self,
        action: kobo_ui::ActionId,
        panel: &DisplayMetrics,
    ) -> Option<Outcome> {
        if self.chrome != Chrome::Note {
            return None;
        }
        let typed = self.note.handle(action)?;
        if typed == Typing::Changed {
            return Some(Outcome::Repaint);
        }
        // Anything but a submitted note leaves what was written before as it
        // was: a blank or abandoned field keeps the highlight and takes
        // nothing away.
        if let Typing::Submitted(note) = typed {
            if let Some(id) = self.word.as_ref().and_then(|held| held.noting) {
                let _ = self.edit_annotation_note(id, Some(&note));
            }
        }
        self.set_chrome(Chrome::Hidden, panel);
        Some(Outcome::Save)
    }

    /// The field a note is typed into.
    pub(crate) fn note_screen(&self, title: &str) -> Screen {
        ScreenBuilder::new("reader-note")
            .top_bar(kobo_ui::drawable_in(title, Face::Text))
            .text_entry_with_hint(
                &self.note,
                "Your note",
                "Kept beside the highlighted word. Leave it blank to keep the highlight alone.",
                "Save note",
            )
            .build()
    }

    /// What the dictionaries said about the held word, and the ways to keep
    /// it.
    pub(crate) fn word_screen(&self, title: &str) -> Screen {
        let Some(held) = &self.word else {
            return self.book_screen(title);
        };
        let mut drawn = self.word_page(title, held, DEFINITION_LENGTHS[0]);
        for most in DEFINITION_LENGTHS.into_iter().skip(1) {
            if crate::shows_whole(&drawn, &self.panel) {
                break;
            }
            drawn = self.word_page(title, held, most);
        }
        drawn
    }

    fn word_page(&self, title: &str, held: &Held, most: usize) -> Screen {
        let screen =
            ScreenBuilder::new("reader-word").top_bar(kobo_ui::drawable_in(title, Face::Text));
        let screen = match &held.entries {
            None => screen
                .heading(kobo_ui::drawable_in(&held.word, Face::Text))
                .activity("Looking up", None),
            Some(entries) => match entries.get(held.showing.min(entries.len().saturating_sub(1))) {
                // Whether no dictionary is installed or none of them has the
                // word, the runtime's answer is the same empty list, so this
                // says the one thing that is true of both and then how to
                // change it. The command is set as a command: inside the
                // sentence it would be wrapped at its hyphens and nobody
                // could type it back.
                None => screen
                    .heading(kobo_ui::drawable_in(&held.word, Face::Text))
                    .text("No dictionary on this reader has this word.")
                    .secondary("To add a dictionary, run this on your computer.")
                    .command("kobo dict install"),
                Some(entry) => {
                    let screen = screen
                        .heading(kobo_ui::drawable_in(&entry.headword, Face::Text))
                        .secondary(kobo_ui::drawable_in(&entry.dictionary, Face::Text))
                        .text(kobo_ui::drawable_in(
                            &crate::opening(&entry.definition, most),
                            Face::Text,
                        ));
                    if entries.len() > 1 {
                        screen
                            .page_turns(action::WORD_BACK, action::WORD_NEXT)
                            .page_position(
                                u16::try_from(held.showing + 1).unwrap_or(u16::MAX),
                                u16::try_from(entries.len()).unwrap_or(u16::MAX),
                            )
                    } else {
                        screen
                    }
                }
            },
        };
        // The verbs sit in the band at the foot, which the layout reserves
        // before it places a line of the definition, so a long entry loses
        // its last words rather than the way to keep the one it defines.
        let noted = self
            .kept(held.range)
            .and_then(|id| self.memory.annotations.get(&id))
            .is_some_and(|annotation| annotation.note.is_some());
        let note = if noted { "Edit note" } else { "Add note" };
        let mark = if self.kept(held.range).is_some() {
            (action::WORD_CLEAR, "Remove highlight")
        } else {
            (action::WORD_HIGHLIGHT, "Highlight")
        };
        screen
            .action_bar([
                (action::BOOK, "Back to the book"),
                mark,
                (action::WORD_NOTE, note),
            ])
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Memory;
    use kobo_doc::{Block, Document};
    use kobo_ui::{Node, QuoteRole, TextScale};

    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    fn book() -> Reader {
        book_on(&panel())
    }

    fn book_on(panel: &DisplayMetrics) -> Reader {
        let document = Document {
            blocks: vec![
                Block::Paragraph("The heron stood in the shallows.".into()),
                Block::Paragraph("Nothing else moved on the river.".into()),
            ],
            ..Document::default()
        };
        Reader::open(document, Memory::default(), panel)
    }

    /// "heron", as the layout reports a hold on it.
    fn heron() -> TextHit {
        TextHit {
            context: 0,
            start: 4,
            end: 9,
        }
    }

    fn entry(headword: &str, definition: &str) -> DictionaryEntry {
        DictionaryEntry {
            dictionary: "Pocket English".into(),
            language: "en".into(),
            headword: headword.into(),
            definition: definition.into(),
        }
    }

    fn words(screen: &Screen) -> Vec<String> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Heading { text, .. }
                | Node::Text { text, .. }
                | Node::Secondary { text, .. }
                | Node::Quote { text, .. } => Some(text.clone()),
                Node::Activity { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect()
    }

    fn bar(screen: &Screen) -> Vec<String> {
        screen.nav_bar.as_ref().map_or_else(Vec::new, |bar| {
            bar.destinations
                .iter()
                .map(|item| item.label.clone())
                .collect()
        })
    }

    #[test]
    fn a_held_word_is_the_word_asked_about_and_the_screen_says_it_is_looking() {
        let mut reader = book();
        assert_eq!(reader.hold(heron(), &panel()).as_deref(), Some("heron"));
        assert_eq!(reader.chrome(), Chrome::Word);
        let screen = reader.screen("A Book");
        assert_eq!(words(&screen), ["heron", "Looking up"]);
        assert_eq!(bar(&screen), ["Back to the book", "Highlight", "Add note"]);
    }

    #[test]
    fn a_hold_that_names_no_words_of_the_book_is_left_alone() {
        let mut reader = book();
        let off_the_end = TextHit {
            context: 0,
            start: 4,
            end: 900,
        };
        assert_eq!(reader.hold(off_the_end, &panel()), None);
        let no_such_block = TextHit {
            context: 40,
            start: 0,
            end: 3,
        };
        assert_eq!(reader.hold(no_such_block, &panel()), None);
        assert_eq!(reader.chrome(), Chrome::Hidden);
    }

    #[test]
    fn a_word_no_dictionary_has_says_so_and_sets_the_way_to_add_one_as_a_command() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        assert!(reader.define("heron", Vec::new()));
        let screen = reader.screen("A Book");
        assert_eq!(
            words(&screen),
            [
                "heron",
                "No dictionary on this reader has this word.",
                "To add a dictionary, run this on your computer.",
                "kobo dict install",
            ]
        );
        assert!(
            screen.nodes.iter().any(|node| matches!(
                node,
                Node::Quote { role: QuoteRole::Command, text, .. } if text == "kobo dict install"
            )),
            "the command is set as a command, not inside a sentence"
        );
        assert!(crate::shows_whole(&screen, &panel()));
    }

    #[test]
    fn a_definition_is_shown_under_its_headword_with_the_dictionary_it_came_from() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        reader.define(
            "heron",
            vec![
                entry("heron", "A long-legged wading bird."),
                entry("heron", "A surname."),
            ],
        );
        let screen = reader.screen("A Book");
        assert_eq!(
            words(&screen),
            ["heron", "Pocket English", "A long-legged wading bird."]
        );
        assert_eq!(
            screen.page_turns.and_then(|turns| turns.position),
            Some((1, 2)),
            "two definitions are paged between rather than stacked"
        );
        assert_eq!(reader.act(action::WORD_NEXT, &panel()), Outcome::Repaint);
        assert_eq!(words(&reader.screen("A Book"))[2], "A surname.");
        reader.act(action::WORD_NEXT, &panel());
        assert_eq!(
            words(&reader.screen("A Book"))[2],
            "A surname.",
            "the last definition is where turning on stops"
        );
        reader.act(action::WORD_BACK, &panel());
        assert_eq!(
            words(&reader.screen("A Book"))[2],
            "A long-legged wading bird."
        );
    }

    #[test]
    fn an_answer_for_a_word_already_let_go_of_does_not_come_back_over_the_page() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        assert!(
            !reader.define("river", vec![entry("river", "A stream.")]),
            "an answer about another word is not this screen's"
        );
        reader.act(action::BOOK, &panel());
        assert!(!reader.define("heron", vec![entry("heron", "A bird.")]));
        assert_eq!(reader.chrome(), Chrome::Hidden);
    }

    #[test]
    fn the_longest_definition_is_cut_to_what_the_panel_shows_at_every_text_size() {
        let long = "A long-legged wading bird of the shallows. ".repeat(95);
        for scale in TextScale::STEPS {
            let mut metrics = panel();
            metrics.text_scale = scale;
            let mut reader = book_on(&metrics);
            reader.hold(heron(), &metrics);
            reader.define("heron", vec![entry("heron", &long), entry("heron", &long)]);
            let screen = reader.screen("A Book");
            assert!(
                crate::shows_whole(&screen, &metrics),
                "the definition ran off the panel at {}%",
                scale.percent()
            );
            let shown = &words(&screen)[2];
            assert!(shown.ends_with('\u{2026}'), "what was cut is marked as cut");
            assert_eq!(bar(&screen).len(), 3, "the ways to keep the word stay");
        }
    }

    #[test]
    fn highlighting_a_held_word_marks_those_letters_and_goes_back_to_the_page() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        assert_eq!(reader.act(action::WORD_HIGHLIGHT, &panel()), Outcome::Save);
        assert_eq!(reader.chrome(), Chrome::Hidden);
        let kept = reader.annotations();
        assert_eq!(kept.len(), 1);
        assert_eq!(reader.text_in(kept[0].range).as_deref(), Some("heron"));
        assert_eq!(kept[0].note, None);
    }

    #[test]
    fn holding_a_highlighted_word_offers_to_take_the_highlight_off_rather_than_add_another() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        reader.act(action::WORD_HIGHLIGHT, &panel());
        reader.hold(heron(), &panel());
        assert_eq!(
            bar(&reader.screen("A Book")),
            ["Back to the book", "Remove highlight", "Add note"]
        );
        assert_eq!(reader.act(action::WORD_CLEAR, &panel()), Outcome::Save);
        assert!(reader.annotations().is_empty());
        assert_eq!(reader.chrome(), Chrome::Hidden);
    }

    #[test]
    fn asking_for_a_note_highlights_the_word_first_and_opens_the_field() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        assert_eq!(reader.act(action::WORD_NOTE, &panel()), Outcome::Save);
        assert_eq!(reader.chrome(), Chrome::Note);
        assert_eq!(reader.annotations().len(), 1);
        assert_eq!(words(&reader.screen("A Book"))[0], "Your note");
        // Back from the field keeps the highlight and closes the field.
        assert_eq!(
            reader.act_on(kobo_ui::ActionId::BACK, &panel()),
            Outcome::Repaint
        );
        assert_eq!(reader.chrome(), Chrome::Hidden);
        assert_eq!(reader.annotations().len(), 1);
    }

    #[test]
    fn back_from_a_definition_returns_to_the_page_and_every_control_on_it_answers() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        for name in [
            action::WORD_HIGHLIGHT,
            action::WORD_NOTE,
            action::WORD_BACK,
            action::WORD_NEXT,
            action::WORD_CLEAR,
        ] {
            let mut reader = reader.clone();
            assert_ne!(
                reader.act_on(kobo_sdk::action_id(name), &panel()),
                Outcome::Elsewhere,
                "{name} reached the screen and nothing answered it"
            );
        }
        assert_eq!(
            reader.act_on(kobo_ui::ActionId::BACK, &panel()),
            Outcome::Repaint
        );
        assert_eq!(reader.chrome(), Chrome::Hidden);
        assert_eq!(
            reader.act_on(kobo_sdk::action_id(action::WORD_HIGHLIGHT), &panel()),
            Outcome::Elsewhere,
            "the word was let go of with its screen"
        );
    }

    #[test]
    fn a_noted_word_is_listed_under_highlights_and_notes_with_its_note_after_a_colon() {
        let mut reader = book();
        reader.hold(heron(), &panel());
        reader.act(action::WORD_NOTE, &panel());
        let id = reader.annotations()[0].id;
        reader
            .edit_annotation_note(id, Some("seen at dusk"))
            .unwrap();
        reader.act(action::HIGHLIGHTS, &panel());
        let screen = reader.screen("A Book");
        let labels: Vec<String> = screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                Node::Heading { text, .. } => Some(text.clone()),
                Node::Button { label, .. } => Some(label.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(labels, ["Highlights and notes", "heron: seen at dusk"]);
    }
}
