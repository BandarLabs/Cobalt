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

/// Where the title of each finished audiobook is kept.
///
/// The shelf stores bytes under a file name, and a file name is a slug: it
/// cannot carry "The Moon's Past and Future" back out again. So the archive
/// name is mapped to the title here, in the ordinary key-value store, which
/// lives beside the application and survives a restart exactly as the shelf
/// does. The shelf remains the truth about what exists; this only says what
/// each thing is called.
const LIBRARY_KEY: &str = "library";

/// One finished audiobook, on the reader, playable with the network off.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Saved {
    name: String,
    title: String,
    bytes: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Stage {
    /// What is already on the reader. The application opens here rather than
    /// on the composer, because an audiobook that took four minutes and
    /// fourteen narration calls to make is worth more than the next one.
    #[default]
    Library,
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
                    screen = screen.top_bar_action(SHELF, "Audiobooks");
                }
                let mut screen = screen
                    .heading("What should it be about?")
                    .text("It is researched from current sources, written as an original spoken script, and narrated aloud. The finished audiobook stays on this reader and plays with the network off.");
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
                let mut screen = ScreenBuilder::new("audiobook-failed")
                    .top_bar("Could not create audiobook")
                    .standard_state(state, advice);
                if self.has_books() {
                    screen =
                        screen.buttons([(AGAIN, "Try another topic"), (SHELF, "Your audiobooks")]);
                } else {
                    screen = screen.button(AGAIN, "Try another topic");
                }
                screen.build()
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
        let screen = screen.top_bar_action(NEW, "Create");
        let showing = self.pages.get(self.page).map_or(&[][..], Vec::as_slice);
        let mut turning = screen
            .rows_with_trailing(showing.iter().filter_map(|index| {
                books.get(*index).map(|book| {
                    (
                        play_action(*index),
                        book.title.clone(),
                        String::new(),
                        Glyph::Book,
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
            .secondary_action(AGAIN, "Create another")
            .owns_back(true);
        player.set_cover(cover);
        player.start(context);
        self.player = Some(player);
        self.stage = Stage::Player;
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
            Stage::Library | Stage::Compose | Stage::Player | Stage::Failed => ("Preparing", 0),
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
        // One clock for the whole creation rather than one per stage. What
        // somebody waiting wants to know is how long they have been waiting,
        // not how long this particular provider has.
        self.clock.start(context);
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
        *self = Self {
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
        if action == action_id(AGAIN) {
            if self.player.is_some() {
                context.device().stop_audio();
            }
            self.clock.stop(context);
            self.reset();
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
            TaskOutcome::Failed(error) => self.fail_with(Failure::of(error)),
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

/// The name of the row that plays the `index`th audiobook.
fn play_action(index: usize) -> String {
    format!("play-{index}")
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
        archive_name, parse_index, play_action, size_on_disk, title_from_name, Audiobook, Saved,
        Stage,
    };
    use kobo_sdk::{action_id, Failure, StandardState, CLARA_BW_METRICS, MAX_ROWS};

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

    /// A missing API key is not a permission the application lacks, and the
    /// screen has to say which thing is actually wrong.
    #[test]
    fn a_missing_key_names_the_key_rather_than_blaming_the_application() {
        // Each stage asks a different provider, so the sentence has to name the
        // one that was actually missing rather than "that service".
        for (stage, key) in [
            (Stage::Research, "exa"),
            (Stage::Write, "openai"),
            (Stage::Narrate, "elevenlabs"),
        ] {
            let mut app = Audiobook {
                stage,
                ..Audiobook::default()
            };
            app.fail_with(Failure::of(kobo_sdk::TaskError::NoCredential));
            let (state, advice) = app.trouble.clone().expect("a failure was recorded");
            assert_eq!(state, StandardState::PermissionDenied);
            assert!(advice.contains(key), "{stage:?}: {advice}");
            assert!(
                advice.contains(&format!("kobo secret set {key}")),
                "{stage:?}: {advice}"
            );
            assert!(
                !advice.contains("does not hold this permission"),
                "{advice}"
            );
        }
    }
}
