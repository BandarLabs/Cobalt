//! On-demand, researched audiobooks for the Kobo library.

mod pipeline;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id,
    audio::{AudioMetadata, AudioPlayer},
    Context, DeviceRequest, DeviceResult, Failure, KoboApp, PictureHandle, Screen, ScreenBuilder,
    ShelfProgress, ShelfUpload, StandardState, StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const AGAIN: &str = "again";
const CANCEL: &str = "cancel";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Stage {
    #[default]
    Compose,
    Research,
    Write,
    Narrate,
    Package,
    Save,
    Player,
    Failed,
}

#[derive(Default)]
struct Audiobook {
    stage: Stage,
    topic: Keyboard,
    task: Option<TaskId>,
    title: String,
    summary: String,
    parts: Vec<String>,
    next_part: usize,
    tracks: Vec<(String, Vec<u8>)>,
    archive_name: String,
    upload: Option<ShelfUpload>,
    saved: u32,
    total: u32,
    trouble: Option<(StandardState, String)>,
    hint: Option<&'static str>,
    player: Option<AudioPlayer>,
}

impl Audiobook {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        match self.stage {
            Stage::Compose => {
                let mut screen = ScreenBuilder::new("audiobook-compose")
                    .top_bar("Create an audiobook")
                    .heading("What should it be about?")
                    .text("It is researched from current sources, written as an original spoken script, and narrated aloud. The finished audiobook is saved to My Books.");
                // Rendered where the person is looking when they are told
                // to change it. Without this the Create button simply does
                // nothing for a topic that is too short.
                if let Some(hint) = self.hint {
                    screen = screen.secondary(hint);
                }
                screen
                    .typed(&self.topic, "Type any topic")
                    .keyboard(&self.topic, "Create")
                    .build()
            }
            Stage::Player => self.player.as_ref().map_or_else(
                || {
                    ScreenBuilder::new("audiobook-player-missing")
                        .top_bar("Audiobook")
                        .error_state("The player could not be prepared.")
                        .button(AGAIN, "Create another")
                        .build()
                },
                AudioPlayer::screen,
            ),
            Stage::Failed => {
                // The state and the words both come from the failure, so a
                // missing key reads as "Permission needed" and names the file,
                // rather than every failure reading "Something went wrong".
                let (state, advice) = self.trouble.as_ref().map_or(
                    (StandardState::Error, "The request failed."),
                    |(state, advice)| (*state, advice.as_str()),
                );
                ScreenBuilder::new("audiobook-failed")
                    .top_bar("Could not create audiobook")
                    .standard_state(state, advice)
                    .button(AGAIN, "Try another topic")
                    .build()
            }
            _ => {
                let (label, percent) = self.progress();
                let mut screen = ScreenBuilder::new("audiobook-progress")
                    .top_bar("Creating audiobook")
                    .heading(if self.title.is_empty() {
                        "Working"
                    } else {
                        &self.title
                    })
                    .activity(label, Some(percent))
                    .cancellable(CANCEL, "Cancel");
                if !self.summary.is_empty() {
                    screen = screen.text(&self.summary);
                }
                if self.stage == Stage::Save {
                    screen = screen.transfer(
                        "Saving to My Books",
                        u64::from(self.saved),
                        Some(u64::from(self.total)),
                    );
                }
                screen.build()
            }
        }
    }

    fn progress(&self) -> (&'static str, u8) {
        match self.stage {
            Stage::Research => ("Researching the topic", 10),
            Stage::Write => ("Writing the spoken script", 30),
            Stage::Narrate => {
                let total = self.parts.len().max(1);
                let percent = 35 + (self.next_part.saturating_mul(50) / total).min(50);
                ("Narrating the script", u8::try_from(percent).unwrap_or(85))
            }
            Stage::Package => ("Packaging Kobo audiobook", 88),
            Stage::Save => ("Saving audiobook", 94),
            Stage::Compose | Stage::Player | Stage::Failed => ("Preparing", 0),
        }
    }

    fn begin(&mut self, context: &mut Context) {
        let topic = self.topic.text().trim();
        if topic.len() < 3 {
            self.hint = Some("That is too short. Type a few words about the topic.");
            self.show(context);
            return;
        }
        self.stage = Stage::Research;
        self.hint = None;
        self.trouble = None;
        self.task = context.spawn(pipeline::research(topic));
        if self.task.is_none() {
            self.fail("The runtime is already busy.");
        }
        self.show(context);
    }

    fn start_writing(&mut self, context: &mut Context, research: &[u8]) {
        match pipeline::write_book(self.topic.text(), research) {
            Ok(task) => {
                self.stage = Stage::Write;
                self.task = context.spawn(task);
                if self.task.is_none() {
                    self.fail("The runtime is already busy.");
                }
            }
            Err(error) => self.fail(error),
        }
        self.show(context);
    }

    fn start_narrating(&mut self, context: &mut Context, response: &[u8]) {
        match pipeline::parse_book(response) {
            Ok(book) => {
                self.title.clone_from(&book.title);
                self.summary.clone_from(&book.summary);
                self.archive_name = archive_name(&book.title);
                self.parts = pipeline::narration_parts(&book);
                if self.parts.is_empty() {
                    self.fail("The script contained nothing to narrate.");
                } else {
                    self.stage = Stage::Narrate;
                    self.next_part = 0;
                    self.tracks.clear();
                    self.start_next_voice(context);
                }
            }
            Err(error) => self.fail(error),
        }
        self.show(context);
    }

    fn start_next_voice(&mut self, context: &mut Context) {
        let Some(text) = self.parts.get(self.next_part) else {
            self.package(context);
            return;
        };
        self.stage = Stage::Narrate;
        self.task = context.spawn(pipeline::speech(text));
        if self.task.is_none() {
            self.fail("The runtime is already busy.");
        }
    }

    fn received_voice(&mut self, context: &mut Context, audio: Vec<u8>) {
        if audio.len() < 256 {
            self.fail("The narration came back empty.");
            self.show(context);
            return;
        }
        self.tracks
            .push((format!("{:03}.mp3", self.next_part + 1), audio));
        self.next_part += 1;
        self.start_next_voice(context);
        self.show(context);
    }

    fn package(&mut self, context: &mut Context) {
        self.stage = Stage::Package;
        match kobo_doc::zip::stored(&self.tracks) {
            Ok(bytes) => {
                self.total = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                self.saved = 0;
                let mut upload = ShelfUpload::new(self.archive_name.clone(), bytes);
                upload.start(context);
                self.upload = Some(upload);
                self.stage = Stage::Save;
            }
            Err(error) => self.fail(format!("Could not package the audiobook: {error}")),
        }
    }

    fn reset(&mut self) {
        *self = Self::default();
    }

    /// A failure this application described itself, in its own words.
    fn fail(&mut self, error: impl Into<String>) {
        self.fail_as(StandardState::Error, error);
    }

    /// A failure the SDK described, carrying its state so the screen shows the
    /// right mark and heading rather than "Something went wrong" for all of
    /// them.
    fn fail_with(&mut self, failure: Failure) {
        self.fail_as(failure.state, failure.advice);
    }

    fn fail_as(&mut self, state: StandardState, error: impl Into<String>) {
        self.stage = Stage::Failed;
        self.task = None;
        self.upload = None;
        self.trouble = Some((state, error.into()));
    }
}

impl KoboApp for Audiobook {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
        if self.stage == Stage::Player
            && self
                .player
                .as_mut()
                .is_some_and(|player| player.press(context, action))
        {
            self.show(context);
            return;
        }
        if action == action_id(CANCEL) {
            if let Some(task) = self.task.take() {
                context.cancel(task);
            }
            self.reset();
            self.show(context);
            return;
        }
        if action == action_id(AGAIN) {
            if self.player.is_some() {
                context.device().stop_audio();
            }
            self.reset();
            self.show(context);
            return;
        }
        if self.stage == Stage::Compose {
            if let Some(pressed) = self.topic.press(action) {
                if pressed == Pressed::Submitted {
                    self.begin(context);
                } else {
                    self.show(context);
                }
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self
            .player
            .as_mut()
            .is_some_and(|player| player.on_task(context, task, &outcome))
        {
            self.show(context);
            return;
        }
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match self.stage {
                Stage::Research => self.start_writing(context, &bytes),
                Stage::Write => self.start_narrating(context, &bytes),
                Stage::Narrate => self.received_voice(context, bytes),
                _ => self.fail("A provider answered at the wrong stage."),
            },
            TaskOutcome::Failed(error) => self.fail_with(Failure::of(error)),
            TaskOutcome::Cancelled => self.reset(),
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        let Some(upload) = self.upload.as_mut() else {
            return;
        };
        match upload.advance(context, &result) {
            ShelfProgress::Moving { done, total } => {
                self.saved = done;
                self.total = total;
            }
            ShelfProgress::Done => {
                self.saved = self.total;
                self.upload = None;
                self.tracks.clear();
                self.parts.clear();
                let (width, height, grey) = cover_art(&self.title);
                let cover = context.put_picture(PictureHandle(1), width, height, grey);
                let mut player = AudioPlayer::shelf(&self.archive_name, &self.title)
                    .metadata(
                        AudioMetadata::new(&self.title)
                            .author("Researched, written and narrated on this reader")
                            .chapter("Generated on demand"),
                    )
                    .secondary_action(AGAIN, "Create another");
                player.set_cover(cover);
                player.start(context);
                self.player = Some(player);
                self.stage = Stage::Player;
            }
            ShelfProgress::Failed(error) => self.fail_with(Failure::storing(error)),
            ShelfProgress::Elsewhere => return,
        }
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if self
            .player
            .as_mut()
            .is_some_and(|player| player.on_device_result(context, &request, &result))
        {
            self.show(context);
        }
    }
}

/// Deterministic monochrome cover art. It travels once through the SDK picture
/// cache and remains visible while transport state and position redraw.
fn cover_art(title: &str) -> (u32, u32, Vec<u8>) {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 320;
    let pixels = usize::try_from(WIDTH * HEIGHT).expect("the cover fits memory");
    let mut grey = vec![240_u8; pixels];
    let seed = title.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        hash.rotate_left(5) ^ u32::from(byte)
    });
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let border = x < 8 || y < 8 || x >= WIDTH - 8 || y >= HEIGHT - 8;
            let disc_x = i64::from(x) - i64::from(WIDTH) / 2;
            let disc_y = i64::from(y) - 118;
            let disc = disc_x * disc_x + disc_y * disc_y < 72 * 72;
            let distance = u32::try_from(disc_x * disc_x + disc_y * disc_y)
                .expect("a cover coordinate has a small square");
            let groove = disc && (distance / 180 + seed) % 3 == 0;
            let bar = (88..=232).contains(&y)
                && (24..WIDTH - 24).contains(&x)
                && (x / 12 + seed) % 5 < 2
                && y > 205 - (x * 17 + seed) % 55;
            let index = usize::try_from(y * WIDTH + x).expect("the cover index fits usize");
            if border || groove || bar {
                grey[index] = 24;
            }
        }
    }
    (WIDTH, HEIGHT, grey)
}

fn archive_name(title: &str) -> String {
    let mut name = String::new();
    let mut dash = false;
    for character in title.chars() {
        if character.is_ascii_alphanumeric() {
            if dash && !name.is_empty() {
                name.push('-');
            }
            name.push(character.to_ascii_lowercase());
            dash = false;
        } else {
            dash = true;
        }
        if name.len() >= 48 {
            break;
        }
    }
    let name = name.trim_matches('-');
    format!("{}.mp3z", if name.is_empty() { "audiobook" } else { name })
}

fn main() -> ExitCode {
    match kobo_sdk::run("audiobook", Audiobook::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("audiobook: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{archive_name, Audiobook, Stage};
    use kobo_sdk::{Failure, StandardState, CLARA_BW_METRICS};

    #[test]
    fn a_title_becomes_a_safe_kobo_filename() {
        assert_eq!(archive_name("Moon: Past & Future"), "moon-past-future.mp3z");
    }

    #[test]
    fn compose_progress_complete_and_failure_screens_fit_a_clara() {
        let mut app = Audiobook::default();
        for stage in [Stage::Compose, Stage::Research, Stage::Failed] {
            app.stage = stage;
            app.title = "A researched history of the night sky".to_owned();
            app.summary = "An original, source-grounded tour of how people learned to understand the Moon, planets, and stars.".to_owned();
            app.archive_name = "history-of-the-night-sky.mp3z".to_owned();
            app.trouble = Some((
                StandardState::Error,
                "The provider could not complete this request.".to_owned(),
            ));
            let issues = app.screen().validate(&CLARA_BW_METRICS);
            assert!(issues.is_empty(), "{stage:?}: {issues:?}");
        }
    }

    /// The hint used to be written to a field the compose screen never drew,
    /// so a topic under three characters made the Create button do nothing at
    /// all. It has to reach the screen, and it has to still fit with a
    /// keyboard already on the panel.
    #[test]
    fn a_short_topic_puts_a_visible_hint_on_the_compose_screen() {
        let app = Audiobook {
            hint: Some("That is too short. Type a few words about the topic."),
            ..Audiobook::default()
        };
        let screen = app.screen();
        let drawn = format!("{screen:?}");
        assert!(drawn.contains("That is too short"), "{drawn}");
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// A missing API key is not a permission the application lacks, and the
    /// screen has to say which thing is actually wrong.
    #[test]
    fn a_missing_key_names_the_key_rather_than_blaming_the_application() {
        let mut app = Audiobook::default();
        app.fail_with(Failure::of(kobo_sdk::TaskError::NoCredential));
        let (state, advice) = app.trouble.clone().expect("a failure was recorded");
        assert_eq!(state, StandardState::PermissionDenied);
        assert!(advice.contains("kobo secret set"), "{advice}");
        assert!(
            !advice.contains("does not hold this permission"),
            "{advice}"
        );
    }
}
