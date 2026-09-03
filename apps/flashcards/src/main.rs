//! Offline Flashcards review for bundles prepared by `flashcards-import`.
//!
//! The app consumes only host-prepared, Cobalt-owned neutral bundles. Imported
//! HTML is already converted to bounded text and semantic emphasis; no card
//! can execute script, resolve a path, or open a network socket.

use kobo_flashcards_format::{
    card_text_font, decode, digest_hex, validate_review_log, verify_card_images, AttachmentKind,
    Card, CardTextFont, CardTextSpan, FormatError, ParsedBundle, DEVICE_DISTRIBUTION_DOCUMENTS,
    JAPANESE_FONT, MAX_BUNDLE_BYTES, MAX_REVIEW_LOG_BYTES,
};
use kobo_sdk::{
    action_id, ActionId, BandAlign, Chrome, Context, ControlState, FontHandle, Glyph, KoboApp,
    LayoutIssueKind, LogLevel, ParagraphAlignment, ParagraphPresentation, PictureHandle,
    RichTextSpan, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, SlotWidth,
    Space, StoreError, StoreResult, TextPresentation, TilePicture,
};
use std::collections::BTreeSet;
use std::process::ExitCode;

const BUNDLE_NAME: &str = "collection.cobfc";
const REVIEW_LOG_NAME: &str = "cobalt-review-log.ndjson";
const JAPANESE_FONT_HANDLE: FontHandle = FontHandle(1);
const PICTURE_HANDLE: PictureHandle = PictureHandle(1);
const NOTICE: &str = "Flashcards reviews host-prepared Cobalt bundles offline. The device contains no linked study-engine code, requests no remote-network capability, and uses only Cobalt's required local runtime IPC. Applicable device licences are in Licences & about.";
const CARD_PRESENTATION: ParagraphPresentation = ParagraphPresentation {
    alignment: ParagraphAlignment::Center,
    line_height_percent: 135,
    margin_before_em: 30,
    margin_after_em: 30,
    first_line_indent_em: 0,
};
const DECK_PRESENTATION: ParagraphPresentation = ParagraphPresentation {
    alignment: ParagraphAlignment::Center,
    line_height_percent: 110,
    margin_before_em: 0,
    margin_after_em: 20,
    first_line_indent_em: 0,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Loading,
    Decks,
    Review,
    Settings,
    Notices,
    NoticeDocument,
    Problem,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProblemKind {
    Missing,
    Corrupt,
    UnsafeMedia,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Grade {
    Again,
    Hard,
    Good,
    Easy,
}

impl Grade {
    const ALL: [Self; 4] = [Self::Again, Self::Hard, Self::Good, Self::Easy];

    const fn action(self) -> &'static str {
        match self {
            Self::Again => "again",
            Self::Hard => "hard",
            Self::Good => "good",
            Self::Easy => "easy",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Again => "Again",
            Self::Hard => "Hard",
            Self::Good => "Good",
            Self::Easy => "Easy",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StyledPage {
    text: String,
    spans: Vec<CardTextSpan>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NoticeDocument {
    title: &'static str,
    pages: Vec<Vec<String>>,
}

#[derive(Clone, Debug)]
struct ReviewScreenModel {
    deck: String,
    status: String,
    details: Option<String>,
    attachment_note: Option<String>,
    media_note: Option<String>,
    picture: Option<TilePicture>,
    answer: bool,
    saving: Option<Grade>,
    japanese_font: Option<FontHandle>,
    menu_open: bool,
    page: usize,
    pages: usize,
    completed: usize,
    total_cards: usize,
}

#[allow(
    clippy::struct_excessive_bools,
    reason = "independent retained UI and transfer states are not one state machine"
)]
struct Flashcards {
    view: View,
    return_view: View,
    problem: Option<(ProblemKind, String)>,
    menu_open: bool,
    library_download: Option<ShelfDownload>,
    review_download: Option<ShelfDownload>,
    review_upload: Option<ShelfUpload>,
    bundle: Option<ParsedBundle>,
    bundle_digest: String,
    selected_deck: Option<usize>,
    reviewed_cards: BTreeSet<i64>,
    answer: bool,
    saving: bool,
    pending_grade: Option<Grade>,
    pending_card_id: Option<i64>,
    pending_review: Option<String>,
    show_details: bool,
    deck_page: usize,
    deck_pages: Vec<Vec<usize>>,
    card_page: usize,
    card_pages: Vec<StyledPage>,
    notice_page: usize,
    notice_documents: Vec<NoticeDocument>,
    notice_document: Option<usize>,
    loading_received: u64,
    loading_total: Option<u64>,
    loading_bucket: Option<u8>,
    picture: Option<TilePicture>,
    media_message: Option<String>,
    japanese_font: Option<FontHandle>,
}

impl Default for Flashcards {
    fn default() -> Self {
        Self {
            view: View::Loading,
            return_view: View::Decks,
            problem: None,
            menu_open: false,
            library_download: None,
            review_download: None,
            review_upload: None,
            bundle: None,
            bundle_digest: String::new(),
            selected_deck: None,
            reviewed_cards: BTreeSet::new(),
            answer: false,
            saving: false,
            pending_grade: None,
            pending_card_id: None,
            pending_review: None,
            show_details: false,
            deck_page: 0,
            deck_pages: Vec::new(),
            card_page: 0,
            card_pages: Vec::new(),
            notice_page: 0,
            notice_documents: Vec::new(),
            notice_document: None,
            loading_received: 0,
            loading_total: None,
            loading_bucket: None,
            picture: None,
            media_message: None,
            japanese_font: None,
        }
    }
}

impl Flashcards {
    fn screen(&self) -> Screen {
        match self.view {
            View::Loading => loading_screen(self.loading_received, self.loading_total),
            View::Decks => self.deck_picker(),
            View::Review => self.review(),
            View::Settings => settings_screen(self.show_details),
            View::Notices => self.notice_index(),
            View::NoticeDocument => self.notice(),
            View::Problem => {
                let (kind, message) = self.problem.as_ref().map_or(
                    (
                        ProblemKind::Corrupt,
                        "The collection could not be opened.".to_owned(),
                    ),
                    |(kind, message)| (*kind, message.clone()),
                );
                problem_screen(kind, &message, self.menu_open)
            }
        }
    }

    fn active_card_ids(&self) -> &[i64] {
        let Some(bundle) = &self.bundle else {
            return &[];
        };
        self.selected_deck
            .and_then(|index| bundle.manifest().review_queue.decks.get(index))
            .map_or(
                bundle.manifest().review_queue.card_ids.as_slice(),
                |queue| queue.card_ids.as_slice(),
            )
    }

    fn current_card(&self) -> Option<&Card> {
        let bundle = self.bundle.as_ref()?;
        let card_id = self
            .active_card_ids()
            .iter()
            .find(|card_id| !self.reviewed_cards.contains(card_id))?;
        let index = bundle
            .manifest()
            .cards
            .binary_search_by_key(card_id, |card| card.id)
            .ok()?;
        bundle.manifest().cards.get(index)
    }

    fn reviewed_count(&self) -> usize {
        self.active_card_ids()
            .iter()
            .filter(|card_id| self.reviewed_cards.contains(card_id))
            .count()
    }

    fn selected_deck_name(&self) -> String {
        let Some(bundle) = &self.bundle else {
            return "Flashcards".to_owned();
        };
        self.selected_deck
            .and_then(|index| bundle.manifest().review_queue.decks.get(index))
            .and_then(|queue| {
                bundle
                    .manifest()
                    .decks
                    .iter()
                    .find(|deck| deck.id == queue.root_deck_id)
            })
            .map_or_else(
                || "All due cards".to_owned(),
                |deck| bounded_label(&deck.name, "Imported deck"),
            )
    }

    fn deck_picker(&self) -> Screen {
        let choices = self.deck_choices();
        let imported_total = self
            .bundle
            .as_ref()
            .map_or(0, |bundle| bundle.manifest().review_queue.card_ids.len());
        let total = self.bundle.as_ref().map_or(0, |bundle| {
            bundle
                .manifest()
                .review_queue
                .card_ids
                .iter()
                .filter(|card_id| !self.reviewed_cards.contains(card_id))
                .count()
        });
        let mut screen = ScreenBuilder::new("flashcards-decks").top_bar("Flashcards");
        screen = screen.top_bar_overflow(
            "more",
            self.menu_open,
            [
                ("settings", "Review settings"),
                ("notices", "Licences & about"),
            ],
        );
        if total == 0 {
            let message = if imported_total == 0 {
                "Nothing is due in this imported collection."
            } else {
                "All due cards in this collection were recorded locally."
            };
            return screen
                .empty_state(message)
                .bottom_action("retry", "Check collection again")
                .build();
        }
        let page_count = self.deck_pages.len().max(1);
        let page = self.deck_page.min(page_count - 1);
        let indices = self
            .deck_pages
            .get(page)
            .cloned()
            .unwrap_or_else(|| (0..choices.len()).collect());
        let rows = indices.into_iter().filter_map(|index| {
            let choice = choices.get(index)?;
            Some((
                choice.action.clone(),
                choice.title.clone(),
                choice.summary.clone(),
                Glyph::Book,
                choice.trailing.clone(),
            ))
        });
        screen = screen
            .section_with_value("Choose a deck", format!("{total} due"))
            .rows_with_trailing(rows);
        if choices
            .iter()
            .any(|choice| matches!(card_text_font(&choice.title), Ok(CardTextFont::Japanese)))
        {
            if let Some(font) = self.japanese_font {
                screen = screen.reading(true).reading_font(font);
            }
        }
        if page_count > 1 {
            screen = screen
                .page_turns("deck-page-prev", "deck-page-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(page_count).unwrap_or(u16::MAX),
                );
        }
        screen.build()
    }

    fn deck_choices(&self) -> Vec<DeckChoice> {
        let Some(bundle) = &self.bundle else {
            return Vec::new();
        };
        let queue = &bundle.manifest().review_queue;
        let mut choices = Vec::new();
        if queue.decks.len() > 1 {
            let remaining = queue
                .card_ids
                .iter()
                .filter(|card_id| !self.reviewed_cards.contains(card_id))
                .count();
            choices.push(DeckChoice {
                action: "deck-all".to_owned(),
                title: "All due cards".to_owned(),
                summary: format!("Across {} decks · imported due order", queue.decks.len()),
                trailing: due_label(remaining),
            });
        }
        for (index, deck_queue) in queue.decks.iter().enumerate() {
            let name = bundle
                .manifest()
                .decks
                .iter()
                .find(|deck| deck.id == deck_queue.root_deck_id)
                .map_or_else(|| "Imported deck".to_owned(), |deck| deck.name.clone());
            let remaining = deck_queue
                .card_ids
                .iter()
                .filter(|card_id| !self.reviewed_cards.contains(card_id))
                .count();
            choices.push(DeckChoice {
                action: format!("deck-{index}"),
                title: bounded_label(&name, "Imported deck"),
                summary: "Imported due order".to_owned(),
                trailing: due_label(remaining),
            });
        }
        choices
    }

    fn review(&self) -> Screen {
        let total = self.active_card_ids().len();
        let completed = self.reviewed_count();
        let Some(card) = self.current_card() else {
            let deck = self.selected_deck_name();
            let japanese_font = matches!(card_text_font(&deck), Ok(CardTextFont::Japanese))
                .then_some(self.japanese_font)
                .flatten();
            return done_screen(&deck, completed, total, self.menu_open, japanese_font);
        };
        let page_count = self.card_pages.len().max(1);
        let page = self.card_page.min(page_count - 1);
        let fallback;
        let shown = if let Some(page) = self.card_pages.get(page) {
            page
        } else {
            fallback = StyledPage {
                text: side_text(card, self.answer).to_owned(),
                spans: side_spans(card, self.answer).to_vec(),
            };
            &fallback
        };
        let phase = if let Some(grade) = self.pending_grade.filter(|_| self.saving) {
            format!("Saving {}", grade.label())
        } else if self.answer {
            "Answer".to_owned()
        } else {
            "Question".to_owned()
        };
        let details = self.show_details.then(|| {
            let template = if matches!(
                card_text_font(&card.template_name),
                Ok(CardTextFont::Interface)
            ) {
                bounded_label(&card.template_name, "Imported template")
            } else {
                "Imported template".to_owned()
            };
            format!(
                "{} · {} prior review{}",
                template,
                card.repetitions,
                if card.repetitions == 1 { "" } else { "s" }
            )
        });
        let deck = self.selected_deck_name();
        let japanese_font = (matches!(
            card_text_font(side_text(card, self.answer)),
            Ok(CardTextFont::Japanese)
        ) || matches!(card_text_font(&deck), Ok(CardTextFont::Japanese)))
        .then_some(self.japanese_font)
        .flatten();
        review_screen(
            &ReviewScreenModel {
                deck,
                status: format!("{phase} · {} of {total}", completed + 1),
                details,
                attachment_note: side_attachment_summary(card, self.answer),
                media_note: self.media_message.clone(),
                picture: self.picture,
                answer: self.answer,
                saving: self.pending_grade.filter(|_| self.saving),
                japanese_font,
                menu_open: self.menu_open,
                page,
                pages: page_count,
                completed,
                total_cards: total,
            },
            &shown.text,
            &shown.spans,
        )
    }

    fn notice_index(&self) -> Screen {
        let rows = self
            .notice_documents
            .iter()
            .enumerate()
            .map(|(index, document)| {
                (
                    format!("notice-document-{index}"),
                    document.title,
                    "Included with this device application",
                    Glyph::Note,
                    format!(
                        "{} page{}",
                        document.pages.len(),
                        if document.pages.len() == 1 { "" } else { "s" }
                    ),
                )
            });
        ScreenBuilder::new("flashcards-notice-index")
            .top_bar("Licences & about")
            .owns_back(true)
            .section("On this device")
            .rows_with_trailing(rows)
            .bottom_action("screen-back", "Done")
            .build()
    }

    fn notice(&self) -> Screen {
        let fallback = NoticeDocument {
            title: "Flashcards",
            pages: vec![vec![NOTICE.to_owned()]],
        };
        let document = self
            .notice_document
            .and_then(|index| self.notice_documents.get(index))
            .unwrap_or(&fallback);
        let total = document.pages.len().max(1);
        let page = self.notice_page.min(total - 1);
        let paragraphs = document
            .pages
            .get(page)
            .map_or(&[] as &[String], Vec::as_slice);
        let mut screen = ScreenBuilder::new("flashcards-notices")
            .top_bar(document.title)
            .owns_back(true);
        for paragraph in paragraphs {
            screen = screen.text(paragraph);
        }
        screen = screen.bottom_action("notice-index", "Licences");
        if total > 1 {
            screen = screen
                .page_turns("notice-prev", "notice-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(total).unwrap_or(u16::MAX),
                );
        }
        screen.build()
    }

    fn start_download(&mut self, context: &mut Context) {
        let mut download = ShelfDownload::new(BUNDLE_NAME)
            .at_most(usize::try_from(MAX_BUNDLE_BYTES).expect("bundle bound fits device usize"));
        download.start(context);
        self.library_download = Some(download);
        self.view = View::Loading;
        self.problem = None;
        self.loading_received = 0;
        self.loading_total = None;
        self.loading_bucket = None;
        context.set_screen(self.screen());
    }

    fn finish_download(&mut self, context: &mut Context) {
        let Some(download) = self.library_download.take() else {
            return;
        };
        let bytes = download.take();
        match decode(&bytes) {
            Ok(bundle) => {
                if verify_card_images(&bundle, &bundle.manifest().review_queue.card_ids).is_err() {
                    self.set_problem(
                        context,
                        ProblemKind::UnsafeMedia,
                        "A card image is missing, damaged, or outside the safe display rules.",
                    );
                    return;
                }
                let needs_japanese =
                    bundle.manifest().decks.iter().any(|deck| {
                        matches!(card_text_font(&deck.name), Ok(CardTextFont::Japanese))
                    }) || bundle
                        .manifest()
                        .review_queue
                        .card_ids
                        .iter()
                        .any(|card_id| {
                            bundle
                                .manifest()
                                .cards
                                .binary_search_by_key(card_id, |card| card.id)
                                .ok()
                                .and_then(|index| bundle.manifest().cards.get(index))
                                .is_some_and(|card| {
                                    matches!(
                                        card_text_font(&card.front),
                                        Ok(CardTextFont::Japanese)
                                    ) || matches!(
                                        card_text_font(&card.back),
                                        Ok(CardTextFont::Japanese)
                                    ) || matches!(
                                        card_text_font(&card.template_name),
                                        Ok(CardTextFont::Japanese)
                                    )
                                })
                        });
                if needs_japanese && self.japanese_font.is_none() {
                    self.japanese_font = context.put_font(
                        JAPANESE_FONT_HANDLE,
                        "CobaltJapanese-Regular.otf",
                        JAPANESE_FONT.to_vec(),
                    );
                }
                if needs_japanese && self.japanese_font.is_none() {
                    self.set_problem(
                        context,
                        ProblemKind::Corrupt,
                        "The bundled Japanese text face could not be loaded on this Kobo.",
                    );
                    return;
                }
                self.bundle_digest = digest_hex(&bytes);
                self.bundle = Some(bundle);
                self.selected_deck = None;
                self.reviewed_cards.clear();
                self.answer = false;
                self.saving = false;
                self.pending_grade = None;
                self.pending_card_id = None;
                self.pending_review = None;
                self.picture = None;
                self.media_message = None;
                self.view = View::Decks;
                self.prepare_deck_pages(context);
                context.set_screen(self.screen());
            }
            Err(error) => {
                self.set_problem(
                    context,
                    ProblemKind::Corrupt,
                    &bundle_rejection_message(&error),
                );
            }
        }
    }

    fn set_problem(&mut self, context: &mut Context, kind: ProblemKind, message: &str) {
        self.view = View::Problem;
        self.problem = Some((kind, message.to_owned()));
        self.menu_open = false;
        context.set_screen(self.screen());
    }

    fn prepare_deck_pages(&mut self, _context: &Context) {
        let choices = self.deck_choices();
        self.deck_pages = (0..choices.len())
            .collect::<Vec<_>>()
            .chunks(3)
            .map(<[_]>::to_vec)
            .collect();
        self.deck_page = self.deck_page.min(self.deck_pages.len().saturating_sub(1));
    }

    fn select_deck(&mut self, context: &mut Context, selected: Option<usize>) {
        self.selected_deck = selected;
        self.view = View::Review;
        self.answer = false;
        self.saving = false;
        self.pending_grade = None;
        self.pending_card_id = None;
        self.pending_review = None;
        self.menu_open = false;
        self.prepare_current_card(context);
        context.set_screen(self.screen());
    }

    fn prepare_current_card(&mut self, context: &mut Context) {
        self.card_page = 0;
        self.load_picture(context);
        self.repaginate_card(context);
    }

    fn load_picture(&mut self, context: &mut Context) {
        self.picture = None;
        self.media_message = None;
        let Some(bundle) = &self.bundle else {
            return;
        };
        let Some(card) = self.current_card() else {
            return;
        };
        let Some(image) = image_for_side(card, self.answer) else {
            return;
        };
        let image_name = image.rendered_name.as_deref().unwrap_or(&image.name);
        let Some(bytes) = bundle.media(image_name) else {
            self.media_message = Some("Image unavailable on this Kobo.".to_owned());
            return;
        };
        let Ok(decoded) = kobo_image::decode(bytes) else {
            self.media_message = Some("Image could not be displayed safely.".to_owned());
            return;
        };
        let Ok(picture) = decoded.fit(900, 480) else {
            self.media_message = Some("Image could not fit the review area.".to_owned());
            return;
        };
        self.picture = context.put_picture(
            PICTURE_HANDLE,
            picture.width(),
            picture.height(),
            picture.into_grey(),
        );
        if self.picture.is_none() {
            self.media_message = Some("Image memory is unavailable right now.".to_owned());
        }
    }

    fn repaginate_card(&mut self, context: &Context) {
        let Some(card) = self.current_card() else {
            self.card_pages.clear();
            self.card_page = 0;
            return;
        };
        let text = side_text(card, self.answer).to_owned();
        let spans = side_spans(card, self.answer).to_vec();
        let total = self.active_card_ids().len();
        let completed = self.reviewed_count();
        let details = self.show_details.then(|| {
            let template = if matches!(
                card_text_font(&card.template_name),
                Ok(CardTextFont::Interface)
            ) {
                bounded_label(&card.template_name, "Imported template")
            } else {
                "Imported template".to_owned()
            };
            format!(
                "{} · {} prior review{}",
                template,
                card.repetitions,
                if card.repetitions == 1 { "" } else { "s" }
            )
        });
        let deck = self.selected_deck_name();
        let japanese_font = (matches!(card_text_font(&text), Ok(CardTextFont::Japanese))
            || matches!(card_text_font(&deck), Ok(CardTextFont::Japanese)))
        .then_some(self.japanese_font)
        .flatten();
        let model = ReviewScreenModel {
            deck,
            status: format!(
                "{} · {} of {total}",
                if self.answer { "Answer" } else { "Question" },
                completed + 1
            ),
            details,
            attachment_note: side_attachment_summary(card, self.answer),
            media_note: self.media_message.clone(),
            picture: self.picture,
            answer: self.answer,
            saving: None,
            japanese_font,
            menu_open: false,
            page: 0,
            pages: 2,
            completed,
            total_cards: total,
        };
        self.card_pages = paginate_review_text(&model, &text, &spans, context.metrics());
        self.card_page = self.card_page.min(self.card_pages.len().saturating_sub(1));
    }

    fn reveal(&mut self, context: &mut Context) {
        if self.saving || self.current_card().is_none() {
            return;
        }
        self.answer = true;
        self.prepare_current_card(context);
        context.set_screen(self.screen());
    }

    fn record_review(&mut self, context: &mut Context, grade: Grade) {
        if self.saving || self.bundle.is_none() || self.bundle_digest.len() != 64 {
            return;
        }
        let Some(card) = self.current_card() else {
            return;
        };
        let card_id = card.id;
        let due = card.due;
        let repetitions = card.repetitions;
        let record = format!(
            "{{\"format\":2,\"bundle_sha256\":\"{}\",\"card_id\":{},\"grade\":\"{}\",\"imported_due\":{},\"imported_reps\":{}}}",
            self.bundle_digest,
            card_id,
            grade.action(),
            due,
            repetitions
        );
        self.saving = true;
        self.pending_grade = Some(grade);
        self.pending_card_id = Some(card_id);
        self.pending_review = Some(record);
        let mut download = ShelfDownload::new(REVIEW_LOG_NAME).at_most(MAX_REVIEW_LOG_BYTES);
        download.start(context);
        self.review_download = Some(download);
        context.set_screen(self.screen());
    }

    fn upload_review_log(&mut self, context: &mut Context, mut log: Vec<u8>) {
        let Some(record) = self.pending_review.as_ref() else {
            return;
        };
        if validate_review_log(&log).is_err() {
            self.review_save_failed(
                context,
                "Review not saved. Export or repair the local review log, then try again.",
            );
            return;
        }
        let required = log.len().saturating_add(record.len()).saturating_add(1);
        if required > MAX_REVIEW_LOG_BYTES {
            self.review_save_failed(
                context,
                "Review log is full. Export it before adding more reviews.",
            );
            return;
        }
        log.extend_from_slice(record.as_bytes());
        log.push(b'\n');
        let mut upload = ShelfUpload::new(REVIEW_LOG_NAME, log);
        upload.start(context);
        self.review_upload = Some(upload);
    }

    fn review_save_failed(&mut self, context: &mut Context, message: &str) {
        self.saving = false;
        self.pending_grade = None;
        self.pending_card_id = None;
        self.pending_review = None;
        self.media_message = Some(message.to_owned());
        context.set_screen(self.screen());
    }

    fn commit_review(&mut self, context: &mut Context) {
        if let Some(card_id) = self.pending_card_id.take() {
            self.reviewed_cards.insert(card_id);
        }
        self.answer = false;
        self.saving = false;
        self.pending_grade = None;
        self.pending_review = None;
        self.media_message = None;
        self.prepare_deck_pages(context);
        self.prepare_current_card(context);
        context.set_screen(self.screen());
    }

    fn open_view(&mut self, context: &mut Context, view: View) {
        self.return_view = self.view;
        self.view = view;
        self.menu_open = false;
        context.set_screen(self.screen());
    }

    fn close_supporting_screen(&mut self, context: &mut Context) {
        self.view = self.return_view;
        self.menu_open = false;
        if self.view == View::Review {
            self.repaginate_card(context);
        }
        context.set_screen(self.screen());
    }

    fn move_page(&mut self, context: &mut Context, forward: bool) {
        match self.view {
            View::Decks => {
                let last = self.deck_pages.len().saturating_sub(1);
                self.deck_page = if forward {
                    self.deck_page.saturating_add(1).min(last)
                } else {
                    self.deck_page.saturating_sub(1)
                };
            }
            View::Review => {
                let last = self.card_pages.len().saturating_sub(1);
                self.card_page = if forward {
                    self.card_page.saturating_add(1).min(last)
                } else {
                    self.card_page.saturating_sub(1)
                };
            }
            View::NoticeDocument => {
                let last = self
                    .notice_document
                    .and_then(|index| self.notice_documents.get(index))
                    .map_or(0, |document| document.pages.len().saturating_sub(1));
                self.notice_page = if forward {
                    self.notice_page.saturating_add(1).min(last)
                } else {
                    self.notice_page.saturating_sub(1)
                };
            }
            _ => return,
        }
        context.set_screen(self.screen());
    }
}

impl KoboApp for Flashcards {
    fn on_start(&mut self, context: &mut Context) {
        context.log(LogLevel::Info, NOTICE);
        self.notice_documents = build_notice_documents(context);
        self.start_download(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("more") {
            self.menu_open = true;
            context.set_screen(self.screen());
            return;
        }
        if action == ActionId::BACK {
            if self.menu_open {
                self.menu_open = false;
                context.set_screen(self.screen());
            } else if self.view == View::NoticeDocument {
                self.view = View::Notices;
                self.notice_document = None;
                self.notice_page = 0;
                context.set_screen(self.screen());
            } else if matches!(self.view, View::Settings | View::Notices) {
                self.close_supporting_screen(context);
            } else if self.view == View::Review {
                self.view = View::Decks;
                self.answer = false;
                self.card_page = 0;
                context.set_screen(self.screen());
            }
            return;
        }
        self.menu_open = false;
        if action == action_id("settings") {
            self.open_view(context, View::Settings);
        } else if action == action_id("notices") {
            self.notice_page = 0;
            self.notice_document = None;
            self.open_view(context, View::Notices);
        } else if action == action_id("notice-index") {
            self.view = View::Notices;
            self.notice_document = None;
            self.notice_page = 0;
            context.set_screen(self.screen());
        } else if action == action_id("screen-back") {
            self.close_supporting_screen(context);
        } else if action == action_id("retry") {
            self.start_download(context);
        } else if action == action_id("choose-deck") || action == action_id("back-decks") {
            self.view = View::Decks;
            self.answer = false;
            self.prepare_deck_pages(context);
            context.set_screen(self.screen());
        } else if action == action_id("deck-all") {
            self.select_deck(context, None);
        } else if let Some(index) = self.bundle.as_ref().and_then(|bundle| {
            (0..bundle.manifest().review_queue.decks.len())
                .find(|index| action == action_id(&format!("deck-{index}")))
        }) {
            self.select_deck(context, Some(index));
        } else if action == action_id("deck-page-prev") {
            self.move_page(context, false);
        } else if action == action_id("deck-page-next") {
            self.move_page(context, true);
        } else if action == action_id("card-page-prev") {
            self.move_page(context, false);
        } else if action == action_id("card-page-next") {
            self.move_page(context, true);
        } else if action == action_id("notice-prev") {
            self.move_page(context, false);
        } else if action == action_id("notice-next") {
            self.move_page(context, true);
        } else if let Some(index) = (0..self.notice_documents.len())
            .find(|index| action == action_id(&format!("notice-document-{index}")))
        {
            self.notice_document = Some(index);
            self.notice_page = 0;
            self.view = View::NoticeDocument;
            context.set_screen(self.screen());
        } else if action == action_id("details-compact") {
            self.show_details = false;
            context.set_screen(self.screen());
        } else if action == action_id("details-detailed") {
            self.show_details = true;
            context.set_screen(self.screen());
        } else if action == action_id("answer") {
            self.reveal(context);
        } else if let Some(grade) = Grade::ALL
            .into_iter()
            .find(|grade| action == action_id(grade.action()))
        {
            self.record_review(context, grade);
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        self.move_page(context, forward);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let Some(download) = &mut self.library_download {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    self.finish_download(context);
                    return;
                }
                ShelfProgress::Moving { done, total } => {
                    let percent = (total > 0).then(|| {
                        u8::try_from(done.saturating_mul(100) / total)
                            .unwrap_or(100)
                            .min(100)
                    });
                    let bucket = percent.map(|percent| percent / 10 * 10);
                    self.loading_received = u64::from(done);
                    self.loading_total = (total > 0).then_some(u64::from(total));
                    if bucket != self.loading_bucket {
                        self.loading_bucket = bucket;
                        context.set_screen(self.screen());
                    }
                    return;
                }
                ShelfProgress::Failed(StoreError::Missing) => {
                    self.library_download = None;
                    self.set_problem(
                        context,
                        ProblemKind::Missing,
                        "No prepared collection is on this Kobo. Stage one with flashcards-import.",
                    );
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.library_download = None;
                    self.set_problem(
                        context,
                        ProblemKind::Corrupt,
                        "The collection could not be read. Check the staged bundle and try again.",
                    );
                    return;
                }
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(download) = &mut self.review_download {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let log = self
                        .review_download
                        .take()
                        .expect("active review download")
                        .take();
                    self.upload_review_log(context, log);
                    return;
                }
                ShelfProgress::Moving { .. } => return,
                ShelfProgress::Failed(StoreError::Missing) => {
                    self.review_download = None;
                    self.upload_review_log(context, Vec::new());
                    return;
                }
                ShelfProgress::Failed(_) => {
                    self.review_download = None;
                    self.review_save_failed(
                        context,
                        "Review not saved. The imported schedule was left unchanged.",
                    );
                    return;
                }
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(upload) = &mut self.review_upload {
            match upload.advance(context, &result) {
                ShelfProgress::Done => {
                    self.review_upload = None;
                    self.commit_review(context);
                }
                ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
                ShelfProgress::Failed(_) => {
                    self.review_upload = None;
                    self.review_save_failed(
                        context,
                        "Review not saved. The imported schedule was left unchanged.",
                    );
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct DeckChoice {
    action: String,
    title: String,
    summary: String,
    trailing: String,
}

fn loading_screen(received: u64, total: Option<u64>) -> Screen {
    let screen = ScreenBuilder::new("flashcards-loading").top_bar("Flashcards");
    if received == 0 && total.is_none() {
        screen
            .splash(
                Some(Glyph::Note),
                "Opening collection",
                "Checking the prepared offline bundle.",
            )
            .build()
    } else {
        screen
            .section("Opening collection")
            .skeleton(5)
            .transfer("Reading verified bundle", received, total)
            .build()
    }
}

fn problem_screen(kind: ProblemKind, message: &str, menu_open: bool) -> Screen {
    let title = match kind {
        ProblemKind::Missing => "Collection not found",
        ProblemKind::Corrupt => "Collection rejected",
        ProblemKind::UnsafeMedia => "Media rejected",
    };
    ScreenBuilder::new("flashcards-problem")
        .top_bar("Flashcards")
        .top_bar_overflow("more", menu_open, [("notices", "Licences & about")])
        .error_state(format!("{title}\n\n{message}"))
        .bottom_action("retry", "Read collection again")
        .build()
}

fn settings_screen(detailed: bool) -> Screen {
    ScreenBuilder::new("flashcards-settings")
        .top_bar("Review settings")
        .owns_back(true)
        .section("Card details")
        .choose(
            "Information shown during review",
            [
                ("details-compact", "Compact"),
                ("details-detailed", "Detailed"),
            ],
        )
        .chosen(usize::from(detailed))
        .section("Review behavior")
        .facts([
            ("Order", "Imported due queue"),
            ("Text size", "Reader setting"),
            ("Intervals", "Not recalculated on Kobo"),
            ("Media", "Fit without stretching"),
        ])
        .bottom_action("screen-back", "Done")
        .build()
}

fn done_screen(
    deck: &str,
    completed: usize,
    total: usize,
    menu_open: bool,
    japanese_font: Option<FontHandle>,
) -> Screen {
    let mut screen = ScreenBuilder::new("flashcards-done")
        .top_bar("Flashcards")
        .top_bar_overflow(
            "more",
            menu_open,
            [
                ("settings", "Review settings"),
                ("notices", "Licences & about"),
            ],
        )
        .owns_back(true)
        .rich_text(deck, Vec::new(), DECK_PRESENTATION)
        .splash(
            Some(Glyph::Check),
            "Review complete",
            format!("{completed} of {total} due cards recorded locally."),
        )
        .bottom_action("back-decks", "Choose another deck");
    if let Some(font) = japanese_font {
        screen = screen.reading(true).reading_font(font);
    }
    screen.build()
}

fn review_screen(model: &ReviewScreenModel, text: &str, spans: &[CardTextSpan]) -> Screen {
    let total = model.pages.max(1);
    let page = model.page.min(total - 1);
    let progress =
        u8::try_from(progress_percent(model.completed, model.total_cards)).unwrap_or(100);
    let mut screen = ScreenBuilder::new("flashcards-review")
        .top_bar("Flashcards")
        .top_bar_overflow(
            "more",
            model.menu_open,
            [
                ("choose-deck", "Decks"),
                ("settings", "Review settings"),
                ("notices", "Licences & about"),
            ],
        )
        .owns_back(true)
        .rich_text(model.deck.clone(), Vec::new(), DECK_PRESENTATION)
        .secondary(model.status.clone())
        .progress(progress);
    if let Some(details) = &model.details {
        screen = screen.secondary(details);
    }
    if let Some(note) = &model.attachment_note {
        screen = screen.secondary(note);
    }
    if let Some(note) = &model.media_note {
        screen = screen.secondary(note);
    }
    if let Some(picture) = model.picture {
        screen = screen.picture(picture, 38);
    }
    let display_text = if text.trim().is_empty() && model.picture.is_none() {
        "No text was rendered for this side."
    } else {
        text
    };
    screen = screen
        .rich_text(display_text, ui_spans(spans), CARD_PRESENTATION)
        .spacer(Space::Small)
        .fill();
    screen = if model.answer {
        rating_controls(screen, model.saving)
    } else if model.saving.is_some() {
        screen.disabled_button("answer", "Reveal answer")
    } else {
        screen.primary_button("answer", "Reveal answer")
    };
    if let Some(font) = model.japanese_font {
        screen = screen.reading(true).reading_font(font);
    }
    if total > 1 {
        screen = screen
            .page_turns("card-page-prev", "card-page-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(total).unwrap_or(u16::MAX),
            );
    }
    screen.build()
}

fn rating_controls(screen: ScreenBuilder, saving: Option<Grade>) -> ScreenBuilder {
    rating_pair(
        rating_pair(screen, Grade::Again, Grade::Hard, saving),
        Grade::Good,
        Grade::Easy,
        saving,
    )
}

fn rating_pair(
    screen: ScreenBuilder,
    first: Grade,
    second: Grade,
    saving: Option<Grade>,
) -> ScreenBuilder {
    type Build = Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>;
    let first_builder: Build = Box::new(move |builder| rating_button(builder, first, saving));
    let second_builder: Build = Box::new(move |builder| rating_button(builder, second, saving));
    screen.band(
        BandAlign::Middle,
        [
            (SlotWidth::Fill, first_builder),
            (SlotWidth::Fill, second_builder),
        ],
    )
}

fn rating_button(screen: ScreenBuilder, grade: Grade, saving: Option<Grade>) -> ScreenBuilder {
    if saving.is_some() {
        screen.button_with_state(grade.action(), grade.label(), ControlState::Disabled)
    } else if grade == Grade::Good {
        screen.primary_button(grade.action(), grade.label())
    } else {
        screen.button(grade.action(), grade.label())
    }
}

fn ui_spans(spans: &[CardTextSpan]) -> Vec<RichTextSpan> {
    spans
        .iter()
        .map(|span| RichTextSpan {
            start: span.start,
            end: span.end,
            presentation: TextPresentation {
                strong: span.style.strong,
                emphasis: span.style.emphasis,
                underline: span.style.underline,
                superscript: span.style.superscript,
                subscript: span.style.subscript,
                highlighted: false,
            },
        })
        .collect()
}

fn paginate_review_text(
    model: &ReviewScreenModel,
    text: &str,
    spans: &[CardTextSpan],
    metrics: kobo_sdk::DisplayMetrics,
) -> Vec<StyledPage> {
    if text.is_empty() {
        return vec![StyledPage::default()];
    }
    let boundaries = text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .collect::<Vec<_>>();
    let mut pages = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let start_index = boundaries
            .binary_search(&start)
            .expect("page start is a character boundary");
        let mut low = start_index + 1;
        let mut high = boundaries.len() - 1;
        let mut best = boundaries[low.min(high)];
        while low <= high {
            let middle = low + (high - low) / 2;
            let end = boundaries[middle];
            let page = styled_page(text, spans, start, end);
            if review_page_fits(model, &page, metrics) {
                best = end;
                low = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                high = middle - 1;
            }
        }
        if best < text.len() {
            let segment = &text[start..best];
            if let Some((offset, character)) =
                segment.char_indices().rev().find(|(offset, character)| {
                    *offset >= segment.len() / 2 && character.is_whitespace()
                })
            {
                best = start + offset + character.len_utf8();
            }
            while best > start
                && text[best..]
                    .chars()
                    .next()
                    .is_some_and(forbidden_page_start)
            {
                let Some((previous, _)) = text[start..best].char_indices().next_back() else {
                    break;
                };
                best = start + previous;
            }
        }
        if best <= start {
            best = text[start..]
                .chars()
                .next()
                .map_or(text.len(), |character| start + character.len_utf8());
        }

        pages.push(styled_page(text, spans, start, best));
        start = best;
        while start < text.len() {
            let character = text[start..].chars().next().expect("start in bounds");
            if !character.is_whitespace() {
                break;
            }
            start += character.len_utf8();
        }
    }
    pages
}

fn forbidden_page_start(character: char) -> bool {
    matches!(
        character,
        '、' | '。'
            | '，'
            | '．'
            | '）'
            | '］'
            | '｝'
            | '〉'
            | '》'
            | '」'
            | '』'
            | '】'
            | '〕'
            | '〗'
            | '〙'
            | '〛'
            | '！'
            | '？'
            | '：'
            | '；'
            | 'ー'
    )
}

fn review_page_fits(
    model: &ReviewScreenModel,
    page: &StyledPage,
    metrics: kobo_sdk::DisplayMetrics,
) -> bool {
    let mut measured = model.clone();
    measured.page = 0;
    measured.pages = 2;
    let diagnostics = review_screen(&measured, &page.text, &page.spans)
        .diagnostics(&metrics, &Chrome::measuring(true));
    !diagnostics.issues.iter().any(|issue| {
        matches!(
            issue.kind,
            LayoutIssueKind::ContentOverflow { .. }
                | LayoutIssueKind::Clipped
                | LayoutIssueKind::TextOverflow
                | LayoutIssueKind::TouchTargetTooSmall { .. }
        )
    })
}

fn styled_page(text: &str, spans: &[CardTextSpan], start: usize, end: usize) -> StyledPage {
    let spans = spans
        .iter()
        .filter_map(|span| {
            let from = span.start.max(start);
            let to = span.end.min(end);
            (from < to).then_some(CardTextSpan {
                start: from - start,
                end: to - start,
                style: span.style,
            })
        })
        .collect();
    StyledPage {
        text: text[start..end].to_owned(),
        spans,
    }
}

fn build_notice_documents(context: &Context) -> Vec<NoticeDocument> {
    let mut documents = Vec::new();
    for (title, text) in DEVICE_DISTRIBUTION_DOCUMENTS {
        let readable = readable_notice(text);
        let mut pages = context.paginate(&readable, true);
        if pages.is_empty() {
            pages.push(vec![readable]);
        }
        documents.push(NoticeDocument { title, pages });
    }
    if documents.is_empty() {
        documents.push(NoticeDocument {
            title: "Flashcards",
            pages: vec![vec![NOTICE.to_owned()]],
        });
    }
    documents
}

fn readable_notice(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut in_code = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let line = if in_code {
            trimmed
        } else {
            trimmed.trim_start_matches('#').trim_start()
        };
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&line.replace('`', "").replace("**", ""));
    }
    output
}

fn bundle_rejection_message(error: &FormatError) -> String {
    if *error == FormatError::UnsupportedVersion(3) {
        "This collection uses the retired bundle format. Recreate it with the current host converter; the separate local review log will be preserved."
            .to_owned()
    } else {
        "The staged collection is corrupt, unsupported, or incomplete. Recreate it with the current host converter."
            .to_owned()
    }
}

fn side_attachment_summary(card: &Card, answer: bool) -> Option<String> {
    let visible = if answer {
        &card.answer_media_names
    } else {
        &card.question_media_names
    };
    let audio = card
        .attachments
        .iter()
        .filter(|attachment| {
            attachment.kind == AttachmentKind::Audio && visible.contains(&attachment.name)
        })
        .count();
    let video = card
        .attachments
        .iter()
        .filter(|attachment| {
            attachment.kind == AttachmentKind::Video && visible.contains(&attachment.name)
        })
        .count();
    match (audio, video) {
        (0, 0) => None,
        (audio, 0) => Some(format!(
            "{audio} audio attachment{} retained · playback unavailable",
            if audio == 1 { "" } else { "s" }
        )),
        (0, video) => Some(format!(
            "{video} video attachment{} retained · playback unavailable",
            if video == 1 { "" } else { "s" }
        )),
        _ => Some(format!(
            "{audio} audio · {video} video attachments retained · playback unavailable"
        )),
    }
}

fn image_for_side(card: &Card, answer: bool) -> Option<&kobo_flashcards_format::Attachment> {
    if answer {
        card.attachments
            .iter()
            .find(|attachment| {
                attachment.kind == AttachmentKind::Image
                    && card.answer_media_names.contains(&attachment.name)
                    && !card.question_media_names.contains(&attachment.name)
            })
            .or_else(|| {
                card.attachments.iter().find(|attachment| {
                    attachment.kind == AttachmentKind::Image
                        && card.answer_media_names.contains(&attachment.name)
                })
            })
    } else {
        card.attachments.iter().find(|attachment| {
            attachment.kind == AttachmentKind::Image
                && card.question_media_names.contains(&attachment.name)
        })
    }
}

fn side_text(card: &Card, answer: bool) -> &str {
    if answer {
        &card.back
    } else {
        &card.front
    }
}

fn side_spans(card: &Card, answer: bool) -> &[CardTextSpan] {
    if answer {
        &card.back_spans
    } else {
        &card.front_spans
    }
}

fn bounded_label(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return fallback.to_owned();
    }
    let mut output = value.chars().take(64).collect::<String>();
    if value.chars().count() > 64 {
        output.push('\u{2026}');
    }
    output
}

fn due_label(count: usize) -> String {
    if count == 0 {
        "Done".to_owned()
    } else {
        format!("{count} due")
    }
}

fn progress_percent(done: usize, total: usize) -> usize {
    done.saturating_mul(100).checked_div(total).unwrap_or(100)
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
    use kobo_flashcards_format::{
        encode, rasterize_svg, Attachment, BundleManifest, Deck, DeckConfiguration, DeckQueue,
        Note, NoteType, ReviewQueue, Source, CONVERTER_REVISION,
    };
    use kobo_ui::{render_all, LayoutKind, PictureCache, Surface, TextScale, CLARA_BW_METRICS};
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Once;

    fn install_fonts() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            kobo_text::install(CLARA_BW_METRICS).expect("Cobalt text face");
            let font = kobo_text::BookFont::from_bytes(
                JAPANESE_FONT,
                "CobaltJapanese-Regular.otf",
                CLARA_BW_METRICS,
            )
            .expect("Japanese face");
            kobo_ui::put_book_typesetter(JAPANESE_FONT_HANDLE, Box::new(font));
        });
    }

    fn source(card_count: usize) -> Source {
        Source {
            package_kind: "apkg".to_owned(),
            collection_member: "collection.anki2".to_owned(),
            collection_schema: 18,
            normalized_schema: 18,
            collection_id: 1,
            collection_created: 0,
            collection_modified: 1,
            schema_modified: 1,
            dirty: 0,
            user_sequence: 0,
            last_sync: 0,
            note_count: card_count,
            card_count,
            converter_revision: CONVERTER_REVISION.to_owned(),
            original_config_json: "{}".to_owned(),
            original_models_json: "{}".to_owned(),
            original_decks_json: "{}".to_owned(),
            original_deck_configurations_json: "{}".to_owned(),
            original_tags_json: "{}".to_owned(),
            normalized_config: Vec::new(),
            normalized_tags: Vec::new(),
        }
    }

    fn fixture_card(id: i64, deck_id: i64) -> Card {
        Card {
            id,
            note_id: id,
            deck_id,
            ordinal: 0,
            user_sequence: 0,
            queue: 2,
            card_type: 2,
            due: 1,
            interval: 4,
            ease_factor: 2500,
            repetitions: 2,
            lapses: 0,
            remaining_steps: 0,
            original_due: 0,
            original_deck_id: 0,
            flags: 0,
            data: String::new(),
            modified: 1,
            template_name: "Recognition".to_owned(),
            front: "Question".to_owned(),
            back: "Answer".to_owned(),
            front_spans: Vec::new(),
            back_spans: Vec::new(),
            tags: Vec::new(),
            question_media_names: Vec::new(),
            answer_media_names: Vec::new(),
            media_names: Vec::new(),
            attachments: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn fixture_bundle(with_cards: bool) -> ParsedBundle {
        let count = usize::from(with_cards) * 2;
        let mut manifest = BundleManifest::empty(source(count));
        manifest.notetypes.push(NoteType {
            id: 1,
            name: "Basic".to_owned(),
            original_json: "{}".to_owned(),
        });
        manifest.deck_configurations.push(DeckConfiguration {
            id: 1,
            name: "Default".to_owned(),
            original_json: "{}".to_owned(),
        });
        for (id, name) in [(1, "日本語"), (2, "Sentences")] {
            manifest.decks.push(Deck {
                id,
                name: name.to_owned(),
                configuration_id: Some(1),
                original_json: "{}".to_owned(),
            });
        }
        if with_cards {
            for id in 1..=2 {
                manifest.notes.push(Note {
                    id,
                    guid: format!("guid-{id}"),
                    notetype_id: 1,
                    modified: 1,
                    user_sequence: 0,
                    tags: Vec::new(),
                    fields: vec![format!("field-{id}")],
                    sort_field: format!("field-{id}"),
                    checksum: id,
                    flags: 0,
                    data: String::new(),
                });
                manifest.cards.push(fixture_card(id, id));
            }
            manifest.review_queue = ReviewQueue {
                card_ids: vec![1, 2],
                new_count: 1,
                learning_count: 0,
                review_count: 1,
                decks: vec![
                    DeckQueue {
                        source_index: 0,
                        root_deck_id: 1,
                        card_ids: vec![1],
                        new_count: 1,
                        learning_count: 0,
                        review_count: 0,
                    },
                    DeckQueue {
                        source_index: 0,
                        root_deck_id: 2,
                        card_ids: vec![2],
                        new_count: 0,
                        learning_count: 0,
                        review_count: 1,
                    },
                ],
            };
        }
        let bytes = encode(manifest, BTreeMap::new()).expect("fixture bundle");
        decode(&bytes).expect("decoded fixture")
    }

    fn review_model(answer: bool) -> ReviewScreenModel {
        ReviewScreenModel {
            deck: "Japanese".to_owned(),
            status: format!("{} · 3 of 18", if answer { "Answer" } else { "Question" }),
            details: None,
            attachment_note: None,
            media_note: None,
            picture: None,
            answer,
            saving: None,
            japanese_font: None,
            menu_open: false,
            page: 0,
            pages: 1,
            completed: 2,
            total_cards: 18,
        }
    }

    fn japanese_pictures() -> (PictureCache, TilePicture, TilePicture) {
        let mut cache = PictureCache::default();
        let mut tiles = Vec::new();
        for (handle, text) in [(PICTURE_HANDLE, "日本語"), (PictureHandle(2), "答え")] {
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="260">
                  <rect x="1" y="1" width="638" height="258" fill="white" stroke="black"/>
                  <text x="320" y="155" text-anchor="middle" font-size="92">{text}</text>
                </svg>"#
            );
            let png = rasterize_svg(svg.as_bytes()).expect("safe Japanese SVG");
            let picture = kobo_image::decode(&png)
                .expect("SVG PNG")
                .fit(900, 480)
                .expect("fit");
            tiles.push(TilePicture::new(handle, picture.width(), picture.height()));
            assert!(cache.put(
                handle,
                picture.width(),
                picture.height(),
                picture.into_grey(),
            ));
        }
        (cache, tiles[0], tiles[1])
    }

    fn render_png(screen: &Screen, pictures: &PictureCache, pressed: Option<ActionId>) -> Vec<u8> {
        install_fonts();
        let chrome = Chrome::measuring(screen.owns_back);
        let mut surface = Surface::new(
            usize::try_from(CLARA_BW_METRICS.width).expect("width"),
            usize::try_from(CLARA_BW_METRICS.height).expect("height"),
        );
        render_all(
            screen,
            &CLARA_BW_METRICS,
            &chrome,
            pictures,
            &mut surface,
            None,
        );
        if let Some(action) = pressed {
            let rect = screen
                .layout_with(&CLARA_BW_METRICS, &chrome)
                .rect_of_action(action)
                .expect("pressed action");
            surface.invert_press(rect, &CLARA_BW_METRICS);
        }
        kobo_image::encode_png_grey(
            u32::try_from(surface.width).expect("width"),
            u32::try_from(surface.height).expect("height"),
            &surface.pixels,
        )
        .expect("PNG")
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one explicit inventory keeps every required golden state auditable"
    )]
    fn capture_cases() -> Vec<(&'static str, Screen, Option<ActionId>, bool)> {
        install_fonts();
        let (_, question_tile, answer_tile) = japanese_pictures();
        let mut deck_app = Flashcards {
            bundle: Some(fixture_bundle(true)),
            deck_pages: vec![vec![0, 1, 2]],
            japanese_font: Some(JAPANESE_FONT_HANDLE),
            view: View::Decks,
            ..Flashcards::default()
        };
        let empty_app = Flashcards {
            bundle: Some(fixture_bundle(false)),
            view: View::Decks,
            ..Flashcards::default()
        };
        let mut question = review_model(false);
        question.deck = "日本語".to_owned();
        question.picture = Some(question_tile);
        question.japanese_font = Some(JAPANESE_FONT_HANDLE);
        let question_screen = review_screen(&question, "この言葉を読みます。", &[]);
        let mut answer = review_model(true);
        answer.deck = "日本語".to_owned();
        answer.japanese_font = Some(JAPANESE_FONT_HANDLE);
        answer.picture = Some(answer_tile);
        let answer_text = "答えは日本語です。 Important detail.";
        let detail_start = answer_text.find("detail").expect("detail text");
        let answer_screen = review_screen(
            &answer,
            answer_text,
            &[CardTextSpan {
                start: detail_start,
                end: detail_start + "detail".len(),
                style: kobo_flashcards_format::CardTextStyle {
                    strong: true,
                    ..kobo_flashcards_format::CardTextStyle::default()
                },
            }],
        );
        let long_text =
            "これは長いカードの文章です。安全にページを分け、操作を画面の下に残します。".repeat(90);
        let mut long_model = review_model(true);
        long_model.japanese_font = Some(JAPANESE_FONT_HANDLE);
        let long_pages = paginate_review_text(&long_model, &long_text, &[], CLARA_BW_METRICS);
        long_model.page = 1.min(long_pages.len().saturating_sub(1));
        long_model.pages = long_pages.len();
        let long_screen = review_screen(
            &long_model,
            &long_pages[long_model.page].text,
            &long_pages[long_model.page].spans,
        );
        let context = Context::default();
        deck_app.notice_documents = build_notice_documents(&context);
        let notice_index = {
            deck_app.view = View::Notices;
            deck_app.notice_index()
        };
        let notice_screen = {
            deck_app.view = View::NoticeDocument;
            deck_app.notice_document = Some(0);
            deck_app.notice_page = 0;
            deck_app.notice()
        };
        vec![
            ("loading", loading_screen(0, None), None, false),
            (
                "loading-progress",
                loading_screen(384 * 1024, Some(1024 * 1024)),
                None,
                false,
            ),
            (
                "deck-picker",
                {
                    deck_app.view = View::Decks;
                    deck_app.deck_picker()
                },
                None,
                false,
            ),
            ("question-japanese-svg", question_screen, None, true),
            ("answer-reveal", answer_screen.clone(), None, true),
            (
                "rating-again",
                answer_screen.clone(),
                Some(action_id("again")),
                true,
            ),
            (
                "rating-hard",
                answer_screen.clone(),
                Some(action_id("hard")),
                true,
            ),
            (
                "rating-good",
                answer_screen.clone(),
                Some(action_id("good")),
                true,
            ),
            ("rating-easy", answer_screen, Some(action_id("easy")), true),
            ("long-text", long_screen, None, false),
            (
                "missing-media",
                problem_screen(
                    ProblemKind::UnsafeMedia,
                    "A required image is missing from the prepared bundle.",
                    false,
                ),
                None,
                false,
            ),
            (
                "unsafe-media",
                problem_screen(
                    ProblemKind::UnsafeMedia,
                    "A card image is outside the safe display rules.",
                    false,
                ),
                None,
                false,
            ),
            ("empty", empty_app.deck_picker(), None, false),
            (
                "done",
                done_screen("日本語", 18, 18, false, Some(JAPANESE_FONT_HANDLE)),
                None,
                false,
            ),
            (
                "corrupt-bundle",
                problem_screen(
                    ProblemKind::Corrupt,
                    "The staged collection is corrupt or unsupported.",
                    false,
                ),
                None,
                false,
            ),
            ("settings", settings_screen(false), None, false),
            ("licenses", notice_index, None, false),
            ("license-document", notice_screen, None, false),
        ]
    }

    #[test]
    fn compatibility_notice_is_unambiguous() {
        assert!(NOTICE.contains("Cobalt bundles"));
        assert!(NOTICE.contains("no linked study-engine code"));
        assert!(NOTICE.contains("no remote-network capability"));
        assert!(NOTICE.contains("local runtime IPC"));
        assert!(JAPANESE_FONT.len() <= kobo_sdk::MAX_FONT_BYTES);
        assert_eq!(
            digest_hex(JAPANESE_FONT),
            "150c82a7b6a4e39645099b3d27c96a00a148a1f57faf523027559910059c2dc0"
        );
    }

    #[test]
    fn deck_and_review_actions_meet_the_seven_millimetre_target() {
        install_fonts();
        let app = Flashcards {
            bundle: Some(fixture_bundle(true)),
            deck_pages: vec![vec![0, 1, 2]],
            japanese_font: Some(JAPANESE_FONT_HANDLE),
            view: View::Decks,
            ..Flashcards::default()
        };
        let deck = app
            .deck_picker()
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(false));
        for action in ["deck-all", "deck-0", "deck-1"] {
            let rect = deck
                .rect_of_action(action_id(action))
                .unwrap_or_else(|| panic!("{action}: {:?}", deck.nodes));
            assert!(
                rect.height >= CLARA_BW_METRICS.touch_target_minimum(),
                "{action}: {rect:?}"
            );
        }
        let answer = review_screen(&review_model(true), "Answer", &[]);
        let layout = answer.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        for grade in Grade::ALL {
            let rect = layout
                .rect_of_action(action_id(grade.action()))
                .expect("grade action");
            assert!(rect.width >= CLARA_BW_METRICS.touch_target_minimum());
            assert!(rect.height >= CLARA_BW_METRICS.touch_target_minimum());
        }
        let question = review_screen(&review_model(false), "Question", &[]);
        let reveal = question
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
            .rect_of_action(action_id("answer"))
            .expect("reveal action");
        assert!(reveal.height >= CLARA_BW_METRICS.touch_target_minimum());
    }

    #[test]
    fn deck_picker_counts_only_cards_remaining_this_session() {
        install_fonts();
        let mut app = Flashcards {
            bundle: Some(fixture_bundle(true)),
            deck_pages: vec![vec![0, 1, 2]],
            japanese_font: Some(JAPANESE_FONT_HANDLE),
            view: View::Decks,
            ..Flashcards::default()
        };
        app.reviewed_cards.insert(1);
        let choices = app.deck_choices();
        assert_eq!(choices[0].trailing, "1 due");
        assert_eq!(choices[1].trailing, "Done");
        assert_eq!(choices[2].trailing, "1 due");
        assert!(choices
            .iter()
            .all(|choice| !choice.summary.contains("1 new")));
        let layout = app
            .deck_picker()
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(false));
        let section = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Section)
            .expect("deck section");
        assert_eq!(section.text_lines.last().map(String::as_str), Some("1 due"));
    }

    #[test]
    fn rating_positions_are_stable_while_a_grade_is_saved() {
        let normal = review_screen(&review_model(true), "Answer", &[])
            .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let button_rect = |layout: &kobo_ui::Layout, label: &str| {
            layout
                .nodes
                .iter()
                .find(|node| {
                    matches!(node.kind, LayoutKind::Button(..))
                        && node.text_lines.iter().any(|line| line == label)
                })
                .map(|node| node.rect)
        };
        for selected in Grade::ALL {
            let mut saving = review_model(true);
            saving.saving = Some(selected);
            saving.status = format!("Saving {} · 3 of 18", selected.label());
            let layout = review_screen(&saving, "Answer", &[])
                .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
            for grade in Grade::ALL {
                assert_eq!(
                    button_rect(&normal, grade.label()),
                    button_rect(&layout, grade.label()),
                    "{} moved while saving {}",
                    grade.label(),
                    selected.label()
                );
            }
        }
    }

    #[test]
    fn question_and_answer_keep_stable_chrome_and_progress() {
        let question_screen = review_screen(&review_model(false), "Question", &[]);
        let answer_screen = review_screen(&review_model(true), "Answer", &[]);
        assert_eq!(question_screen.id, answer_screen.id);
        let question = question_screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let answer = answer_screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let rect = |layout: &kobo_ui::Layout, kind: LayoutKind| {
            layout
                .nodes
                .iter()
                .find(|node| node.kind == kind)
                .map(|node| node.rect)
                .expect("layout node")
        };
        assert_eq!(
            rect(&question, LayoutKind::TopBar),
            rect(&answer, LayoutKind::TopBar)
        );
        assert_eq!(
            rect(&question, LayoutKind::Progress),
            rect(&answer, LayoutKind::Progress)
        );
        let primary_count = |layout: &kobo_ui::Layout| {
            layout
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::Button(_, _, kobo_ui::Emphasis::Primary)
                    )
                })
                .count()
        };
        assert_eq!(primary_count(&question), 1);
        assert_eq!(primary_count(&answer), 1);
    }

    #[test]
    fn long_cards_paginate_without_clipping_controls() {
        install_fonts();
        let text = "長い文章でも、カードの操作は画面から消えません。".repeat(180);
        for scale in [
            TextScale::Default,
            TextScale::ExtraLarge,
            TextScale::Largest,
        ] {
            let mut metrics = CLARA_BW_METRICS;
            metrics.text_scale = scale;
            let mut model = review_model(true);
            model.japanese_font = Some(JAPANESE_FONT_HANDLE);
            let pages = paginate_review_text(&model, &text, &[], metrics);
            assert!(pages.len() > 1);
            for (index, page) in pages.iter().enumerate() {
                model.page = index;
                model.pages = pages.len();
                let screen = review_screen(&model, &page.text, &page.spans);
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{scale:?} page {index}: {:?}",
                    diagnostics.issues
                );
                for grade in Grade::ALL {
                    assert!(screen
                        .layout_with(&metrics, &Chrome::measuring(true))
                        .rect_of_action(action_id(grade.action()))
                        .is_some());
                }
            }
        }
    }

    #[test]
    fn supporting_screens_fit_without_dense_or_clipped_controls() {
        let context = Context::default();
        let notices = build_notice_documents(&context);
        assert_eq!(notices.len(), DEVICE_DISTRIBUTION_DOCUMENTS.len());
        assert!(
            notices
                .iter()
                .map(|document| document.pages.len())
                .sum::<usize>()
                > DEVICE_DISTRIBUTION_DOCUMENTS.len()
        );
        let notice_index = Flashcards {
            view: View::Notices,
            notice_documents: notices.clone(),
            ..Flashcards::default()
        }
        .notice_index();
        let diagnostics = notice_index.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        for index in 0..notices.len() {
            let rect = notice_index
                .layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true))
                .rect_of_action(action_id(&format!("notice-document-{index}")))
                .expect("notice row");
            assert!(rect.height >= CLARA_BW_METRICS.touch_target_minimum());
        }
        let screens = [
            loading_screen(0, None),
            loading_screen(100, Some(200)),
            settings_screen(false),
            problem_screen(
                ProblemKind::Corrupt,
                "The staged collection is corrupt or unsupported.",
                false,
            ),
            done_screen("Japanese", 20, 20, false, None),
        ];
        for screen in screens {
            let diagnostics =
                screen.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(screen.owns_back));
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
        for notice in notices {
            for paragraphs in notice.pages {
                let mut screen = ScreenBuilder::new("notice-test")
                    .top_bar(notice.title)
                    .owns_back(true);
                for paragraph in paragraphs {
                    screen = screen.text(paragraph);
                }
                let screen = screen
                    .bottom_action("done", "Licences")
                    .page_turns("previous", "next")
                    .page_position(1, 2)
                    .build();
                let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true));
                assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
            }
        }
    }

    #[test]
    fn answer_only_media_is_never_selected_before_reveal() {
        let card = Card {
            question_media_names: vec!["question.png".to_owned()],
            answer_media_names: vec!["answer.png".to_owned(), "question.png".to_owned()],
            media_names: vec!["answer.png".to_owned(), "question.png".to_owned()],
            attachments: vec![
                Attachment {
                    name: "answer.png".to_owned(),
                    rendered_name: None,
                    mime: "image/png".to_owned(),
                    kind: AttachmentKind::Image,
                },
                Attachment {
                    name: "question.png".to_owned(),
                    rendered_name: None,
                    mime: "image/png".to_owned(),
                    kind: AttachmentKind::Image,
                },
            ],
            ..fixture_card(1, 1)
        };
        assert_eq!(
            image_for_side(&card, false).map(|attachment| attachment.name.as_str()),
            Some("question.png")
        );
        assert_eq!(
            image_for_side(&card, true).map(|attachment| attachment.name.as_str()),
            Some("answer.png")
        );
    }

    #[test]
    fn rejection_messages_never_expose_media_names_or_raw_html() {
        let message = bundle_rejection_message(&FormatError::MediaDigestMismatch(
            "private-name.svg".to_owned(),
        ));
        assert!(!message.contains("private-name"));
        assert!(!message.contains('<'));
        assert!(message.contains("host converter"));
    }

    #[test]
    fn screenshot_goldens_match_every_required_state() {
        let (pictures, _, _) = japanese_pictures();
        let update = std::env::var_os("UPDATE_FLASHCARDS_GOLDENS").is_some();
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("screenshots")
            .join("states");
        let capture = std::env::var_os("COBALT_FLASHCARDS_SCREENSHOT_DIR").map(PathBuf::from);
        if update {
            fs::create_dir_all(&source).expect("golden directory");
        }
        if let Some(directory) = &capture {
            fs::create_dir_all(directory).expect("capture directory");
        }
        for (name, screen, pressed, uses_picture) in capture_cases() {
            let empty = PictureCache::default();
            let png = render_png(
                &screen,
                if uses_picture { &pictures } else { &empty },
                pressed,
            );
            let golden = source.join(format!("{name}.png"));
            if update {
                fs::write(&golden, &png).expect("write golden");
            }
            assert_eq!(
                fs::read(&golden)
                    .unwrap_or_else(|error| { panic!("read {}: {error}", golden.display()) }),
                png,
                "golden changed for {name}"
            );
            if let Some(directory) = &capture {
                fs::write(directory.join(format!("{name}.png")), &png)
                    .expect("write external capture");
            }
        }
    }
}
