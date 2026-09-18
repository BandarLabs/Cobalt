//! On-demand, researched audiobooks for the Kobo library.

mod pipeline;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id,
    audio::{AudioMetadata, AudioPlayer},
    Context, DeviceRequest, DeviceResult, Failure, Glyph, Heartbeat, KoboApp, PictureHandle,
    Screen, ScreenBuilder, ShelfProgress, ShelfUpload, StandardState, StoreResult, TaskId,
    TaskOutcome,
};
use std::process::ExitCode;

const AGAIN: &str = "again";
const CANCEL: &str = "cancel";
const NEW: &str = "new";
/// The firmware's container for a sideloaded audiobook.
const ARCHIVE_SUFFIX: &str = ".mp3z";
const SHELF: &str = "shelf";
const LIBRARY_BACK: &str = "library-back";
const LIBRARY_NEXT: &str = "library-next";

/// The three accounts one audiobook spends, checked before the first spend.
const SECRETS: &[&str] = &["exa", "openai", "elevenlabs"];
const RESUME: &str = "resume";

/// Where the title of each finished audiobook is kept.
///
/// The shelf stores bytes under a file name, and a file name is a slug: it
/// cannot carry "The Moon's Past and Future" back out again. So the archive
/// name is mapped to the title here, in the ordinary key-value store, which
/// lives beside the application and survives a restart exactly as the shelf
/// does. The shelf remains the truth about what exists; this only says what
/// each thing is called.
const LIBRARY_KEY: &str = "library";

/// Where the last interrupted creation is kept.
///
/// A checkpoint is text only: the script and how far narration got through
/// it. Audio already narrated lives in memory for an immediate retry; after
/// a restart the script is what survives, and narration starts it again from
/// the first part. That is the honest bound of what a key-value store can
/// hold - the parts together are a spoken book, and the store is for facts.
const CHECKPOINT_KEY: &str = "checkpoint";

/// One finished audiobook, on the reader, playable with the network off.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Saved {
    name: String,
    title: String,
    bytes: u32,
}

/// A creation interrupted after its script was written.
///
/// Everything needed to narrate it again: the words, the name it will be
/// saved under, and how many parts were already spoken when it stopped.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Checkpoint {
    topic: String,
    language: pipeline::Language,
    title: String,
    summary: String,
    archive_name: String,
    parts: Vec<String>,
    next_part: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Stage {
    /// What is already on the reader. The application opens here rather than
    /// on the composer, because an audiobook that took four minutes and
    /// fourteen narration calls to make is worth more than the next one.
    #[default]
    Library,
    Compose,
    /// Account setup is checked before the first paid request, so a
    /// missing key is reported before anything is spent.
    Setup,
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
    /// What the book is written and narrated in. Chosen on the composer,
    /// spoken by a narrator whose accent is native to it.
    language: pipeline::Language,
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
    /// `None` until the shelf has answered, so the library can say it is
    /// looking rather than claiming to be empty before it knows.
    library: Option<Vec<Saved>>,
    /// Archive name to title, oldest first, as it was last saved.
    titles: Vec<(String, String)>,
    /// Which library entries belong to which page. Nothing here scrolls.
    pages: Vec<Vec<usize>>,
    page: usize,
    /// Ticks while a provider is thinking, so a stage that takes a hundred
    /// seconds does not sit on an unchanging panel looking crashed.
    clock: Heartbeat,
    /// Set when the shelf refuses to say what is on it, so the library can
    /// stop looking. Without this a refusal leaves "Looking on the shelf" on
    /// the panel for as long as the application is open.
    shelf_unreadable: bool,
    /// The interrupted creation the store last told us about, offered on the
    /// composer until a new book supersedes it or a save finishes it.
    checkpoint: Option<Checkpoint>,
}

impl Audiobook {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        match self.stage {
            Stage::Library => self.library_screen(),
            Stage::Compose => {
                let mut screen =
                    ScreenBuilder::new("audiobook-compose").top_bar("Create an audiobook");
                if self.has_books() {
                    screen = screen.top_bar_glyph(SHELF, "Audiobooks", Glyph::Headphones);
                }
                let mut screen = screen
                    .heading("What should it be about?")
                    .text("It is researched from current sources, written as an original spoken script, and narrated aloud. The finished audiobook stays on this reader and plays with the network off.");
                if let Some(checkpoint) = &self.checkpoint {
                    let label = if checkpoint.title.is_empty() {
                        "Resume the interrupted audiobook".to_owned()
                    } else {
                        format!(
                            "Resume '{}' (part {} of {})",
                            checkpoint.title,
                            checkpoint.next_part + 1,
                            checkpoint.parts.len().max(1)
                        )
                    };
                    screen = screen.button(RESUME, label);
                }
                // Rendered where the person is looking when they are told
                // to change it. Without this the Create button simply does
                // nothing for a topic that is too short.
                if let Some(hint) = self.hint {
                    screen = screen.secondary(hint);
                }
                screen
                    .section("Language")
                    .chips(pipeline::LANGUAGES.map(|language| {
                        (
                            language_action(language),
                            language.label().to_owned(),
                            language == self.language,
                        )
                    }))
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
            Stage::Failed => self.failed_screen(),
            _ => {
                let (label, percent) = self.progress();
                let label = label.as_str();
                let mut screen = ScreenBuilder::new("audiobook-progress")
                    .top_bar("Creating audiobook")
                    .heading(if self.title.is_empty() {
                        "Working"
                    } else {
                        &self.title
                    })
                    .activity(label, Some(percent))
                    .cancellable(CANCEL, "Cancel");
                // The only honest thing there is to say about a request whose
                // far end reports nothing until it is finished. The percentage
                // above is the stage; this is the proof that the reader is
                // still alive.
                let waited = self.clock.waited_words();
                if !waited.is_empty() {
                    screen = screen.secondary(waited);
                }
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

    /// The failure, in the words of whatever failed, with the cheapest way
    /// forward first.
    fn failed_screen(&self) -> Screen {
        // The state and the words both come from the failure, so a
        // missing key reads as "Permission needed" and names the file,
        // rather than every failure reading "Something went wrong".
        let (state, advice) = self.trouble.as_ref().map_or(
            (StandardState::Error, "The request failed."),
            |(state, advice)| (*state, advice.as_str()),
        );
        let mut screen = ScreenBuilder::new("audiobook-failed")
            .top_bar("Could not create audiobook")
            .standard_state(state, advice);
        // Resume is first because it is the cheaper way forward: the
        // script survives in the store and, within a session, so do
        // the parts already narrated.
        let resume_label = if self.parts.is_empty() {
            self.checkpoint
                .as_ref()
                .map(|checkpoint| format!("Resume '{}'", checkpoint.title))
        } else {
            Some(format!("Resume '{}'", self.title))
        };
        match (resume_label.is_some(), self.has_books()) {
            (true, true) => {
                screen = screen.buttons([
                    (RESUME, resume_label.as_deref().unwrap_or("Resume")),
                    (AGAIN, "Start over"),
                    (SHELF, "Your audiobooks"),
                ]);
            }
            (true, false) => {
                screen = screen.buttons([
                    (RESUME, resume_label.as_deref().unwrap_or("Resume")),
                    (AGAIN, "Start over"),
                ]);
            }
            (false, true) => {
                screen = screen.buttons([(AGAIN, "Try another topic"), (SHELF, "Your audiobooks")]);
            }
            (false, false) => screen = screen.button(AGAIN, "Try another topic"),
        }
        screen.build()
    }

    /// What is already on the reader, and the way back to it.
    /// What is already on the reader, and the way back to it.
    ///
    /// Everything here reads from disk. Nothing on this screen, and nothing
    /// reached from it, needs the network: a book made in March plays in
    /// September on a reader that has been in aeroplane mode since.
    fn library_screen(&self) -> Screen {
        let screen = ScreenBuilder::new("audiobook-library").top_bar("Audiobooks");
        let Some(books) = self.library.as_ref() else {
            return screen.activity("Looking on the shelf", None).build();
        };
        if books.is_empty() {
            let screen = if self.shelf_unreadable {
                screen
                    .error_state("The shelf could not be read, so what is saved cannot be listed.")
            } else {
                screen.empty_state(
                    "No audiobooks yet. One made here stays on the reader and plays offline.",
                )
            };
            return screen.button(NEW, "Create an audiobook").build();
        }
        // The bottom of the panel is spent on page turns, so the way to the
        // composer is the one action the top bar allows.
        let screen = screen.top_bar_glyph(NEW, "Create", Glyph::Plus);
        let showing = self.pages.get(self.page).map_or(&[][..], Vec::as_slice);
        let mut turning = screen
            .rows_with_trailing(showing.iter().filter_map(|index| {
                books.get(*index).map(|book| {
                    (
                        play_action(*index),
                        book.title.clone(),
                        String::new(),
                        Glyph::Headphones,
                        size_on_disk(book.bytes),
                    )
                })
            }))
            .page_turns(LIBRARY_BACK, LIBRARY_NEXT);
        if self.pages.len() > 1 {
            turning = turning.page_position(
                u16::try_from(self.page + 1).unwrap_or(u16::MAX),
                u16::try_from(self.pages.len()).unwrap_or(u16::MAX),
            );
        }
        turning.build()
    }

    fn has_books(&self) -> bool {
        self.library.as_ref().is_some_and(|books| !books.is_empty())
    }

    /// Opens the library, and asks the shelf what is on it.
    ///
    /// The shelf is asked every time rather than once at start, because this
    /// is also the screen somebody arrives at after making an audiobook, and
    /// the disk is the only thing that knows whether it really landed.
    fn open_library(&mut self, context: &mut Context) {
        self.stage = Stage::Library;
        context.shelf().list();
        self.show(context);
    }

    /// Builds the library from what is actually on the shelf.
    ///
    /// The shelf decides what exists; the saved index only supplies titles and
    /// the order they were made in. A book whose title was lost still lists,
    /// under its file name turned back into words, because a listing that
    /// silently omits a four minute audiobook is worse than one with an ugly
    /// name in it.
    fn shelved(&mut self, context: &Context, blobs: &[(String, u32)]) {
        let mut books = Vec::new();
        for (name, title) in self.titles.iter().rev() {
            if let Some((name, bytes)) = blobs.iter().find(|(shelved, _)| shelved == name) {
                books.push(Saved {
                    name: name.clone(),
                    title: title.clone(),
                    bytes: *bytes,
                });
            }
        }
        for (name, bytes) in blobs {
            let known = books.iter().any(|book| &book.name == name);
            if !known && name.ends_with(ARCHIVE_SUFFIX) {
                books.push(Saved {
                    name: name.clone(),
                    title: title_from_name(name),
                    bytes: *bytes,
                });
            }
        }
        let sizes = books
            .iter()
            .map(|book| size_on_disk(book.bytes))
            .collect::<Vec<_>>();
        let rows = books
            .iter()
            .zip(&sizes)
            .map(|(book, size)| (book.title.as_str(), "", size.as_str()))
            .collect::<Vec<_>>();
        self.pages = context.paginate_rows_with_trailing(&rows, false);
        self.page = self.page.min(self.pages.len().saturating_sub(1));
        self.library = Some(books);
    }

    /// Records the title of a finished audiobook, so the library can name it.
    fn remember(&mut self, context: &mut Context) {
        self.titles.retain(|(name, _)| name != &self.archive_name);
        self.titles
            .push((self.archive_name.clone(), self.title.clone()));
        let index = self
            .titles
            .iter()
            .map(|(name, title)| format!("{name}\t{title}"))
            .collect::<Vec<_>>()
            .join("\n");
        context.store().save(LIBRARY_KEY, index);
    }

    fn play(&mut self, context: &mut Context, index: usize) {
        let Some(book) = self
            .library
            .as_ref()
            .and_then(|books| books.get(index))
            .cloned()
        else {
            return;
        };
        self.title = book.title;
        self.archive_name = book.name;
        self.open_player(context);
    }

    /// The player, for an audiobook that has just been made and for one that
    /// was made weeks ago. One function, because the two must not drift: the
    /// second is the one somebody uses fifty times.
    fn open_player(&mut self, context: &mut Context) {
        let (width, height, grey) = cover_art(&self.title);
        let cover = context.put_picture(PictureHandle(1), width, height, grey);
        let mut player = AudioPlayer::shelf(&self.archive_name, &self.title)
            .metadata(
                AudioMetadata::new(&self.title)
                    .author("Researched, written and narrated on this reader")
                    .chapter("Saved on this reader"),
            )
            .secondary_action(AGAIN, "Create another", Glyph::Plus)
            .owns_back(true);
        player.set_cover(cover);
        player.start(context);
        self.player = Some(player);
        self.stage = Stage::Player;
    }

    /// Continues an interrupted creation.
    ///
    /// Within a session the parts already narrated are still in memory and
    /// only the part that failed is asked for again. After a restart the
    /// script is what the store held, and narration begins it again from
    /// the first part - re-narrating a part costs one call, re-writing the
    /// book would cost every one of them.
    fn resume(&mut self, context: &mut Context) {
        if self.parts.is_empty() {
            let Some(checkpoint) = self.checkpoint.take() else {
                return;
            };
            self.topic = Keyboard::with_text(&checkpoint.topic);
            self.language = checkpoint.language;
            self.title = checkpoint.title;
            self.summary = checkpoint.summary;
            self.archive_name = checkpoint.archive_name;
            self.parts = checkpoint.parts;
            self.next_part = 0;
            self.tracks.clear();
        }
        if self.parts.is_empty() {
            return;
        }
        self.upload = None;
        self.trouble = None;
        self.clock.start(context);
        self.stage = Stage::Narrate;
        self.start_next_voice(context);
    }

    /// Records how far an interrupted creation got, at each boundary where
    /// that answer changes: the script written, and each part narrated.
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            topic: self.topic.text().to_owned(),
            language: self.language,
            title: self.title.clone(),
            summary: self.summary.clone(),
            archive_name: self.archive_name.clone(),
            parts: self.parts.clone(),
            next_part: self.next_part,
        }
    }

    fn save_checkpoint(&self, context: &mut Context) {
        context
            .store()
            .save(CHECKPOINT_KEY, write_checkpoint(&self.checkpoint()));
    }

    fn progress(&self) -> (String, u8) {
        match self.stage {
            Stage::Setup => ("Checking account setup".to_owned(), 5),
            Stage::Research => ("Researching the topic".to_owned(), 10),
            Stage::Write => ("Writing the spoken script".to_owned(), 30),
            Stage::Narrate => {
                let total = self.parts.len().max(1);
                let percent = 35 + (self.next_part.saturating_mul(50) / total).min(50);
                (
                    format!(
                        "Narrating part {} of {total}",
                        self.next_part.min(total - 1) + 1
                    ),
                    u8::try_from(percent).unwrap_or(85),
                )
            }
            Stage::Package => ("Packaging Kobo audiobook".to_owned(), 88),
            Stage::Save => ("Saving audiobook".to_owned(), 94),
            Stage::Library | Stage::Compose | Stage::Player | Stage::Failed => {
                ("Preparing".to_owned(), 0)
            }
        }
    }

    fn begin(&mut self, context: &mut Context) {
        let topic = self.topic.text().trim();
        if topic.len() < 3 {
            self.hint = Some("That is too short. Type a few words about the topic.");
            self.show(context);
            return;
        }
        self.stage = Stage::Setup;
        self.hint = None;
        self.trouble = None;
        // A new creation supersedes an interrupted one: the person read the
        // offer to resume it and chose a fresh topic instead.
        self.checkpoint = None;
        context.store().forget(CHECKPOINT_KEY);
        // Every account this book will spend is checked before the first
        // request goes out. Research, writing and narration each have their
        // own key, and finding out the narration key is missing after the
        // research was paid for is the failure this stage exists to prevent.
        context.secrets().check(SECRETS);
        self.show(context);
    }

    /// The answer to the preflight: start spending, or say exactly which
    /// accounts are missing while nothing has been spent yet.
    fn checked_secrets(&mut self, context: &mut Context, present: &[String]) {
        if self.stage != Stage::Setup {
            return;
        }
        let missing = SECRETS
            .iter()
            .filter(|name| !present.iter().any(|held| held == *name))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            self.stage = Stage::Research;
            // One clock for the whole creation rather than one per stage. What
            // somebody waiting wants to know is how long they have been waiting,
            // not how long this particular provider has.
            self.clock.start(context);
            self.task = context.spawn(pipeline::research(self.topic.text().trim()));
            if self.task.is_none() {
                self.fail("The runtime is already busy.");
            }
        } else {
            let services = missing
                .iter()
                .map(|name| service_name(name))
                .collect::<Vec<_>>()
                .join(", ");
            let names = missing
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            self.fail_as(
                StandardState::PermissionDenied,
                format!(
                    "Account details for {services} are missing (secrets: {names}). Add them, then try again."
                ),
            );
        }
        self.show(context);
    }

    /// The check itself could not run, so the flow continues and lets each
    /// stage report its own missing key the way it always has. An old
    /// runtime answers this way; it is a degraded path, not a failure.
    fn skip_preflight(&mut self, context: &mut Context) {
        if self.stage != Stage::Setup {
            return;
        }
        self.stage = Stage::Research;
        self.clock.start(context);
        self.task = context.spawn(pipeline::research(self.topic.text().trim()));
        if self.task.is_none() {
            self.fail("The runtime is already busy.");
        }
        self.show(context);
    }

    fn start_writing(&mut self, context: &mut Context, research: &[u8]) {
        match pipeline::write_book(self.topic.text(), self.language, research) {
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
                    // The script is the expensive thing two providers made;
                    // from here an interruption has something worth keeping.
                    let checkpoint = self.checkpoint();
                    self.checkpoint = Some(checkpoint.clone());
                    context
                        .store()
                        .save(CHECKPOINT_KEY, write_checkpoint(&checkpoint));
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
        self.task = context.spawn(pipeline::speech(text, self.language));
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
        self.save_checkpoint(context);
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

    /// Back to a blank composer, keeping the library.
    ///
    /// `Default` clears everything, and everything used to be the right
    /// amount, because the application forgot each audiobook the moment it
    /// finished. It no longer does, so what is on the shelf has to outlive a
    /// cancel.
    fn reset(&mut self) {
        let library = self.library.take();
        let titles = std::mem::take(&mut self.titles);
        let pages = std::mem::take(&mut self.pages);
        let shelf_unreadable = self.shelf_unreadable;
        // A person who narrates in Hindi will narrate in Hindi again.
        let language = self.language;
        *self = Self {
            language,
            library,
            titles,
            pages,
            shelf_unreadable,
            ..Self::default()
        };
    }

    /// A failure this application described itself, in its own words.
    fn fail(&mut self, error: impl Into<String>) {
        self.fail_as(StandardState::Error, error);
    }

    /// A failure the SDK described, carrying its state so the screen shows the
    /// right mark and heading rather than "Something went wrong" for all of
    /// them.
    fn fail_with(&mut self, failure: Failure) {
        // Three providers, three keys. "Install one with kobo secret set" is
        // no help at all if it does not say which of the three is missing, and
        // the stage that failed is exactly the thing that knows.
        self.fail_as(failure.state, failure.naming(self.secret_wanted()));
    }

    /// The credential the stage in flight asked for.
    const fn secret_wanted(&self) -> &'static str {
        match self.stage {
            Stage::Research => "exa",
            Stage::Narrate => "elevenlabs",
            _ => "openai",
        }
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
        // Titles first: the shelf is asked once they are back, so a listing
        // never arrives with nothing to name it by.
        context.store().load(LIBRARY_KEY);
        context.store().load(CHECKPOINT_KEY);
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
            self.clock.stop(context);
            self.reset();
            self.show(context);
            return;
        }
        if action == action_id(RESUME) {
            self.resume(context);
            self.show(context);
            return;
        }
        if action == action_id(AGAIN) {
            if self.player.is_some() {
                context.device().stop_audio();
            }
            self.clock.stop(context);
            self.reset();
            // Starting over from a failure retires what was interrupted:
            // the person has seen it fail and chosen a different book.
            self.checkpoint = None;
            context.store().forget(CHECKPOINT_KEY);
            self.stage = Stage::Compose;
            self.show(context);
            return;
        }
        // The player is always reached from the shelf, so the runtime's back
        // control belongs to the shelf here rather than to leaving the
        // application. Without this a tap on a book was a one way door.
        if action == action_id(SHELF)
            || (self.stage == Stage::Player && action == kobo_sdk::ActionId::BACK)
        {
            if self.player.is_some() {
                context.device().stop_audio();
            }
            self.clock.stop(context);
            self.reset();
            self.open_library(context);
            return;
        }
        if self.stage == Stage::Library {
            if action == action_id(NEW) {
                self.stage = Stage::Compose;
                self.show(context);
                return;
            }
            if action == action_id(LIBRARY_BACK) {
                self.page = self.page.saturating_sub(1);
                self.show(context);
                return;
            }
            if action == action_id(LIBRARY_NEXT) {
                self.page = (self.page + 1).min(self.pages.len().saturating_sub(1));
                self.show(context);
                return;
            }
            for index in self.pages.get(self.page).cloned().unwrap_or_default() {
                if action == action_id(&play_action(index)) {
                    self.play(context, index);
                    self.show(context);
                    return;
                }
            }
            return;
        }
        if self.stage == Stage::Compose {
            for language in pipeline::LANGUAGES {
                if action == action_id(&language_action(language)) {
                    self.language = language;
                    self.show(context);
                    return;
                }
            }
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
        // First, and returning immediately: a tick is not an answer, and
        // matching it against the task this application is waiting for would
        // report a nap as a provider's reply.
        if self.clock.on_task(context, task, &outcome) {
            if matches!(
                self.stage,
                Stage::Research | Stage::Write | Stage::Narrate | Stage::Package | Stage::Save
            ) {
                self.show(context);
            }
            return;
        }
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
            TaskOutcome::Failed(error) => {
                if error == kobo_sdk::TaskError::NoCredential {
                    let service = service_name(self.secret_wanted());
                    self.fail_as(
                        StandardState::PermissionDenied,
                        format!(
                            "Account details for {service} are missing (secret: {}). Add them, then try again.",
                            self.secret_wanted()
                        ),
                    );
                } else {
                    self.fail_with(Failure::of(error));
                }
            }
            TaskOutcome::Cancelled => self.reset(),
        }
        if !matches!(
            self.stage,
            Stage::Research | Stage::Write | Stage::Narrate | Stage::Package | Stage::Save
        ) {
            self.clock.stop(context);
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let Some(upload) = self.upload.as_mut() {
            match upload.advance(context, &result) {
                ShelfProgress::Moving { done, total } => {
                    self.saved = done;
                    self.total = total;
                    self.show(context);
                    return;
                }
                ShelfProgress::Done => {
                    self.saved = self.total;
                    self.clock.stop(context);
                    self.upload = None;
                    self.tracks.clear();
                    self.parts.clear();
                    // Finished is the one end to an interruption: there is
                    // nothing left to resume once the book is on the shelf.
                    self.checkpoint = None;
                    context.store().forget(CHECKPOINT_KEY);
                    self.remember(context);
                    context.shelf().list();
                    self.open_player(context);
                    self.show(context);
                    return;
                }
                ShelfProgress::Failed(error) => {
                    self.clock.stop(context);
                    self.fail_with(Failure::storing(error));
                    self.show(context);
                    return;
                }
                // Not the upload's answer. It is one of the two the library
                // asks for, so fall through rather than dropping it.
                ShelfProgress::Elsewhere => {}
            }
        }
        match result {
            StoreResult::Loaded { key, value } if key == LIBRARY_KEY => {
                self.titles = parse_index(value.as_deref().unwrap_or_default());
                context.shelf().list();
            }
            StoreResult::Loaded { key, value } if key == CHECKPOINT_KEY => {
                self.checkpoint = value.as_deref().and_then(read_checkpoint);
                if self.stage == Stage::Compose {
                    self.show(context);
                }
            }
            StoreResult::Shelf(blobs) => {
                self.shelf_unreadable = false;
                self.shelved(context, &blobs);
                if self.stage == Stage::Library {
                    self.show(context);
                }
            }
            StoreResult::Denied(_) if self.library.is_none() => {
                self.shelf_unreadable = true;
                self.library = Some(Vec::new());
                if self.stage == Stage::Library {
                    self.show(context);
                }
            }
            _ => {}
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if let kobo_sdk::DeviceRequest::CheckSecrets { .. } = request {
            match result {
                DeviceResult::Secrets { present } => self.checked_secrets(context, &present),
                DeviceResult::Denied(_) | DeviceResult::Failed(_) => self.skip_preflight(context),
                _ => {}
            }
            return;
        }
        if self
            .player
            .as_mut()
            .is_some_and(|player| player.on_device_result(context, &request, &result))
        {
            self.show(context);
        }
    }
}

/// The service one secret pays for, in the words the compose screen used.
fn service_name(secret: &str) -> &'static str {
    match secret {
        "exa" => "research",
        "elevenlabs" => "narration",
        _ => "writing",
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

/// The name of the row that plays the `index`th audiobook.
fn play_action(index: usize) -> String {
    format!("play-{index}")
}

/// The name of the chip that selects a narration language.
fn language_action(language: pipeline::Language) -> String {
    format!("language-{}", language.name())
}

/// The size of a book on the card, in the coarsest honest unit.
fn size_on_disk(bytes: u32) -> String {
    let megabytes = f64::from(bytes) / (1024.0 * 1024.0);
    if megabytes < 1.0 {
        format!("{} KB", (bytes / 1024).max(1))
    } else {
        format!("{megabytes:.0} MB")
    }
}

/// A file name turned back into something to read.
///
/// Only for an audiobook whose title the store lost. It cannot restore
/// capitals or punctuation, and it does not pretend to: it undoes the slug and
/// stops there.
fn title_from_name(name: &str) -> String {
    let stem = name.strip_suffix(ARCHIVE_SUFFIX).unwrap_or(name);
    let words = stem.replace('-', " ");
    let mut title = String::with_capacity(words.len());
    for (index, character) in words.chars().enumerate() {
        if index == 0 {
            title.extend(character.to_uppercase());
        } else {
            title.push(character);
        }
    }
    if title.is_empty() {
        "Audiobook".to_owned()
    } else {
        title
    }
}

/// Reads the saved archive-name-to-title index.
///
/// A malformed line is skipped rather than failing the whole index, because
/// the cost of one unnamed book is one ugly row and the cost of failing is
/// every book unnamed.
fn parse_index(saved: &[u8]) -> Vec<(String, String)> {
    String::from_utf8_lossy(saved)
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(name, title)| !name.is_empty() && !title.is_empty())
        .map(|(name, title)| (name.to_owned(), title.to_owned()))
        .collect()
}

/// One clean line per field: tabs and newlines are the delimiters, so a
/// field that contains one would corrupt the record. Provider text is
/// Latin-script prose; stripping delimiters loses nothing it should carry.
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn write_checkpoint(checkpoint: &Checkpoint) -> Vec<u8> {
    let mut out = format!(
        "v1\t{}\t{}\t{}\t{}\t{}\t{}\n",
        clean(&checkpoint.topic),
        checkpoint.language.name(),
        clean(&checkpoint.title),
        clean(&checkpoint.summary),
        clean(&checkpoint.archive_name),
        checkpoint.next_part
    );
    out.push_str(
        &checkpoint
            .parts
            .iter()
            .map(|part| clean(part))
            .collect::<Vec<_>>()
            .join("\u{1f}"),
    );
    out.into_bytes()
}

/// Reads a stored checkpoint. Anything short of a complete record is no
/// record: a half-written checkpoint resumes nothing, so it offers nothing.
fn read_checkpoint(saved: &[u8]) -> Option<Checkpoint> {
    let text = String::from_utf8_lossy(saved);
    let (header, parts) = text.split_once('\n')?;
    let fields = header.split('\t').collect::<Vec<_>>();
    if fields.len() != 7 || fields[0] != "v1" {
        return None;
    }
    let language = pipeline::LANGUAGES
        .iter()
        .find(|language| language.name() == fields[2])
        .copied()?;
    let parts = parts
        .split('\u{1f}')
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parts.is_empty() || fields[6].parse::<usize>().ok()? > parts.len() {
        return None;
    }
    Some(Checkpoint {
        topic: fields[1].to_owned(),
        language,
        title: fields[3].to_owned(),
        summary: fields[4].to_owned(),
        archive_name: fields[5].to_owned(),
        parts,
        next_part: fields[6].parse().ok()?,
    })
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
    format!(
        "{}{ARCHIVE_SUFFIX}",
        if name.is_empty() { "audiobook" } else { name }
    )
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
    use super::{
        archive_name, parse_index, play_action, read_checkpoint, size_on_disk, title_from_name,
        write_checkpoint, Audiobook, Checkpoint, Saved, Stage,
    };
    use crate::pipeline;
    use kobo_sdk::{action_id, Failure, StandardState, CLARA_BW_METRICS, MAX_ROWS};

    #[test]
    fn a_title_becomes_a_safe_kobo_filename() {
        assert_eq!(archive_name("Moon: Past & Future"), "moon-past-future.mp3z");
    }

    #[test]
    fn compose_progress_complete_and_failure_screens_fit_a_clara() {
        let mut app = Audiobook::default();
        for stage in [Stage::Compose, Stage::Setup, Stage::Research, Stage::Failed] {
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
            stage: Stage::Compose,
            hint: Some("That is too short. Type a few words about the topic."),
            ..Audiobook::default()
        };
        let screen = app.screen();
        let drawn = format!("{screen:?}");
        assert!(drawn.contains("That is too short"), "{drawn}");
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// Every stage the library can be in has to fit, including a shelf with
    /// more audiobooks on it than one panel holds.
    #[test]
    fn every_library_screen_fits_a_clara() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let context = runner.context();
        let mut app = Audiobook::default();
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
        app.library = Some(Vec::new());
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
        let blobs = (0..40)
            .map(|index| (format!("book-{index}.mp3z"), 9_400_000))
            .collect::<Vec<_>>();
        app.titles = blobs
            .iter()
            .map(|(name, _)| {
                (
                    name.clone(),
                    format!("A researched history of the night sky, part {name}"),
                )
            })
            .collect();
        app.shelved(&context, &blobs);
        assert!(app.pages.len() > 1, "40 audiobooks are more than one page");
        for page in 0..app.pages.len() {
            app.page = page;
            let issues = app.screen().validate(&CLARA_BW_METRICS);
            assert!(issues.is_empty(), "page {page}: {issues:?}");
        }
    }

    /// The shelf says what exists and the saved index says what it is called.
    /// A book the index never heard of still has to list, because the bytes
    /// are on the card either way.
    #[test]
    fn the_library_is_the_shelf_named_by_the_index_newest_first() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let context = runner.context();
        let mut app = Audiobook {
            titles: vec![
                ("moon.mp3z".to_owned(), "The Moon".to_owned()),
                ("tides.mp3z".to_owned(), "The Tides".to_owned()),
            ],
            ..Audiobook::default()
        };
        app.shelved(
            &context,
            &[
                ("moon.mp3z".to_owned(), 4_000_000),
                ("stray-recording.mp3z".to_owned(), 1_000_000),
                ("notes.txt".to_owned(), 12),
            ],
        );
        let books = app.library.expect("the shelf answered");
        assert_eq!(books.len(), 2, "{books:?}");
        assert_eq!(books[0].title, "The Moon");
        assert_eq!(books[1].title, "Stray recording");
        assert!(!books.iter().any(|book| book.name == "notes.txt"));
    }

    /// An audiobook took four minutes and fourteen narration calls to make.
    /// Cancelling the next one must not lose it.
    #[test]
    fn a_reset_keeps_what_is_already_on_the_shelf() {
        let mut app = Audiobook {
            stage: Stage::Narrate,
            titles: vec![("moon.mp3z".to_owned(), "The Moon".to_owned())],
            library: Some(vec![Saved {
                name: "moon.mp3z".to_owned(),
                title: "The Moon".to_owned(),
                bytes: 4_000_000,
            }]),
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        app.reset();
        assert_eq!(app.stage, Stage::Library);
        assert!(app.topic.text().is_empty());
        assert_eq!(app.library.expect("the library survived").len(), 1);
        assert_eq!(app.titles.len(), 1);
    }

    #[test]
    fn the_index_survives_a_round_trip_and_ignores_a_broken_line() {
        let saved = "moon.mp3z\tThe Moon\nrubbish\ntides.mp3z\tThe Tides\n";
        assert_eq!(
            parse_index(saved.as_bytes()),
            vec![
                ("moon.mp3z".to_owned(), "The Moon".to_owned()),
                ("tides.mp3z".to_owned(), "The Tides".to_owned()),
            ]
        );
        assert!(parse_index(b"").is_empty());
    }

    #[test]
    fn a_lost_title_falls_back_to_the_file_name() {
        assert_eq!(title_from_name("moon-past-future.mp3z"), "Moon past future");
        assert_eq!(title_from_name(".mp3z"), "Audiobook");
    }

    #[test]
    fn a_size_is_reported_in_the_coarsest_honest_unit() {
        assert_eq!(size_on_disk(9_437_184), "9 MB");
        assert_eq!(size_on_disk(4_096), "4 KB");
        assert_eq!(size_on_disk(0), "1 KB");
    }

    /// Every row on the library has its own action, or tapping one book plays
    /// another.
    #[test]
    fn each_row_has_its_own_action() {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..MAX_ROWS {
            assert!(seen.insert(action_id(&play_action(index))), "{index}");
        }
    }

    /// A topic submit asks the runtime which accounts are installed rather
    /// than spending first and finding out later.
    #[test]
    fn a_submit_checks_every_account_before_spending() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Compose,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        app.begin(&mut context);
        assert_eq!(app.stage, Stage::Setup);
        assert!(app.task.is_none(), "nothing is spent during the check");
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// The preflight answer names every missing account at once, while the
    /// reader has spent nothing.
    #[test]
    fn a_missing_account_is_named_before_anything_is_spent() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Setup,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        app.checked_secrets(&mut context, &["openai".to_owned()]);
        assert_eq!(app.stage, Stage::Failed);
        let (state, advice) = app.trouble.clone().expect("a failure was recorded");
        assert_eq!(state, StandardState::PermissionDenied);
        assert!(advice.contains("research"), "{advice}");
        assert!(advice.contains("narration"), "{advice}");
        assert!(!advice.contains("writing"), "{advice}");
        assert!(advice.contains("exa"), "{advice}");
        assert!(advice.contains("elevenlabs"), "{advice}");
        assert!(app.task.is_none(), "nothing was spent");
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// A runtime that cannot answer the check leaves the flow to find a
    /// missing key at the stage that spends it, the way it always has.
    #[test]
    fn an_unanswerable_preflight_defers_to_the_stages() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Setup,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        app.skip_preflight(&mut context);
        assert_eq!(app.stage, Stage::Research);
    }

    /// A checkpoint carries the script and the stopping point, and only a
    /// complete record comes back.
    #[test]
    fn a_checkpoint_round_trips_and_rejects_short_records() {
        let checkpoint = Checkpoint {
            topic: "the moon".to_owned(),
            language: pipeline::Language::default(),
            title: "The Moon".to_owned(),
            summary: "A tour of the night sky.".to_owned(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec![
                "Part one, spoken.".to_owned(),
                "Part two, spoken.".to_owned(),
                "Part three, spoken.".to_owned(),
            ],
            next_part: 2,
        };
        let saved = write_checkpoint(&checkpoint);
        assert_eq!(read_checkpoint(&saved), Some(checkpoint.clone()));
        assert_eq!(read_checkpoint(b""), None);
        assert_eq!(read_checkpoint(b"v1\tonly\tthree\tfields\npart"), None);
        // A stopping point past the parts is no record at all.
        let text = String::from_utf8(write_checkpoint(&checkpoint)).unwrap();
        let broken = text.replace("\t2\n", "\t9\n");
        assert_eq!(read_checkpoint(broken.as_bytes()), None);
    }

    /// Narration interrupted at part three of five resumes at part three,
    /// not from the beginning: the calls already paid for are not made again.
    #[test]
    fn an_interrupted_narration_resumes_where_it_stopped() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Failed,
            title: "The Moon".to_owned(),
            parts: vec![
                "one".to_owned(),
                "two".to_owned(),
                "three".to_owned(),
                "four".to_owned(),
                "five".to_owned(),
            ],
            next_part: 2,
            ..Audiobook::default()
        };
        app.resume(&mut context);
        assert_eq!(app.stage, Stage::Narrate);
        assert_eq!(
            app.next_part, 2,
            "part three is asked for again, not part one"
        );
        assert!(app.task.is_some(), "narration is back in flight");
    }

    /// After a restart only the script survives, so a stored checkpoint
    /// resumes the narration from its first part - not from a part whose
    /// audio is gone.
    #[test]
    fn a_stored_checkpoint_resumes_the_script_from_the_top() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let checkpoint = Checkpoint {
            topic: "the moon".to_owned(),
            language: pipeline::Language::default(),
            title: "The Moon".to_owned(),
            summary: String::new(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            next_part: 2,
        };
        let mut app = Audiobook {
            stage: Stage::Compose,
            checkpoint: Some(checkpoint),
            ..Audiobook::default()
        };
        app.resume(&mut context);
        assert_eq!(app.stage, Stage::Narrate);
        assert_eq!(app.next_part, 0, "the audio is gone, so the parts restart");
        assert_eq!(app.title, "The Moon");
        assert_eq!(app.parts.len(), 3);
    }

    /// The failure screen leads with resuming when there is something to
    /// resume, and does not offer it when there is not.
    #[test]
    fn the_failed_screen_offers_resume_only_when_it_can() {
        let mut app = Audiobook {
            stage: Stage::Failed,
            trouble: Some((StandardState::Error, "The request failed.".to_owned())),
            ..Audiobook::default()
        };
        let drawn = format!("{:?}", app.screen());
        assert!(!drawn.contains("Resume"), "{drawn}");
        app.parts = vec!["one".to_owned()];
        let drawn = format!("{:?}", app.screen());
        assert!(drawn.contains("Resume"), "{drawn}");
        assert!(drawn.contains("Start over"), "{drawn}");
        assert!(app.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// Missing account setup should use the shared customer-facing remedy.
    #[test]
    fn missing_account_setup_uses_consumer_copy() {
        for stage in [Stage::Research, Stage::Write, Stage::Narrate] {
            let mut app = Audiobook {
                stage,
                ..Audiobook::default()
            };
            app.fail_with(Failure::of(kobo_sdk::TaskError::NoCredential));
            let (state, advice) = app.trouble.clone().expect("a failure was recorded");
            assert_eq!(state, StandardState::PermissionDenied);
            assert_eq!(advice, "Add account details to connect this service.");
        }
    }
}
