//! Going to a place in the book that is not next to this one.
//!
//! # Why a stepper and a number, and nothing dragged
//!
//! The control every other reader gives this is a slider, and a slider is a
//! thing dragged under a finger while the page follows it. This panel repaints
//! a few times a second at best, so the page cannot follow, and a thumb moved
//! across it lands a chapter away from where it was meant to. What works on it
//! is a tap that moves a known amount and says where that is: a tenth of the
//! book at a time, with the page it comes to written beside it, and one more
//! tap to go there once it reads right. A reader who knows the page they want
//! types it, on the digits.
//!
//! # Why a page is all that is asked for
//!
//! A page number is not a place and nothing here keeps one. It is asked for
//! because it is what a reader has in their head, turned at once into the
//! block that page opens with, and the block is what is gone to and kept. The
//! same number typed after the type has been made larger is a different
//! block, which is correct: it is the page the reader can see the count of.

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{BannerLevel, Screen, ScreenBuilder};
use kobo_ui::{DisplayMetrics, Face, Glyph};

use crate::{action, Chrome, Outcome, Reader};

/// How far one tap of the stepper moves through the book, out of a hundred.
///
/// A tenth. Finer than that is more taps than a reader will make on a panel
/// that flashes at each one, and what is left to do after the nearest tenth
/// is a few page turns or a typed number.
const NOTCH: usize = 10;

/// Where a reader is thinking of going, before they have gone.
#[derive(Clone, Debug)]
pub(crate) struct GoTo {
    /// How far through the book the stepper stands, out of a hundred. `None`
    /// until it has been moved, when it stands wherever the reader is.
    notch: Option<usize>,
    /// The field a page number is typed into, opened on the digits.
    entry: TextEntry,
    /// Whether what was last typed was not a page of this book.
    refused: bool,
}

impl Default for GoTo {
    fn default() -> Self {
        Self {
            notch: None,
            entry: TextEntry::new().numeric(),
            refused: false,
        }
    }
}

impl GoTo {
    /// Closes the field if it is open and forgets where the stepper stood,
    /// so the next time it is opened it starts from where the reader is.
    pub(crate) fn put_away(&mut self) {
        if self.entry.is_open() {
            self.entry.close();
        }
        self.notch = None;
        self.refused = false;
    }
}

impl Reader {
    /// The last page, counted from zero, which is where a hundred percent is.
    fn last_page(&self) -> usize {
        self.pages.len().saturating_sub(1)
    }

    /// How far through the book a page is, out of a hundred.
    fn percent_at(&self, page: usize) -> usize {
        match self.last_page() {
            0 => 0,
            last => (page.min(last) * 100 + last / 2) / last,
        }
    }

    /// The page a percentage of the way through the book falls on, counted
    /// from zero.
    fn page_at(&self, percent: usize) -> usize {
        (percent.min(100) * self.last_page() + 50) / 100
    }

    /// How far through the book the stepper stands.
    fn notch(&self) -> usize {
        self.go_to
            .notch
            .unwrap_or_else(|| self.percent_at(self.page))
    }

    /// The page the stepper would go to, counted from zero.
    ///
    /// The page the reader is on until the stepper is moved, rather than the
    /// page its percentage rounds to, so opening this screen and going
    /// nowhere is never offered as somewhere to go.
    fn target(&self) -> usize {
        self.go_to
            .notch
            .map_or(self.page, |notch| self.page_at(notch))
    }

    /// Applies one of the go-to screen's own actions.
    pub(crate) fn go_to_action(&mut self, name: &str, panel: &DisplayMetrics) -> Outcome {
        match name {
            action::GO_TO => {
                self.set_chrome(Chrome::GoTo, panel);
                Outcome::Repaint
            }
            action::GO_TO_LESS | action::GO_TO_MORE => {
                // From a place between two notches the first tap goes to the
                // nearer one in that direction, not a whole tenth past it.
                let at = self.notch();
                let next = if name == action::GO_TO_MORE {
                    (at / NOTCH + 1) * NOTCH
                } else {
                    at.saturating_sub(1) / NOTCH * NOTCH
                };
                self.go_to.notch = Some(next.min(100));
                self.go_to.refused = false;
                Outcome::Repaint
            }
            action::GO_TO_GO => {
                let page = self.target();
                self.go_to_page(page, panel)
            }
            action::GO_TO_TYPE => {
                self.go_to.refused = false;
                self.go_to.entry.open();
                self.set_chrome(Chrome::GoToPage, panel);
                Outcome::Repaint
            }
            _ => Outcome::Elsewhere,
        }
    }

    /// Turns to a page counted from zero, leaving a way back to this one.
    fn go_to_page(&mut self, page: usize, panel: &DisplayMetrics) -> Outcome {
        let page = page.min(self.last_page());
        if page != self.page {
            self.leave_a_way_back();
            self.found = None;
            self.page = page;
            self.remember_position();
        }
        self.set_chrome(Chrome::Hidden, panel);
        Outcome::Save
    }

    /// Offers `action` to the page number field while it is open.
    pub(crate) fn go_to_typed(
        &mut self,
        action: kobo_ui::ActionId,
        panel: &DisplayMetrics,
    ) -> Option<Outcome> {
        if self.chrome != Chrome::GoToPage {
            return None;
        }
        match self.go_to.entry.handle(action)? {
            Typing::Changed => Some(Outcome::Repaint),
            Typing::Submitted(typed) => {
                let wanted = typed
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|page| (1..=self.pages.len()).contains(page));
                Some(match wanted {
                    Some(page) => self.go_to_page(page - 1, panel),
                    // Back to the screen the field was opened from, which
                    // says what a page of this book is, rather than to the
                    // page with nothing said about why it did not move.
                    None => {
                        self.go_to.refused = true;
                        self.chrome = Chrome::GoTo;
                        Outcome::Repaint
                    }
                })
            }
            _ => {
                self.chrome = Chrome::GoTo;
                Some(Outcome::Repaint)
            }
        }
    }

    /// The field a page number is typed into.
    pub(crate) fn go_to_page_screen(&self, title: &str) -> Screen {
        ScreenBuilder::new("reader-go-to-page")
            .top_bar(kobo_ui::drawable_in(title, Face::Text))
            .text_entry_with_hint(
                &self.go_to.entry,
                "Page",
                &format!("A number from 1 to {}.", self.pages.len()),
                "Go",
            )
            .build()
    }

    /// The stepper through the book, and the ways off it.
    pub(crate) fn go_to_screen(&self, title: &str) -> Screen {
        let pages = self.pages.len();
        let notch = self.notch();
        let target = self.target();
        let mut screen = ScreenBuilder::new("reader-go-to")
            .top_bar(kobo_ui::drawable_in(title, Face::Text))
            .heading("Go to")
            .secondary(format!("You are on page {} of {pages}.", self.page + 1));
        if self.go_to.refused {
            screen = screen.banner(
                BannerLevel::Attention,
                format!("The pages run from 1 to {pages}. Type one of those."),
            );
        }
        let go = format!("Go to page {}", target + 1);
        screen = screen
            .stepper(
                format!("{notch}% \u{b7} Page {}", target + 1),
                action::GO_TO_LESS,
                Glyph::Minus,
                action::GO_TO_MORE,
                Glyph::Plus,
            )
            .stepper_ends(notch > 0, notch < 100)
            .stepper_track(u8::try_from(notch).unwrap_or(100));
        // Live only once the stepper names another page. A control that goes
        // to the page already open would answer a tap by closing this screen
        // and changing nothing, which reads as the tap having missed. It is
        // drawn either way, at the same size, so nothing under the stepper
        // moves when the first tap wakes it.
        let state = if target == self.page {
            kobo_ui::ControlState::Disabled
        } else {
            kobo_ui::ControlState::Enabled
        };
        screen
            .primary_button_with_state(action::GO_TO_GO, go, state)
            .action_bar([
                (action::BOOK, "Back to the book"),
                (action::GO_TO_TYPE, "Type a page"),
            ])
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Memory;
    use kobo_doc::{Block, Document};
    use kobo_ui::{ControlState, Node};

    fn panel() -> DisplayMetrics {
        DisplayMetrics::default()
    }

    /// A book long enough that a tenth of it is several pages.
    fn book() -> Reader {
        let document = Document {
            blocks: (0..400)
                .map(|n| {
                    Block::Paragraph(format!(
                        "Line {n} of a letter that runs on for page after page about the river."
                    ))
                })
                .collect(),
            ..Document::default()
        };
        Reader::open(document, Memory::default(), &panel())
    }

    /// What the stepper reads, and whether the control under it is live.
    fn stepper(reader: &Reader) -> (String, bool) {
        let screen = reader.screen("A Letter");
        let label = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                Node::Stepper { label, .. } => Some(label.clone()),
                _ => None,
            })
            .expect("the go-to screen has a stepper");
        let live = screen.nodes.iter().any(|node| {
            matches!(
                node,
                Node::Button {
                    state: ControlState::Enabled,
                    ..
                }
            )
        });
        (label, live)
    }

    #[test]
    fn the_controls_say_the_place_on_the_line_that_leads_to_changing_it() {
        let mut reader = book();
        reader.act(action::CONTROLS, &panel());
        assert_eq!(
            reader.act_on(kobo_sdk::action_id(action::GO_TO), &panel()),
            Outcome::Repaint
        );
        assert_eq!(reader.chrome(), Chrome::GoTo);
    }

    #[test]
    fn the_stepper_starts_where_the_reader_is_and_offers_nowhere_until_it_is_moved() {
        let mut reader = book();
        for _ in 0..3 {
            reader.act(action::FORWARD, &panel());
        }
        reader.act(action::GO_TO, &panel());
        let (label, live) = stepper(&reader);
        assert!(
            label.ends_with("\u{b7} Page 4"),
            "the stepper opened on {label}"
        );
        assert!(
            !live,
            "the page already open was offered as somewhere to go"
        );
    }

    #[test]
    fn each_tap_moves_to_the_next_tenth_and_names_the_page_it_falls_on() {
        let mut reader = book();
        let pages = reader.page_count();
        reader.act(action::FORWARD, &panel());
        assert!(
            (1..10).contains(&reader.percent_at(1)),
            "the second page is between the start and the first tenth"
        );
        reader.act(action::GO_TO, &panel());
        // From between two tenths, the first tap goes to the nearer one.
        reader.act(action::GO_TO_MORE, &panel());
        let (label, live) = stepper(&reader);
        let tenth = (10 * (pages - 1) + 50) / 100 + 1;
        assert_eq!(label, format!("10% \u{b7} Page {tenth}"));
        assert!(live);
        reader.act(action::GO_TO_LESS, &panel());
        assert_eq!(stepper(&reader).0, "0% \u{b7} Page 1");
        reader.act(action::GO_TO_LESS, &panel());
        assert_eq!(
            stepper(&reader).0,
            "0% \u{b7} Page 1",
            "it stops at the start"
        );
        for _ in 0..12 {
            reader.act(action::GO_TO_MORE, &panel());
        }
        assert_eq!(
            stepper(&reader).0,
            format!("100% \u{b7} Page {pages}"),
            "and at the end"
        );
    }

    #[test]
    fn going_turns_to_the_page_named_and_back_returns_to_the_page_left() {
        let mut reader = book();
        reader.act(action::FORWARD, &panel());
        let left = reader.page_number();
        reader.act(action::GO_TO, &panel());
        for _ in 0..5 {
            reader.act(action::GO_TO_MORE, &panel());
        }
        let (label, _) = stepper(&reader);
        assert_eq!(
            reader.act_on(kobo_sdk::action_id(action::GO_TO_GO), &panel()),
            Outcome::Save
        );
        assert_eq!(reader.chrome(), Chrome::Hidden);
        assert!(label.ends_with(&format!("Page {}", reader.page_number())));
        assert_ne!(reader.page_number(), left);
        assert_eq!(
            reader.act_on(kobo_ui::ActionId::BACK, &panel()),
            Outcome::Save
        );
        assert_eq!(reader.page_number(), left);
    }

    #[test]
    fn the_screen_opens_afresh_each_time_rather_than_where_the_stepper_was_left() {
        let mut reader = book();
        reader.act(action::GO_TO, &panel());
        reader.act(action::GO_TO_MORE, &panel());
        reader.act(action::BOOK, &panel());
        reader.act(action::GO_TO, &panel());
        assert_eq!(stepper(&reader).0, "0% \u{b7} Page 1");
    }

    #[test]
    fn a_typed_page_is_gone_to_and_one_the_book_does_not_have_is_said_not_to_be_one() {
        let mut reader = book();
        let pages = reader.page_count();
        reader.act(action::GO_TO, &panel());
        assert_eq!(
            reader.act_on(kobo_sdk::action_id(action::GO_TO_TYPE), &panel()),
            Outcome::Repaint
        );
        assert_eq!(reader.chrome(), Chrome::GoToPage);
        let field = reader.screen("A Letter");
        assert!(matches!(
            field.nodes.get(1),
            Some(Node::Secondary { text, .. }) if *text == format!("A number from 1 to {pages}.")
        ));

        // The runtime's answer is stood in for here: the field is the
        // runtime's, and what it hands back is the typed text.
        reader.go_to.entry.close();
        reader.go_to.refused = true;
        reader.chrome = Chrome::GoTo;
        let refused = reader.screen("A Letter");
        assert!(refused.nodes.iter().any(|node| matches!(
            node,
            Node::Banner { text, .. } if *text == format!("The pages run from 1 to {pages}. Type one of those.")
        )));
        assert!(crate::shows_whole(&refused, &panel()));

        assert_eq!(reader.go_to_page(pages + 40, &panel()), Outcome::Save);
        assert_eq!(reader.page_number(), pages, "past the end is the end");
    }

    #[test]
    fn the_go_to_screen_fits_the_panel_and_every_control_on_it_answers_at_every_text_size() {
        for scale in kobo_ui::TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..panel()
            };
            let mut reader = book();
            reader.restore(Memory::default(), &metrics);
            reader.act(action::GO_TO, &metrics);
            reader.act(action::GO_TO_MORE, &metrics);
            assert!(
                crate::shows_whole(&reader.screen("A Letter"), &metrics),
                "the go-to screen ran off the panel at {}%",
                scale.percent()
            );
            for name in [
                action::GO_TO_LESS,
                action::GO_TO_MORE,
                action::GO_TO_GO,
                action::GO_TO_TYPE,
            ] {
                assert_ne!(
                    reader.clone().act_on(kobo_sdk::action_id(name), &metrics),
                    Outcome::Elsewhere,
                    "{name} reached the screen and nothing answered it"
                );
            }
        }
    }
}
