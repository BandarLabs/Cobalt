//! Offline card review with persistent due state.
use kobo_sdk::{action_id, ActionId, Context, KoboApp, Screen, ScreenBuilder, StoreResult};
use std::process::ExitCode;

const STATE: &str = "review-state";
#[derive(Clone, Copy)]
struct Card {
    front: &'static str,
    back: &'static str,
}
const CARDS: [Card; 4] = [
    Card {
        front: "French: bonjour",
        back: "hello",
    },
    Card {
        front: "Capital of Japan",
        back: "Tokyo",
    },
    Card {
        front: "2 × 8",
        back: "16",
    },
    Card {
        front: "Spanish: gracias",
        back: "thank you",
    },
];
#[derive(Clone, Copy, PartialEq)]
enum View {
    Decks,
    Review,
    Stats,
    Finished,
}
struct Flashcards {
    view: View,
    card: usize,
    revealed: bool,
    reviews: u16,
    again: u16,
    loaded: bool,
}
impl Default for Flashcards {
    fn default() -> Self {
        Self {
            view: View::Decks,
            card: 0,
            revealed: false,
            reviews: 0,
            again: 0,
            loaded: false,
        }
    }
}
impl Flashcards {
    fn save(&self, c: &mut Context) {
        c.store().save(
            STATE,
            format!("{},{},{}", self.card, self.reviews, self.again),
        );
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Decks => {
                let mut s = ScreenBuilder::new("flashcards-decks")
                    .top_bar("Flashcards")
                    .secondary(if self.loaded {
                        format!(
                            "{} due · {} reviewed today",
                            CARDS.len().saturating_sub(self.card),
                            self.reviews
                        )
                    } else {
                        "Loading collection…".into()
                    });
                s = s.rows_with_trailing([
                    (
                        "review",
                        "Default",
                        "New 4 · learning 0 · due 0",
                        kobo_sdk::Glyph::Bookmark,
                        format!("{} due", CARDS.len().saturating_sub(self.card)),
                    ),
                    (
                        "stats",
                        "Stats",
                        "Today and collection counts",
                        kobo_sdk::Glyph::Chart,
                        String::new(),
                    ),
                ]);
                s.build()
            }
            View::Review => {
                let c = CARDS[self.card];
                let mut s = ScreenBuilder::new("flashcards-review")
                    .top_bar("Default")
                    .heading(c.front)
                    .secondary(format!("{} of {}", self.card + 1, CARDS.len()));
                if self.revealed {
                    s = s
                        .text(c.back)
                        .grid(
                            2,
                            false,
                            [
                                ("again", "Again · <1m"),
                                ("hard", "Hard · 6m"),
                                ("good", "Good · 3d"),
                                ("easy", "Easy · 7d"),
                            ],
                        )
                        .bottom_action("undo", "Undo");
                } else {
                    s = s.bottom_action("show", "Show answer");
                }
                s.build()
            }
            View::Stats => ScreenBuilder::new("flashcards-stats")
                .top_bar("Stats")
                .rows_with_trailing([
                    (
                        "today",
                        "Today",
                        format!("{} reviews · {} again", self.reviews, self.again),
                        kobo_sdk::Glyph::Chart,
                        String::new(),
                    ),
                    (
                        "collection",
                        "Collection",
                        format!("{} cards remaining", CARDS.len().saturating_sub(self.card)),
                        kobo_sdk::Glyph::Bookmark,
                        String::new(),
                    ),
                ])
                .owns_back(true)
                .build(),
            View::Finished => ScreenBuilder::new("flashcards-finished")
                .top_bar("Flashcards")
                .empty_state("No cards are due. Reviews appear here after a deck transfer.")
                .bottom_action("decks", "Decks")
                .build(),
        }
    }
    fn answer(&mut self, again: bool) {
        self.reviews += 1;
        self.again += u16::from(again);
        self.card += 1;
        self.revealed = false;
        self.view = if self.card == CARDS.len() {
            View::Finished
        } else {
            View::Review
        };
    }
}
impl KoboApp for Flashcards {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(STATE);
        c.set_screen(self.screen());
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { key, value } = r {
            if key == STATE {
                if let Some(bytes) = value {
                    if let Ok(s) = String::from_utf8(bytes) {
                        let p: Vec<_> = s.split(',').collect();
                        if p.len() == 3 {
                            self.card = p[0].parse::<usize>().unwrap_or(0).min(CARDS.len());
                            self.reviews = p[1].parse().unwrap_or(0);
                            self.again = p[2].parse().unwrap_or(0);
                        }
                    }
                }
                self.loaded = true;
                c.set_screen(self.screen());
            }
        }
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if a == action_id("review") {
            self.view = if self.card == CARDS.len() {
                View::Finished
            } else {
                View::Review
            };
        } else if a == action_id("stats") {
            self.view = View::Stats;
        } else if a == action_id("show") {
            self.revealed = true;
        } else if ["again", "hard", "good", "easy"]
            .iter()
            .any(|n| a == action_id(n))
        {
            self.answer(a == action_id("again"));
            self.save(c);
        } else if a == action_id("decks") || a == ActionId::BACK {
            self.view = View::Decks;
        }
        c.set_screen(self.screen());
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("flashcards", Flashcards::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flashcards: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn answer_advances_and_counts_again() {
        let mut a = Flashcards::default();
        a.view = View::Review;
        a.answer(true);
        assert_eq!((a.card, a.reviews, a.again), (1, 1, 1));
    }
    #[test]
    fn review_controls_fit() {
        let mut a = Flashcards::default();
        a.view = View::Review;
        a.revealed = true;
        let l = a
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for n in ["again", "hard", "good", "easy"] {
            assert!(
                l.rect_of_action(action_id(n)).unwrap().height
                    >= CLARA_BW_METRICS.touch_target_minimum()
            );
        }
        assert!(a
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
