//! Offline Flashcards review for bundles prepared by `flashcards-import`.
//!
//! The app consumes only host-prepared, Cobalt-owned neutral bundles. Imported
//! HTML is already converted to text; no card can execute script, access a
//! file, or open a network socket.

use kobo_flashcards_format::{
    decode, digest_hex, validate_review_log, verify_card_images, AttachmentKind, FormatError,
    ParsedBundle, DEVICE_DISTRIBUTION_DOCUMENTS, MAX_BUNDLE_BYTES, MAX_REVIEW_LOG_BYTES,
};
use kobo_sdk::{
    action_id, ActionId, Context, KoboApp, LogLevel, PictureHandle, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreError, StoreResult,
};
use std::process::ExitCode;

const BUNDLE_NAME: &str = "collection.cobfc";
const REVIEW_LOG_NAME: &str = "cobalt-review-log.ndjson";
const NOTICE_PAGE_BYTES: usize = 700;
const NOTICE: &str = "Flashcards reviews host-prepared Cobalt bundles offline. The device contains no linked study-engine code, requests no remote-network capability, and uses only Cobalt's required local runtime IPC. Applicable device licences are in the Notices screen.";

#[derive(Default)]
struct Flashcards {
    library_download: Option<ShelfDownload>,
    review_download: Option<ShelfDownload>,
    review_upload: Option<ShelfUpload>,
    bundle: Option<ParsedBundle>,
    bundle_digest: String,
    queue_index: usize,
    answer: bool,
    saving: bool,
    pending_review: Option<String>,
    notice_page: Option<usize>,
    message: String,
    picture: Option<kobo_sdk::TilePicture>,
}

impl Flashcards {
    fn loading(&self) -> Screen {
        ScreenBuilder::new("flashcards-loading")
            .top_bar("Flashcards")
            .heading("Opening offline collection")
            .secondary(if self.message.is_empty() {
                "Reading the verified collection from this Kobo.".to_owned()
            } else {
                self.message.clone()
            })
            .build()
    }

    fn review(&self) -> Screen {
        if let Some(page) = self.notice_page {
            return Self::notice(page);
        }
        let Some(bundle) = &self.bundle else {
            return self.loading();
        };
        let Some(card) = self.current_card() else {
            return ScreenBuilder::new("flashcards-empty")
                .top_bar("Flashcards")
                .empty_state("This collection has no reviewable cards.")
                .button("notices", "Notices")
                .build();
        };
        let deck = bundle
            .manifest()
            .decks
            .iter()
            .find(|deck| deck.id == card.deck_id)
            .map_or("Imported deck", |deck| deck.name.as_str());
        let mut secondary = format!(
            "{} · {} due · {} review{}",
            card.template_name,
            card.due,
            card.repetitions,
            if card.repetitions == 1 { "" } else { "s" }
        );
        if self.answer {
            let attachments = non_playing_attachments(card);
            if !attachments.is_empty() {
                secondary.push('\n');
                secondary.push_str(&attachments);
            }
        }
        if !self.message.is_empty() {
            secondary = format!("{}\n{secondary}", self.message);
        }
        let mut screen = ScreenBuilder::new("flashcards-review")
            .top_bar("Flashcards")
            .heading(deck)
            .secondary(secondary)
            .text(if self.answer { &card.back } else { &card.front });
        if let Some(picture) = self.picture {
            screen = screen.unframed_picture(picture, 48);
        }
        if self.saving {
            return screen
                .transfer("Saving local review record", 0, None)
                .build();
        }
        if self.answer {
            screen
                .action_bar([("again", "Again"), ("hard", "Hard"), ("good", "Good")])
                .build()
        } else {
            screen
                .action_bar([("answer", "Show answer"), ("notices", "Notices")])
                .build()
        }
    }

    fn current_card(&self) -> Option<&kobo_flashcards_format::Card> {
        let bundle = self.bundle.as_ref()?;
        let card_id = *bundle
            .manifest()
            .review_queue
            .card_ids
            .get(self.queue_index)?;
        let index = bundle
            .manifest()
            .cards
            .binary_search_by_key(&card_id, |card| card.id)
            .ok()?;
        bundle.manifest().cards.get(index)
    }

    fn notice(page: usize) -> Screen {
        let total = notice_page_count();
        let (title, text) = notice_page(page.min(total.saturating_sub(1)));
        let mut actions = Vec::new();
        if page > 0 {
            actions.push(("notice-prev", "Previous"));
        }
        actions.push(("notice-close", "Review"));
        if page + 1 < total {
            actions.push(("notice-next", "Next"));
        }
        ScreenBuilder::new("flashcards-notices")
            .top_bar("Flashcards notices")
            .heading(title)
            .secondary(format!("Page {} of {total}", page + 1))
            .text(text)
            .action_bar(actions)
            .build()
    }

    fn start_download(&mut self, context: &mut Context) {
        let mut download = ShelfDownload::new(BUNDLE_NAME)
            .at_most(usize::try_from(MAX_BUNDLE_BYTES).expect("bundle bound fits device usize"));
        download.start(context);
        self.library_download = Some(download);
        self.message.clear();
        context.set_screen(self.loading());
    }

    fn finish_download(&mut self, context: &mut Context) {
        let Some(download) = self.library_download.take() else {
            return;
        };
        let bytes = download.take();
        match decode(&bytes) {
            Ok(bundle) => {
                if let Err(error) =
                    verify_card_images(&bundle, &bundle.manifest().review_queue.card_ids)
                {
                    self.message = format!("The transferred collection was rejected: {error}");
                    context.set_screen(
                        ScreenBuilder::new("flashcards-invalid")
                            .top_bar("Flashcards")
                            .error_state(&self.message)
                            .button("retry", "Read again")
                            .build(),
                    );
                    return;
                }
                self.bundle_digest = digest_hex(&bytes);
                self.bundle = Some(bundle);
                self.queue_index = 0;
                self.answer = false;
                self.message.clear();
                self.load_picture(context);
                context.set_screen(self.review());
            }
            Err(error) => {
                self.message = bundle_rejection_message(&error);
                context.set_screen(
                    ScreenBuilder::new("flashcards-invalid")
                        .top_bar("Flashcards")
                        .error_state(&self.message)
                        .button("retry", "Read again")
                        .build(),
                );
            }
        }
    }

    fn load_picture(&mut self, context: &mut Context) {
        self.picture = None;
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
            return;
        };
        let Ok(picture) = kobo_image::decode(bytes) else {
            return;
        };
        let Ok(picture) = picture.fit_enlarging(960, 650) else {
            return;
        };
        self.picture = context.put_picture(
            PictureHandle(1),
            picture.width(),
            picture.height(),
            picture.into_grey(),
        );
    }

    fn record_review(&mut self, context: &mut Context, grade: &str) {
        if self.saving || self.bundle.is_none() || self.bundle_digest.len() != 64 {
            return;
        }
        let Some(card) = self.current_card() else {
            return;
        };
        // Imported queue, ease and due values remain untouched. This is a
        // Cobalt-local event for later host reconciliation.
        let record = format!(
            "{{\"format\":2,\"bundle_sha256\":\"{}\",\"card_id\":{},\"grade\":\"{}\",\"imported_due\":{},\"imported_reps\":{}}}",
            self.bundle_digest, card.id, grade, card.due, card.repetitions
        );
        self.message.clear();
        self.saving = true;
        self.pending_review = Some(record);
        let mut download = ShelfDownload::new(REVIEW_LOG_NAME).at_most(MAX_REVIEW_LOG_BYTES);
        download.start(context);
        self.review_download = Some(download);
        context.set_screen(self.review());
    }

    fn upload_review_log(&mut self, context: &mut Context, mut log: Vec<u8>) {
        let Some(record) = self.pending_review.as_ref() else {
            return;
        };
        if validate_review_log(&log).is_err() {
            self.saving = false;
            self.pending_review = None;
            "Review was not saved because the existing local review log is malformed."
                .clone_into(&mut self.message);
            context.set_screen(self.review());
            return;
        }
        let required = log.len().saturating_add(record.len()).saturating_add(1);
        if required > MAX_REVIEW_LOG_BYTES {
            self.saving = false;
            self.pending_review = None;
            self.message.clear();
            self.message.push_str(
                "Review was not saved: export the local review log before adding more records.",
            );
            context.set_screen(self.review());
            return;
        }
        log.extend_from_slice(record.as_bytes());
        log.push(b'\n');
        let mut upload = ShelfUpload::new(REVIEW_LOG_NAME, log);
        upload.start(context);
        self.review_upload = Some(upload);
    }

    fn commit_review(&mut self, context: &mut Context) {
        let count = self
            .bundle
            .as_ref()
            .map_or(0, |bundle| bundle.manifest().review_queue.card_ids.len());
        if count > 0 {
            self.queue_index = self.queue_index.saturating_add(1).min(count);
        }
        self.answer = false;
        self.saving = false;
        self.pending_review = None;
        self.message.clear();
        self.load_picture(context);
        context.set_screen(self.review());
    }
}

impl KoboApp for Flashcards {
    fn on_start(&mut self, context: &mut Context) {
        context.log(LogLevel::Info, NOTICE);
        self.start_download(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("notices") {
            self.notice_page = Some(0);
            context.set_screen(self.review());
        } else if action == action_id("notice-prev") {
            self.notice_page = self.notice_page.map(|page| page.saturating_sub(1));
            context.set_screen(self.review());
        } else if action == action_id("notice-next") {
            let last = notice_page_count().saturating_sub(1);
            self.notice_page = self
                .notice_page
                .map(|page| page.saturating_add(1).min(last));
            context.set_screen(self.review());
        } else if action == action_id("notice-close") {
            self.notice_page = None;
            context.set_screen(self.review());
        } else if action == action_id("retry") {
            self.start_download(context);
        } else if action == action_id("answer") && !self.saving {
            self.answer = true;
            self.load_picture(context);
            context.set_screen(self.review());
        } else if action == action_id("again") {
            self.record_review(context, "again");
        } else if action == action_id("hard") {
            self.record_review(context, "hard");
        } else if action == action_id("good") {
            self.record_review(context, "good");
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let Some(download) = &mut self.library_download {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    self.finish_download(context);
                    return;
                }
                ShelfProgress::Moving { done, total } => {
                    self.message = format!("Reading {done} of {total} bytes");
                    context.set_screen(self.loading());
                    return;
                }
                ShelfProgress::Failed(error) => {
                    self.library_download = None;
                    self.message = format!("Collection transfer failed: {error}");
                    context.set_screen(
                        ScreenBuilder::new("flashcards-missing")
                            .top_bar("Flashcards")
                            .empty_state(&self.message)
                            .button("retry", "Try again")
                            .build(),
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
                ShelfProgress::Failed(error) => {
                    self.review_download = None;
                    self.saving = false;
                    self.pending_review = None;
                    self.message = format!(
                        "Review was not saved ({error}); the imported schedule was left unchanged."
                    );
                    context.set_screen(self.review());
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
                ShelfProgress::Failed(error) => {
                    self.review_upload = None;
                    self.saving = false;
                    self.pending_review = None;
                    self.message = format!(
                        "Review was not saved ({error}); the imported schedule was left unchanged."
                    );
                    context.set_screen(self.review());
                }
            }
        }
    }
}

fn bundle_rejection_message(error: &FormatError) -> String {
    if *error == FormatError::UnsupportedVersion(3) {
        "This collection uses the retired pre-neutral bundle format. Recreate it with the current host converter; the separate local review log will be preserved."
            .to_owned()
    } else {
        format!("The transferred collection was rejected: {error}")
    }
}

fn non_playing_attachments(card: &kobo_flashcards_format::Card) -> String {
    let audio = card
        .attachments
        .iter()
        .filter(|attachment| attachment.kind == AttachmentKind::Audio)
        .count();
    let video = card
        .attachments
        .iter()
        .filter(|attachment| attachment.kind == AttachmentKind::Video)
        .count();
    match (audio, video) {
        (0, 0) => String::new(),
        (audio, 0) => format!("{audio} audio attachment(s) retained; playback is unavailable."),
        (0, video) => format!("{video} video attachment(s) retained; playback is unavailable."),
        _ => format!(
            "{audio} audio and {video} video attachment(s) retained; playback is unavailable."
        ),
    }
}

fn image_for_side(
    card: &kobo_flashcards_format::Card,
    answer: bool,
) -> Option<&kobo_flashcards_format::Attachment> {
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

fn notice_page_count() -> usize {
    DEVICE_DISTRIBUTION_DOCUMENTS
        .iter()
        .map(|(_, text)| document_page_count(text))
        .sum::<usize>()
        .max(1)
}

fn notice_page(mut page: usize) -> (&'static str, String) {
    for (title, text) in DEVICE_DISTRIBUTION_DOCUMENTS {
        let pages = document_page_count(text);
        if page < pages {
            let mut start = 0;
            for _ in 0..page {
                start = notice_chunk_end(text, start);
            }
            let end = notice_chunk_end(text, start);
            return (title, text[start..end].trim().to_owned());
        }
        page = page.saturating_sub(pages);
    }
    ("Compatibility notice", NOTICE.to_owned())
}

fn document_page_count(text: &str) -> usize {
    let mut pages = 0;
    let mut start = 0;
    while start < text.len() {
        start = notice_chunk_end(text, start);
        pages += 1;
    }
    pages.max(1)
}

fn notice_chunk_end(text: &str, start: usize) -> usize {
    let mut end = start.saturating_add(NOTICE_PAGE_BYTES).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        if let Some(boundary) = text[start..end].rfind(['\n', ' ']) {
            end = start + boundary + 1;
        }
    }
    end.max((start + 1).min(text.len()))
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
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn loading_screen_fits_the_clara_bw_panel() {
        let app = Flashcards::default();
        let diagnostics = app
            .loading()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn compatibility_notice_is_unambiguous() {
        assert!(NOTICE.contains("Cobalt bundles"));
        assert!(NOTICE.contains("no linked study-engine code"));
        assert!(NOTICE.contains("no remote-network capability"));
        assert!(NOTICE.contains("local runtime IPC"));
    }

    #[test]
    fn shipped_notice_pages_include_every_required_document() {
        assert!(notice_page_count() > DEVICE_DISTRIBUTION_DOCUMENTS.len());
        let all = (0..notice_page_count())
            .map(|page| notice_page(page).1)
            .collect::<String>();
        assert!(all.contains("Apache License"));
        assert!(all.contains("Flashcards device application dependency notices"));
        assert!(!all.contains("AGPL-3.0-or-later"));
    }

    #[test]
    fn notice_screen_fits_the_clara_bw_panel() {
        let app = Flashcards {
            notice_page: Some(0),
            ..Flashcards::default()
        };
        let diagnostics = app
            .review()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
    }

    #[test]
    fn answer_only_media_is_not_selected_before_reveal() {
        let card = kobo_flashcards_format::Card {
            id: 1,
            note_id: 1,
            deck_id: 1,
            ordinal: 0,
            user_sequence: 0,
            queue: 0,
            card_type: 0,
            due: 0,
            interval: 0,
            ease_factor: 0,
            repetitions: 0,
            lapses: 0,
            remaining_steps: 0,
            original_due: 0,
            original_deck_id: 0,
            flags: 0,
            data: String::new(),
            modified: 0,
            template_name: "Card".to_owned(),
            front: "front".to_owned(),
            back: "back".to_owned(),
            tags: Vec::new(),
            question_media_names: vec!["question.png".to_owned()],
            answer_media_names: vec!["answer.png".to_owned(), "question.png".to_owned()],
            media_names: vec!["answer.png".to_owned(), "question.png".to_owned()],
            attachments: vec![
                kobo_flashcards_format::Attachment {
                    name: "answer.png".to_owned(),
                    rendered_name: None,
                    mime: "image/png".to_owned(),
                    kind: AttachmentKind::Image,
                },
                kobo_flashcards_format::Attachment {
                    name: "question.png".to_owned(),
                    rendered_name: None,
                    mime: "image/png".to_owned(),
                    kind: AttachmentKind::Image,
                },
            ],
            diagnostics: Vec::new(),
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
    fn legacy_bundle_rejection_explains_the_reimport_path() {
        let message = bundle_rejection_message(&FormatError::UnsupportedVersion(3));
        assert!(message.contains("Recreate"));
        assert!(message.contains("review log will be preserved"));
    }
}
