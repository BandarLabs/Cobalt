mod import;
mod model;
mod scheduler;

use crate::model::{decode, encode, Card, Library};
use crate::scheduler::{answer, preview, Rating};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult,
};
use std::process::ExitCode;

const LIBRARY: &str = "anki-library-v1";
const INCOMING: &str = "incoming-anki-package";
const MAX_LIBRARY: usize = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Decks,
    Review,
    Stats,
    Finished,
    Transfer,
}
struct Undo {
    index: usize,
    card: Card,
    reviews: u32,
    again: u32,
}
struct Flashcards {
    library: Library,
    loaded: bool,
    view: View,
    deck: Option<String>,
    current: Option<usize>,
    revealed: bool,
    undo: Option<Undo>,
    notice: Option<String>,
    loading: Option<ShelfDownload>,
    incoming: Option<ShelfDownload>,
    saving: Option<ShelfUpload>,
    confirm_import: bool,
    pending_import: Option<Library>,
}
impl Default for Flashcards {
    fn default() -> Self {
        Self {
            library: Library::default(),
            loaded: false,
            view: View::Decks,
            deck: None,
            current: None,
            revealed: false,
            undo: None,
            notice: None,
            loading: None,
            incoming: None,
            saving: None,
            confirm_import: false,
            pending_import: None,
        }
    }
}
impl Flashcards {
    fn today() -> i32 {
        i32::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                / 86_400,
        )
        .unwrap_or(i32::MAX)
    }
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen().with_own_back(self.view != View::Decks));
    }
    fn deck_rows(&self) -> Vec<(String, String, String, Glyph, String)> {
        self.library
            .decks(Self::today())
            .into_iter()
            .enumerate()
            .map(|(index, (deck, new, learning, due))| {
                let total = new + learning + due;
                (
                    format!("deck-{index}"),
                    deck,
                    format!("New {new} · learning {learning} · due {due}"),
                    Glyph::Bookmark,
                    format!("{total} due"),
                )
            })
            .collect()
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Decks => self.decks_screen(),
            View::Review => self.review_screen(),
            View::Stats => self.stats_screen(),
            View::Finished => Self::finished_screen(),
            View::Transfer => self.transfer_screen(),
        }
    }
    fn decks_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("flashcards-decks")
            .top_bar("Flashcards")
            .top_bar_glyph("transfer", "Transfer", Glyph::Download);
        if !self.loaded {
            return screen.skeleton(4).build();
        }
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        let rows = self.deck_rows();
        if rows.is_empty() {
            screen
                .splash(
                    Some(Glyph::Bookmark),
                    "No cards yet",
                    "Anki package import is paused until host compatibility support is ready.",
                )
                .build()
        } else {
            screen
                .rows_with_trailing(rows)
                .rows_with_trailing([(
                    "stats",
                    "Stats",
                    format!(
                        "{} reviews today · {} again",
                        self.library.reviews_today, self.library.again_today
                    ),
                    Glyph::Chart,
                    String::new(),
                )])
                .build()
        }
    }
    fn review_screen(&self) -> Screen {
        let Some(index) = self.current else {
            return Self::finished_screen();
        };
        let card = &self.library.cards[index];
        let mut screen = ScreenBuilder::new("flashcards-review")
            .top_bar(card.deck.clone())
            .heading(card.front.clone())
            .secondary(format!(
                "{} due in this deck",
                self.library
                    .decks(Self::today())
                    .iter()
                    .find(|(name, ..)| name == &card.deck)
                    .map_or(0, |(_, new, learning, due)| new + learning + due)
            ));
        if self.saving.is_some() {
            return screen.secondary("Saving answer…").build();
        }
        if self.revealed {
            let labels = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy].map(|rating| {
                let days = preview(card, Self::today(), rating).days;
                (
                    rating.action(),
                    format!(
                        "{} · {}d",
                        match rating {
                            Rating::Again => "Again",
                            Rating::Hard => "Hard",
                            Rating::Good => "Good",
                            Rating::Easy => "Easy",
                        },
                        days
                    ),
                )
            });
            screen = screen.text(card.back.clone()).grid(2, false, labels);
            if self.undo.is_some() {
                screen = screen.bottom_action("undo", "Undo last answer");
            }
        } else {
            screen = screen.bottom_action("show", "Show answer");
        }
        screen.build()
    }
    fn stats_screen(&self) -> Screen {
        let today = Self::today();
        let new = self
            .library
            .cards
            .iter()
            .filter(|card| card.due_day <= today && card.state == model::CardState::New)
            .count();
        let learning = self
            .library
            .cards
            .iter()
            .filter(|card| card.due_day <= today && card.state == model::CardState::Learning)
            .count();
        let review = self
            .library
            .cards
            .iter()
            .filter(|card| card.due_day <= today && card.state == model::CardState::Review)
            .count();
        ScreenBuilder::new("flashcards-stats")
            .top_bar("Stats")
            .facts([
                (
                    "Today",
                    format!(
                        "{} reviews · {} again",
                        self.library.reviews_today, self.library.again_today
                    ),
                ),
                (
                    "Due",
                    format!("{new} new · {learning} learning · {review} review"),
                ),
                ("Library", format!("{} cards", self.library.cards.len())),
            ])
            .button("decks", "Decks")
            .build()
    }
    fn finished_screen() -> Screen {
        ScreenBuilder::new("flashcards-finished")
            .top_bar("Flashcards")
            .splash(
                Some(Glyph::Check),
                "Caught up",
                "No cards are due in this deck.",
            )
            .bottom_action("decks", "Decks")
            .build()
    }
    fn transfer_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("flashcards-transfer")
            .top_bar("Transfer")
            .heading("Import paused")
            .text("Anki package import is unavailable until the host uses Anki's renderer and media importer. Existing cards remain unchanged.");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        if self.confirm_import {
            screen = screen.confirm(
                "Replace this library?",
                format!(
                    "{} cards will be replaced by this .colpkg.",
                    self.library.cards.len()
                ),
                ("confirm-import", "Replace library"),
                ("cancel-import", "Cancel"),
            );
        } else if let Some(incoming) = &self.incoming {
            screen = screen.transfer("Reading package", incoming.bytes().len() as u64, None);
        } else if self.saving.is_some() {
            screen = screen.transfer("Saving library", 0, None);
        }
        screen.build()
    }
    fn open_deck(&mut self, deck: &str) {
        self.deck = Some(deck.to_owned());
        self.current = self.library.next_due(deck, Self::today());
        self.revealed = false;
        self.view = if self.current.is_some() {
            View::Review
        } else {
            View::Finished
        };
    }
    fn save(&mut self, cx: &mut Context) {
        let bytes = encode(&self.library);
        if bytes.len() > MAX_LIBRARY {
            self.notice = Some("The imported library is too large for this reader. Remove media or split the deck.".to_owned());
            return;
        }
        let mut upload = ShelfUpload::new(LIBRARY, bytes);
        upload.start(cx);
        self.saving = Some(upload);
    }
    fn import_received(&mut self, cx: &mut Context) {
        if self.incoming.is_some() {
            return;
        }
        let mut download = ShelfDownload::new(INCOMING).at_most(MAX_LIBRARY);
        download.start(cx);
        self.incoming = Some(download);
        self.notice = None;
    }
    fn merge_import(&mut self, imported: Library) {
        for card in imported.cards {
            if let Some(existing) = self
                .library
                .cards
                .iter_mut()
                .find(|existing| existing.id == card.id)
            {
                *existing = card;
            } else {
                self.library.cards.push(card);
            }
        }
        self.library.transfer_at = imported.transfer_at;
    }
    fn finish_import(&mut self, cx: &mut Context, imported: Library, replace: bool) {
        if replace {
            self.library = imported;
        } else {
            self.merge_import(imported);
        }
        self.notice = Some(format!("Imported {} cards.", self.library.cards.len()));
        self.view = View::Decks;
        self.save(cx);
    }
    fn apply_import(&mut self, cx: &mut Context, bytes: &[u8]) {
        match import::import(bytes, Self::today()) {
            Ok(imported) => {
                if imported.replaces_collection && !self.library.cards.is_empty() {
                    self.pending_import = Some(imported.library);
                    self.confirm_import = true;
                } else {
                    self.finish_import(cx, imported.library, imported.replaces_collection);
                }
            }
            Err(error) => {
                self.notice = Some(error.to_string());
                self.view = View::Transfer;
            }
        }
    }
    fn answer(&mut self, cx: &mut Context, rating: Rating) {
        let Some(index) = self.current else {
            return;
        };
        let previous = self.library.cards[index].clone();
        self.undo = Some(Undo {
            index,
            card: previous,
            reviews: self.library.reviews_today,
            again: self.library.again_today,
        });
        answer(&mut self.library.cards[index], Self::today(), rating);
        self.library.reviews_today = self.library.reviews_today.saturating_add(1);
        if rating == Rating::Again {
            self.library.again_today = self.library.again_today.saturating_add(1);
        }
        let deck = self.deck.clone().unwrap_or_default();
        self.current = self.library.next_due(&deck, Self::today());
        self.revealed = false;
        if self.current.is_none() {
            self.view = View::Finished;
        }
        self.save(cx);
    }
    fn undo(&mut self, cx: &mut Context) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        self.library.cards[undo.index] = undo.card;
        self.library.reviews_today = undo.reviews;
        self.library.again_today = undo.again;
        self.current = Some(undo.index);
        self.view = View::Review;
        self.revealed = true;
        self.save(cx);
    }
}
impl KoboApp for Flashcards {
    fn on_start(&mut self, cx: &mut Context) {
        let mut download = ShelfDownload::new(LIBRARY).at_most(MAX_LIBRARY);
        download.start(cx);
        self.loading = Some(download);
        self.show(cx);
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let Some(upload) = &mut self.saving {
            match upload.advance(cx, &result) {
                ShelfProgress::Done => {
                    self.saving = None;
                    self.show(cx);
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.saving = None;
                    self.notice = Some(
                        "The library could not be saved. Free space, then answer again.".to_owned(),
                    );
                    self.show(cx);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.loading {
            match download.advance(cx, &result) {
                ShelfProgress::Done => {
                    let bytes = self.loading.take().expect("active library download").take();
                    if let Some(library) = decode(&bytes) {
                        self.library = library;
                    } else {
                        self.notice = Some(
                            "Your saved library could not be read. Re-import it after compatible host support ships."
                                .to_owned(),
                        );
                    }
                    self.loaded = true;
                    self.show(cx);
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.loaded = true;
                    self.show(cx);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.incoming {
            match download.advance(cx, &result) {
                ShelfProgress::Done => {
                    let bytes = self
                        .incoming
                        .take()
                        .expect("active package download")
                        .take();
                    self.apply_import(cx, &bytes);
                    self.show(cx);
                }
                ShelfProgress::Failed(_) => {
                    self.incoming = None;
                    self.notice = Some(
                        "No complete package arrived. Compatible host import support has not shipped yet."
                            .to_owned(),
                    );
                    self.show(cx);
                }
                ShelfProgress::Moving { .. } => {
                    self.show(cx);
                }
                ShelfProgress::Elsewhere => {}
            }
        }
    }
    fn on_action(&mut self, cx: &mut Context, action: ActionId) {
        if self.saving.is_some() {
            return;
        }
        if action == ActionId::BACK {
            if self.view == View::Decks {
                cx.exit();
                return;
            }
            if self.view == View::Transfer {
                self.incoming = None;
                self.confirm_import = false;
                self.pending_import = None;
            }
            self.view = View::Decks;
        } else if action == action_id("transfer") {
            self.view = View::Transfer;
        } else if action == action_id("import") {
            self.import_received(cx);
        } else if action == action_id("confirm-import") {
            self.confirm_import = false;
            if let Some(imported) = self.pending_import.take() {
                self.finish_import(cx, imported, true);
            }
        } else if action == action_id("cancel-import") {
            self.confirm_import = false;
            self.pending_import = None;
        } else if action == action_id("stats") {
            self.view = View::Stats;
        } else if action == action_id("decks") {
            self.view = View::Decks;
        } else if action == action_id("show") {
            self.revealed = true;
        } else if action == action_id("undo") {
            self.undo(cx);
            self.show(cx);
            return;
        } else if let Some(rating) = [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy]
            .into_iter()
            .find(|rating| action == action_id(rating.action()))
        {
            self.answer(cx, rating);
            self.show(cx);
            return;
        } else {
            let decks = self.library.decks(Self::today());
            for (index, (deck, ..)) in decks.iter().enumerate() {
                if action == action_id(&format!("deck-{index}")) {
                    self.open_deck(deck);
                    break;
                }
            }
        }
        self.show(cx);
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("flashcards", Flashcards::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flashcards: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn review_controls_are_tappable_and_fit_clara_bw() {
        let mut app = Flashcards {
            loaded: true,
            view: View::Review,
            library: Library {
                cards: vec![Card {
                    id: 1,
                    deck: "Default".into(),
                    front: "Question".into(),
                    back: "Answer".into(),
                    last_review_day: Flashcards::today(),
                    due_day: Flashcards::today(),
                    state: model::CardState::New,
                    reps: 0,
                    lapses: 0,
                    stability: None,
                    difficulty: None,
                    media: 0,
                }],
                ..Library::default()
            },
            current: Some(0),
            ..Flashcards::default()
        };
        app.revealed = true;
        let screen = app.screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for name in ["again", "hard", "good", "easy"] {
            assert!(
                layout
                    .rect_of_action(action_id(name))
                    .expect("review action")
                    .height
                    >= CLARA_BW_METRICS.touch_target_minimum()
            );
        }
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
    #[test]
    fn deck_selection_uses_only_due_cards() {
        let library = Library {
            cards: vec![Card {
                id: 1,
                deck: "Default".into(),
                front: "Q".into(),
                back: "A".into(),
                last_review_day: 5,
                due_day: 5,
                state: model::CardState::New,
                reps: 0,
                lapses: 0,
                stability: None,
                difficulty: None,
                media: 0,
            }],
            ..Library::default()
        };
        assert_eq!(library.next_due("Default", 4), None);
        assert_eq!(library.next_due("Default", 5), Some(0));
    }
    #[test]
    fn import_replacement_confirmation_is_tappable_on_clara_bw() {
        let app = Flashcards {
            loaded: true,
            view: View::Transfer,
            confirm_import: true,
            library: Library {
                cards: vec![Card {
                    id: 1,
                    deck: "Default".into(),
                    front: "Q".into(),
                    back: "A".into(),
                    last_review_day: 5,
                    due_day: 5,
                    state: model::CardState::New,
                    reps: 0,
                    lapses: 0,
                    stability: None,
                    difficulty: None,
                    media: 0,
                }],
                ..Library::default()
            },
            ..Flashcards::default()
        };
        let screen = app.screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        for name in ["confirm-import", "cancel-import"] {
            assert!(
                layout
                    .rect_of_action(action_id(name))
                    .expect("confirmation action")
                    .height
                    >= CLARA_BW_METRICS.touch_target_minimum()
            );
        }
    }

    #[test]
    fn back_exits_from_decks_and_returns_there_from_a_subscreen() {
        let mut runner = AppRunner::new(Flashcards {
            loaded: true,
            ..Flashcards::default()
        });
        runner.start();
        assert!(runner
            .action(ActionId::BACK)
            .iter()
            .any(|command| matches!(command, Command::Exit)));

        let mut runner = AppRunner::new(Flashcards {
            loaded: true,
            view: View::Stats,
            ..Flashcards::default()
        });
        runner.start();
        assert!(!runner
            .action(ActionId::BACK)
            .iter()
            .any(|command| matches!(command, Command::Exit)));
        assert_eq!(runner.app_mut().view, View::Decks);
    }

    #[test]
    fn review_waits_for_its_answer_to_save() {
        let app = Flashcards {
            loaded: true,
            view: View::Review,
            library: Library {
                cards: vec![Card {
                    id: 1,
                    deck: "Default".into(),
                    front: "Question".into(),
                    back: "Answer".into(),
                    last_review_day: Flashcards::today(),
                    due_day: Flashcards::today(),
                    state: model::CardState::New,
                    reps: 0,
                    lapses: 0,
                    stability: None,
                    difficulty: None,
                    media: 0,
                }],
                ..Library::default()
            },
            current: Some(0),
            saving: Some(ShelfUpload::new(LIBRARY, Vec::new())),
            ..Flashcards::default()
        };
        let layout = app
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("show")).is_none());
        assert!(layout.rect_of_action(action_id("again")).is_none());
    }
}
