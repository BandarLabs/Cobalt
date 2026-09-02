//! Offline Flashcards review for bundles prepared by `flashcards-import`.
//!
//! Flashcards is unofficial Anki-package compatibility software and is not
//! affiliated with Ankitects or `AnkiDroid`. Imported HTML is converted to text;
//! no card can execute script, access a file, or open a network socket.

use kobo_flashcards_format::{decode, AttachmentKind, ParsedBundle};
use kobo_sdk::{
    action_id, ActionId, Context, KoboApp, LogLevel, PictureHandle, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreError, StoreResult,
};
use std::process::ExitCode;

const BUNDLE_NAME: &str = "collection.cobfc";
const REVIEW_LOG_NAME: &str = "cobalt-review-log.ndjson";
const MAX_LOCAL_REVIEW_LOG_BYTES: usize = 16 * 1024 * 1024;
const MAX_DEVICE_BUNDLE_BYTES: usize = 32 * 1024 * 1024;
const NOTICE: &str = "Flashcards is unofficial Anki-package compatibility software and is not affiliated with Ankitects or AnkiDroid.";

#[derive(Default)]
struct Flashcards {
    library_download: Option<ShelfDownload>,
    review_download: Option<ShelfDownload>,
    review_upload: Option<ShelfUpload>,
    bundle: Option<ParsedBundle>,
    card: usize,
    answer: bool,
    saving: bool,
    pending_review: Option<String>,
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
        let Some(bundle) = &self.bundle else {
            return self.loading();
        };
        let Some(card) = bundle.manifest().cards.get(self.card) else {
            return ScreenBuilder::new("flashcards-empty")
                .top_bar("Flashcards")
                .empty_state("This collection has no reviewable cards.")
                .build();
        };
        let deck = bundle
            .manifest()
            .decks
            .iter()
            .find(|deck| deck.id == card.deck_id)
            .map_or("Imported deck", |deck| deck.name.as_str());
        let mut screen = ScreenBuilder::new("flashcards-review")
            .top_bar("Flashcards")
            .heading(deck)
            .secondary(format!(
                "{} · {} due · {} review{}",
                card.template_name,
                card.due,
                card.repetitions,
                if card.repetitions == 1 { "" } else { "s" }
            ))
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
                .secondary(non_playing_attachments(card))
                .action_bar([("again", "Again"), ("hard", "Hard"), ("good", "Good")])
                .build()
        } else {
            screen.bottom_action("answer", "Show answer").build()
        }
    }

    fn start_download(&mut self, context: &mut Context) {
        let mut download = ShelfDownload::new(BUNDLE_NAME).at_most(MAX_DEVICE_BUNDLE_BYTES);
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
                self.bundle = Some(bundle);
                self.card = 0;
                self.answer = false;
                self.message.clear();
                self.load_picture(context);
                context.set_screen(self.review());
            }
            Err(error) => {
                self.message = format!("The transferred collection was rejected: {error}");
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
        let Some(card) = bundle.manifest().cards.get(self.card) else {
            return;
        };
        let Some(image) = card.attachments.iter().find(|attachment| {
            attachment.kind == AttachmentKind::Image
                && if self.answer {
                    card.answer_media_names.contains(&attachment.name)
                } else {
                    card.question_media_names.contains(&attachment.name)
                }
        }) else {
            return;
        };
        let image_name = image.rendered_name.as_deref().unwrap_or(&image.name);
        let Some(bytes) = bundle.media(image_name) else {
            return;
        };
        let rendered: Vec<u8>;
        let bytes = if image.mime == "image/svg+xml" {
            rendered = match Self::rasterize_svg(bytes) {
                Ok(bytes) => bytes,
                Err(_) => return,
            };
            &rendered
        } else {
            bytes
        };
        let Ok(picture) = kobo_image::decode(bytes) else {
            return;
        };
        let Ok(picture) = picture.fit(960, 650) else {
            return;
        };
        self.picture = context.put_picture(
            PictureHandle(1),
            picture.width(),
            picture.height(),
            picture.into_grey(),
        );
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "both dimensions are capped to 1,920 pixels before scaling"
    )]
    fn rasterize_svg(bytes: &[u8]) -> Result<Vec<u8>, String> {
        let sanitized = std::str::from_utf8(bytes)
            .map_err(|_| "SVG is not UTF-8".to_owned())?
            .replace("kvg:", "metadata-kvg-")
            .replace("inkscape:", "metadata-inkscape-")
            .replace("sodipodi:", "metadata-sodipodi-");
        let tree =
            resvg::usvg::Tree::from_data(sanitized.as_bytes(), &resvg::usvg::Options::default())
                .map_err(|error| error.to_string())?;
        let size = tree.size().to_int_size();
        let width = size.width().min(1_920);
        let height = size.height().min(1_920);
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| "image dimensions are empty or too large".to_owned())?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(
                width as f32 / size.width() as f32,
                height as f32 / size.height() as f32,
            ),
            &mut pixmap.as_mut(),
        );
        pixmap.encode_png().map_err(|error| error.to_string())
    }

    fn record_review(&mut self, context: &mut Context, grade: &str) {
        if self.saving || self.bundle.is_none() {
            return;
        }
        let card = &self.bundle.as_ref().expect("checked").manifest().cards[self.card];
        // The imported Anki queue, ease and due values remain untouched. This
        // is explicitly a Cobalt-local event for later host reconciliation.
        let record = format!(
            "{{\"format\":1,\"card_id\":{},\"grade\":\"{}\",\"imported_due\":{},\"imported_reps\":{}}}",
            card.id, grade, card.due, card.repetitions
        );
        self.saving = true;
        self.pending_review = Some(record);
        let mut download = ShelfDownload::new(REVIEW_LOG_NAME).at_most(MAX_LOCAL_REVIEW_LOG_BYTES);
        download.start(context);
        self.review_download = Some(download);
        context.set_screen(self.review());
    }

    fn upload_review_log(&mut self, context: &mut Context, mut log: Vec<u8>) {
        let Some(record) = self.pending_review.as_ref() else {
            return;
        };
        let required = log.len().saturating_add(record.len()).saturating_add(1);
        if required > MAX_LOCAL_REVIEW_LOG_BYTES {
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
            .map_or(0, |bundle| bundle.manifest().cards.len());
        if count > 0 {
            self.card = (self.card + 1) % count;
        }
        self.answer = false;
        self.saving = false;
        self.pending_review = None;
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
        if action == action_id("retry") {
            self.start_download(context);
        } else if action == action_id("answer") && !self.saving {
            self.answer = true;
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
        assert!(NOTICE.contains("unofficial"));
        assert!(NOTICE.contains("not affiliated"));
    }

    #[test]
    fn safe_svgs_are_rasterized_before_the_image_decoder_runs() {
        let png = Flashcards::rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2"/></svg>"#,
        )
        .expect("rasterize");
        assert!(kobo_image::decode(&png).is_ok());
    }
}
