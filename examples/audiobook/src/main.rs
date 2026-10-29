//! On-demand, researched audiobooks for the Kobo library.

mod pipeline;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id,
    audio::{AudioMetadata, AudioPlayer},
    Context, DeviceRequest, DeviceResult, Failure, Glyph, Heartbeat, KoboApp, LogLevel,
    PictureHandle, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StandardState,
    StoreResult, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const AGAIN: &str = "again";
const CANCEL: &str = "cancel";
const NEW: &str = "new";
/// Asks the runtime again which keys are installed, after the owner has run
/// the commands the keys screen named.
const CHECK: &str = "check";
/// Back to the composer with the topic still typed.
const EDIT: &str = "edit";
/// Spends: the one control that starts the paid requests.
const START: &str = "start";
/// The firmware's container for a sideloaded audiobook.
const ARCHIVE_SUFFIX: &str = ".mp3z";
const SHELF: &str = "shelf";
const LIBRARY_BACK: &str = "library-back";
const LIBRARY_NEXT: &str = "library-next";

/// The three accounts one audiobook spends, checked before the first spend.
const SECRETS: &[&str] = &["exa", "openai", "elevenlabs"];
const RESUME: &str = "resume";
const SAMPLE: &str = "sample";
/// The sample's place in the library once it is saved.
const SAMPLE_NAME: &str = "the-quiet-shelf.mp3z";
const SAMPLE_TITLE: &str = "The Quiet Shelf";
/// The free sample: fifty-two seconds of original narration, made for this
/// application and shipped inside it, so the player can be tried before any
/// account exists. Encoded exactly as a narrated book is (MP3, 44.1 kHz,
/// 128 kbps), because a sample that plays differently proves nothing.
const SAMPLE_BYTES: &[u8] = include_bytes!("../assets/sample.mp3");

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
/// A checkpoint is text only: the script and how many of its parts are
/// narrated and safe on the shelf. The audio of each part is written to the
/// shelf as it arrives, under [`part_name`], so a restart reads those parts
/// back rather than paying to narrate them a second time. The parts go once
/// the finished book is saved, or once the owner starts a different one.
const CHECKPOINT_KEY: &str = "checkpoint";

/// The largest narrated part read back after a restart. A part is a thousand
/// characters of speech, about a megabyte of MP3, and narration refuses
/// anything over four.
const MAX_PART_BYTES: usize = 4 * 1024 * 1024;

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
    /// A key is missing. The screen names the command that installs it and
    /// keeps the topic, so fixing it is one command and one tap.
    Keys,
    /// Every key is there. What the book will cost and how long it will
    /// take, shown before the first paid request rather than after.
    Confirm,
    Research,
    Write,
    Narrate,
    /// Reading parts narrated before a restart back off the shelf.
    Restore,
    Package,
    Save,
    Player,
    Failed,
}

/// Whether narrated parts are written to the shelf as they arrive.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Keeping {
    #[default]
    Parts,
    /// The shelf refused a part. The book still finishes from memory; only
    /// a restart in the middle of it would pay for those parts again.
    Refused,
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
    /// Whether trying the same topic again could help, so the failure
    /// screen can offer that before a new topic.
    retryable: bool,
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
    /// The upload in flight is the free sample, not a creation: saving it
    /// must not retire somebody's interrupted book.
    saving_sample: bool,
    /// The accounts the keys screen asks for.
    missing: Vec<&'static str>,
    /// The narrated part being written to the shelf. One at a time, and
    /// never beside the finished book's own upload, so every store answer
    /// belongs to exactly one transfer.
    part_upload: Option<ShelfUpload>,
    /// How many narrated parts are safe on the shelf, which is what the
    /// checkpoint promises a restart.
    saved_parts: usize,
    /// Whether narrated parts are still being written to the shelf.
    keeping: Keeping,
    /// The part being read back after a restart.
    part_download: Option<ShelfDownload>,
    /// How many saved parts a resume is reading back.
    restoring: usize,
}

impl Audiobook {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context));
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.stage {
            Stage::Library => self.library_screen(),
            Stage::Compose => {
                let mut screen = ScreenBuilder::new("audiobook-compose")
                    .top_bar("Create an audiobook")
                    .owns_back(true);
                if self.has_books() {
                    screen = screen.top_bar_glyph(SHELF, "Audiobooks", Glyph::Headphones);
                }
                if self.checkpoint.is_some() {
                    screen = screen.top_bar_action(RESUME, "Resume");
                }
                let mut screen = screen.heading("Your topic");
                if let Some(hint) = self.hint {
                    screen = screen.secondary(hint);
                } else if let Some(checkpoint) = &self.checkpoint {
                    let title = if checkpoint.title.is_empty() {
                        "Interrupted audiobook"
                    } else {
                        &checkpoint.title
                    };
                    screen = screen.secondary(context.one_line_row(
                        &format!(
                            "Resume part {} of {}: {title}",
                            checkpoint.next_part + 1,
                            checkpoint.parts.len().max(1)
                        ),
                        false,
                    ));
                } else {
                    screen = screen.secondary("Saved here to play offline.");
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
            Stage::Keys => self.keys_screen(),
            Stage::Confirm => confirm_screen(),
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
        // Where a resume would carry on, said in the message rather than on
        // the button. A title on the button wrapped onto three lines and
        // squeezed its neighbours.
        let resume_from = if self.parts.is_empty() {
            self.checkpoint
                .as_ref()
                .map(|checkpoint| (checkpoint.next_part, checkpoint.parts.len()))
        } else {
            Some((self.next_part, self.parts.len()))
        };
        let message = match resume_from {
            Some((next, total)) => format!(
                "{advice} Resume carries on from part {} of {total}.",
                (next + 1).min(total.max(1))
            ),
            None => advice.to_owned(),
        };
        let screen = ScreenBuilder::new("audiobook-failed")
            .top_bar("Could not create audiobook")
            .standard_state(state, message);
        // Resume first because it is the cheaper way forward. Then trying
        // the same topic again when that could help, which keeps the topic:
        // "Try another topic" was the only way out of a dropped connection
        // and it threw away what had been typed.
        let (first, second) = match (resume_from.is_some(), self.retryable) {
            (true, _) => ((RESUME, "Resume"), (AGAIN, "Start over")),
            (false, true) => ((CHECK, "Try again"), (AGAIN, "New topic")),
            (false, false) => ((AGAIN, "New topic"), (SHELF, "Your audiobooks")),
        };
        let screen = screen.primary_button(first.0, first.1);
        if second.0 != SHELF && self.has_books() {
            screen.buttons([second, (SHELF, "Your audiobooks")]).build()
        } else if second.0 == SHELF && !self.has_books() {
            screen.build()
        } else {
            screen.button(second.0, second.1).build()
        }
    }

    /// Which keys are missing and the exact command that installs each.
    ///
    /// This used to be a failure screen with one way out, "Try another
    /// topic", which threw the topic away and led straight back to the same
    /// refusal. Now it names the command, keeps the topic, and asks again.
    fn keys_screen(&self) -> Screen {
        let missing = self
            .missing
            .iter()
            .map(|secret| provider_name(secret))
            .collect::<Vec<_>>();
        let named = match missing.as_slice() {
            [] => String::new(),
            [only] => (*only).to_owned(),
            [first @ .., last] => format!("{} or {last}", first.join(", ")),
        };
        let mut screen = ScreenBuilder::new("audiobook-keys")
            .top_bar("Add your keys")
            .owns_back(true)
            .secondary(format!(
                "Exa researches, OpenAI writes and ElevenLabs narrates, each on your own \
                 account. This reader has no key for {named} yet. On your computer, run:"
            ));
        for secret in &self.missing {
            screen = screen.command(format!("kobo secret set {secret}"));
        }
        screen
            .secondary(format!(
                "Each command reads its key from a file such as ~/.{}. Nothing has been spent.",
                self.missing.first().copied().unwrap_or("exa")
            ))
            .primary_button(CHECK, "Check again")
            .button(EDIT, "Edit topic")
            .build()
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
            return screen
                .primary_button(NEW, "Create an audiobook")
                .button(SAMPLE, "Play the sample")
                .build();
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

    /// The free sample, playable before any account exists.
    ///
    /// Once saved it is an ordinary book on the shelf and lists like one.
    /// Before that, saving it is the whole errand: the same upload and the
    /// same player a created book uses, because a sample that took a
    /// shortcut would prove the shortcut, not the book.
    fn play_sample(&mut self, context: &mut Context) {
        SAMPLE_TITLE.clone_into(&mut self.title);
        SAMPLE_NAME.clone_into(&mut self.archive_name);
        let already_saved = self
            .library
            .as_ref()
            .is_some_and(|books| books.iter().any(|book| book.name == SAMPLE_NAME));
        if already_saved {
            self.open_player(context);
            return;
        }
        let bytes = match kobo_doc::zip::stored(&[("001.mp3".to_owned(), SAMPLE_BYTES.to_vec())]) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fail(format!("Could not package the sample: {error}"));
                return;
            }
        };
        self.total = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        self.saved = 0;
        self.saving_sample = true;
        let mut upload = ShelfUpload::new(self.archive_name.clone(), bytes);
        upload.start(context);
        self.upload = Some(upload);
        self.stage = Stage::Save;
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
    /// parts that reached the shelf are read back first, and narration picks
    /// up after the last of them. It used to begin again from the first
    /// part, which paid for every part twice.
    fn resume(&mut self, context: &mut Context) {
        if self.parts.is_empty() {
            let Some(checkpoint) = self.checkpoint.clone() else {
                return;
            };
            self.topic = Keyboard::with_text(&checkpoint.topic);
            self.language = checkpoint.language;
            self.title = checkpoint.title;
            self.summary = checkpoint.summary;
            self.archive_name = checkpoint.archive_name;
            self.parts = checkpoint.parts;
            self.next_part = 0;
            self.saved_parts = 0;
            self.keeping = Keeping::Parts;
            self.tracks.clear();
            self.restoring = checkpoint.next_part.min(self.parts.len());
        }
        if self.parts.is_empty() {
            return;
        }
        self.upload = None;
        self.trouble = None;
        self.clock.start(context);
        if self.tracks.len() < self.restoring {
            self.stage = Stage::Restore;
            self.read_next_part(context);
        } else {
            self.stage = Stage::Narrate;
            self.start_next_voice(context);
        }
    }

    /// Asks the shelf for the next part narrated before a restart.
    fn read_next_part(&mut self, context: &mut Context) {
        let mut download = ShelfDownload::new(part_name(&self.archive_name, self.tracks.len()))
            .at_most(MAX_PART_BYTES);
        download.start(context);
        self.part_download = Some(download);
    }

    /// One saved part is back in memory, exactly as it was narrated.
    fn restored_part(&mut self, context: &mut Context, audio: Vec<u8>) {
        if audio.len() < 256 {
            self.restored(context);
            return;
        }
        self.tracks
            .push((format!("{:03}.mp3", self.tracks.len() + 1), audio));
        if self.tracks.len() < self.restoring {
            self.read_next_part(context);
        } else {
            self.restored(context);
        }
    }

    /// Narrates whatever the shelf could not give back.
    ///
    /// Also the answer to a part that is missing or damaged: the parts
    /// before it are kept and narration starts at it, which pays only for
    /// what was actually lost.
    fn restored(&mut self, context: &mut Context) {
        self.part_download = None;
        self.restoring = 0;
        self.next_part = self.tracks.len();
        self.saved_parts = self.tracks.len();
        self.stage = Stage::Narrate;
        self.start_next_voice(context);
    }

    /// Records how far an interrupted creation got, at each boundary where
    /// that answer changes: the script written, and each part saved.
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            topic: self.topic.text().to_owned(),
            language: self.language,
            title: self.title.clone(),
            summary: self.summary.clone(),
            archive_name: self.archive_name.clone(),
            parts: self.parts.clone(),
            next_part: self.saved_parts,
        }
    }

    /// Writes the checkpoint, and keeps the copy in memory in step with it.
    ///
    /// The copy in memory is what a cancel leaves on the composer. When only
    /// the store was updated, a cancel followed by Resume read the copy made
    /// when the script arrived and narrated from the first part again.
    fn save_checkpoint(&mut self, context: &mut Context) {
        let checkpoint = self.checkpoint();
        context
            .store()
            .save(CHECKPOINT_KEY, write_checkpoint(&checkpoint));
        self.checkpoint = Some(checkpoint);
    }

    /// Forgets an interrupted creation and the parts it saved.
    ///
    /// Covers both the creation in memory and the one the store offered,
    /// which are usually the same book and are removed by name either way.
    fn retire_checkpoint(&mut self, context: &mut Context) {
        let mut books = Vec::new();
        if !self.parts.is_empty() {
            books.push((self.archive_name.clone(), self.parts.len()));
        }
        if let Some(checkpoint) = self.checkpoint.take() {
            books.push((checkpoint.archive_name, checkpoint.parts.len()));
        }
        books.dedup();
        for (archive_name, parts) in books {
            for index in 0..parts {
                context.shelf().remove(part_name(&archive_name, index));
            }
        }
        context.store().forget(CHECKPOINT_KEY);
    }

    fn progress(&self) -> (String, u8) {
        match self.stage {
            Stage::Setup => ("Checking account setup".to_owned(), 5),
            Stage::Research => ("Researching the topic".to_owned(), 10),
            Stage::Write => ("Writing the spoken script".to_owned(), 30),
            Stage::Restore => (
                format!(
                    "Reading back part {} of {}",
                    (self.tracks.len() + 1).min(self.restoring.max(1)),
                    self.restoring.max(1)
                ),
                35,
            ),
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
            Stage::Library
            | Stage::Compose
            | Stage::Keys
            | Stage::Confirm
            | Stage::Player
            | Stage::Failed => ("Preparing".to_owned(), 0),
        }
    }

    fn begin(&mut self, context: &mut Context) {
        let topic = self.topic.text().trim();
        if topic.len() < 3 {
            self.hint = Some("That is too short. Add a few words.");
            self.stage = Stage::Compose;
            self.show(context);
            return;
        }
        self.stage = Stage::Setup;
        self.hint = None;
        self.trouble = None;
        // Every account this book will spend is checked before the first
        // request goes out. Research, writing and narration each have their
        // own key, and finding out the narration key is missing after the
        // research was paid for is the failure this stage exists to prevent.
        context.secrets().check(SECRETS);
        self.show(context);
    }

    /// The answer to the preflight: show what the book will cost, or say
    /// exactly which keys are missing while nothing has been spent yet.
    fn checked_secrets(&mut self, context: &mut Context, present: &[String]) {
        if self.stage != Stage::Setup {
            return;
        }
        self.missing = SECRETS
            .iter()
            .copied()
            .filter(|name| !present.iter().any(|held| held == name))
            .collect();
        self.stage = if self.missing.is_empty() {
            Stage::Confirm
        } else {
            Stage::Keys
        };
        self.show(context);
    }

    /// The check itself could not run, so the flow continues and lets each
    /// stage report its own missing key the way it always has. An old
    /// runtime answers this way; it is a degraded path, not a failure, and
    /// the estimate is still shown before anything is spent.
    fn skip_preflight(&mut self, context: &mut Context) {
        if self.stage != Stage::Setup {
            return;
        }
        self.stage = Stage::Confirm;
        self.show(context);
    }

    /// The owner has read the estimate and chosen to spend.
    fn start_creation(&mut self, context: &mut Context) {
        // A new creation supersedes an interrupted one: the person read the
        // offer to resume it and chose a fresh topic instead.
        self.retire_checkpoint(context);
        self.stage = Stage::Research;
        // One clock for the whole creation rather than one per stage. What
        // somebody waiting wants to know is how long they have been waiting,
        // not how long this particular provider has.
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
                    self.saved_parts = 0;
                    self.keeping = Keeping::Parts;
                    self.tracks.clear();
                    // The script is the expensive thing two providers made;
                    // from here an interruption has something worth keeping.
                    self.save_checkpoint(context);
                    self.start_next_voice(context);
                }
            }
            Err(error) => self.fail(error),
        }
        self.show(context);
    }

    fn start_next_voice(&mut self, context: &mut Context) {
        let Some(text) = self.parts.get(self.next_part) else {
            self.package_when_saved(context);
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
        self.save_next_part(context);
        self.start_next_voice(context);
        self.show(context);
    }

    /// Writes the oldest narrated part that is not on the shelf yet.
    ///
    /// The checkpoint moves only once a part is safely written, so it never
    /// promises a restart audio that is not there.
    fn save_next_part(&mut self, context: &mut Context) {
        if self.part_upload.is_some() || self.keeping == Keeping::Refused {
            return;
        }
        let Some((_, audio)) = self.tracks.get(self.saved_parts) else {
            return;
        };
        let mut upload = ShelfUpload::new(
            part_name(&self.archive_name, self.saved_parts),
            audio.clone(),
        );
        upload.start(context);
        self.part_upload = Some(upload);
    }

    /// Packages the book once its last part is narrated and no part is
    /// still being written, so the book's own upload never shares the shelf
    /// with a part's.
    fn package_when_saved(&mut self, context: &mut Context) {
        if self.part_upload.is_none() {
            self.package(context);
        }
    }

    /// A tap on the shelf: the sample, a new book, a page turn or a book.
    fn on_library_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
        if action == action_id(SAMPLE) {
            self.play_sample(context);
            self.show(context);
            return;
        }
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
    }

    /// A tap on the keys screen or the estimate, the two screens between
    /// typing a topic and paying for it. Back and Edit topic both return to
    /// the composer with the topic still typed.
    fn before_spending(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
        if action == kobo_sdk::ActionId::BACK || action == action_id(EDIT) {
            self.stage = Stage::Compose;
            self.show(context);
        } else if action == action_id(CHECK) && self.stage == Stage::Keys {
            self.begin(context);
        } else if action == action_id(START) && self.stage == Stage::Confirm {
            self.start_creation(context);
        }
    }

    /// Feeds a store answer to a part being written or read back. Returns
    /// whether it was one.
    fn part_transfer(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        if let Some(download) = self.part_download.as_mut() {
            match download.advance(context, result) {
                ShelfProgress::Elsewhere => {}
                ShelfProgress::Moving { .. } => return true,
                ShelfProgress::Done => {
                    let audio = self
                        .part_download
                        .take()
                        .map(ShelfDownload::take)
                        .unwrap_or_default();
                    self.restored_part(context, audio);
                    self.show(context);
                    return true;
                }
                ShelfProgress::Failed(_) => {
                    self.restored(context);
                    self.show(context);
                    return true;
                }
            }
        }
        if let Some(upload) = self.part_upload.as_mut() {
            match upload.advance(context, result) {
                ShelfProgress::Elsewhere => {}
                ShelfProgress::Moving { .. } => return true,
                // Saving a part changes nothing on the panel, so the panel
                // is redrawn only when the book moves on to being packaged.
                ShelfProgress::Done => {
                    let stage = self.stage;
                    self.part_upload = None;
                    self.saved_parts += 1;
                    self.save_checkpoint(context);
                    self.save_next_part(context);
                    self.package_if_ready(context);
                    if self.stage != stage {
                        self.show(context);
                    }
                    return true;
                }
                ShelfProgress::Failed(error) => {
                    context.log(
                        LogLevel::Warn,
                        format!("audiobook part not kept for resuming: {error}"),
                    );
                    let stage = self.stage;
                    self.part_upload = None;
                    self.keeping = Keeping::Refused;
                    self.package_if_ready(context);
                    if self.stage != stage {
                        self.show(context);
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Picks up a book whose last part was waiting on a part write.
    fn package_if_ready(&mut self, context: &mut Context) {
        if self.stage == Stage::Narrate
            && self.task.is_none()
            && !self.parts.is_empty()
            && self.next_part >= self.parts.len()
        {
            self.package_when_saved(context);
        }
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
        // A cancel stops the work, not the record of it: the checkpoint the
        // store holds stays offered, here as much as after a restart.
        let checkpoint = self.checkpoint.take();
        // A person who narrates in Hindi will narrate in Hindi again.
        let language = self.language;
        *self = Self {
            language,
            library,
            titles,
            pages,
            shelf_unreadable,
            checkpoint,
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

    /// A provider's refusal, told in that provider's name with the command
    /// that fixes its key.
    ///
    /// Three providers, three keys, and the stage that failed is exactly the
    /// thing that knows which one. The shared wording is kept for every
    /// cause but one: these endpoints are fixed, so a refusal that is not
    /// about the key or the pace is never "check the server address".
    fn fail_task(&mut self, error: kobo_sdk::TaskError) {
        let secret = self.secret_wanted();
        let name = provider_name(secret);
        let fix = format!("Replace the key from your computer: kobo secret set {secret}.");
        let problem = kobo_sdk::failure::Service::new(name)
            .fixed_by(&fix)
            .explain(error);
        let retryable = problem.retryable;
        let message = if problem.cause == kobo_sdk::failure::Cause::NotFound {
            format!(
                "{name} turned the request down. The account may be out of credit, or {name} \
                 may have retired what this app asks for. Check the account, or check Store \
                 for an update."
            )
        } else {
            problem.message
        };
        self.fail_as(problem.state, message);
        self.retryable = retryable;
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
        self.retryable = false;
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
        if self.stage == Stage::Compose
            && (action == kobo_sdk::ActionId::BACK || action == action_id(SHELF))
        {
            // Navigation does not discard an unsubmitted topic or language.
            self.open_library(context);
            return;
        }
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
        if action == action_id(CHECK) && self.stage == Stage::Failed {
            self.clock.stop(context);
            self.begin(context);
            return;
        }
        if action == action_id(AGAIN) {
            if self.player.is_some() {
                context.device().stop_audio();
            }
            self.clock.stop(context);
            // Starting over from a failure retires what was interrupted:
            // the person has seen it fail and chosen a different book.
            // "Create another" under the player does not, because nothing
            // about finishing one book says another should be thrown away.
            if self.stage == Stage::Failed {
                self.retire_checkpoint(context);
            }
            self.reset();
            self.stage = Stage::Compose;
            self.show(context);
            return;
        }
        if matches!(self.stage, Stage::Keys | Stage::Confirm) {
            self.before_spending(context, action);
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
            self.on_library_action(context, action);
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
                    self.hint = None;
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
                Stage::Research
                    | Stage::Write
                    | Stage::Narrate
                    | Stage::Restore
                    | Stage::Package
                    | Stage::Save
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
                let secret = self.secret_wanted();
                if error == kobo_sdk::TaskError::NoCredential && self.parts.is_empty() {
                    // Nothing worth keeping has been made yet, so this is
                    // the same moment as the preflight and gets its screen:
                    // the command, and a way to check again.
                    self.task = None;
                    self.missing = vec![secret];
                    self.stage = Stage::Keys;
                } else if error == kobo_sdk::TaskError::NoCredential {
                    self.fail_as(
                        StandardState::PermissionDenied,
                        format!(
                            "This reader has no {} key, which {} needs. On your computer, run \
                             kobo secret set {secret}, then tap Resume.",
                            provider_name(secret),
                            service_name(secret)
                        ),
                    );
                } else {
                    self.fail_task(error);
                }
            }
            TaskOutcome::Cancelled => self.reset(),
        }
        if !matches!(
            self.stage,
            Stage::Research
                | Stage::Write
                | Stage::Narrate
                | Stage::Restore
                | Stage::Package
                | Stage::Save
        ) {
            self.clock.stop(context);
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.part_transfer(context, &result) {
            return;
        }
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
                    // Finished is the one end to an interruption: there is
                    // nothing left to resume once the book is on the shelf,
                    // and the parts it was saved in are inside it now. The
                    // sample is not a creation, so saving it retires nothing.
                    if !self.saving_sample {
                        self.retire_checkpoint(context);
                    }
                    self.tracks.clear();
                    self.parts.clear();
                    self.saving_sample = false;
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

/// What a new book will cost and how long it will take, before anything is
/// spent.
///
/// The figures are ranges and say so. The script's length is the writer's
/// choice and every account is on its own plan, so a single number would be
/// a promise this application cannot keep; a range it can.
fn confirm_screen() -> Screen {
    let estimate = pipeline::Estimate::for_a_book();
    let (short, long) = estimate.narration_thousands();
    ScreenBuilder::new("audiobook-confirm")
        .top_bar("Before you start")
        .owns_back(true)
        .facts([
            (
                "Time",
                format!("{} to {} minutes", estimate.minutes.0, estimate.minutes.1),
            ),
            (
                "Cost",
                format!("${} to ${}", estimate.dollars.0, estimate.dollars.1),
            ),
            (
                "Narration",
                format!("{short} to {long} thousand ElevenLabs characters"),
            ),
            (
                "Also",
                "One Exa research run and one OpenAI script".to_owned(),
            ),
        ])
        .secondary(
            "An estimate, billed to your own accounts. If it stops part way, Resume \
             does not pay again for what is done.",
        )
        .primary_button(START, "Create audiobook")
        .button(EDIT, "Edit topic")
        .build()
}

/// The company behind one secret, as the owner knows it.
fn provider_name(secret: &str) -> &'static str {
    match secret {
        "exa" => "Exa",
        "elevenlabs" => "ElevenLabs",
        _ => "OpenAI",
    }
}

/// The shelf name a narrated part waits under until the book is saved.
///
/// Not an `.mp3`, because this shelf is the folder the Kobo reader looks in
/// for audiobooks, and a part must never be mistaken for a book.
fn part_name(archive_name: &str, index: usize) -> String {
    let stem = archive_name
        .strip_suffix(ARCHIVE_SUFFIX)
        .unwrap_or(archive_name);
    format!("{stem}.part-{:03}", index + 1)
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
        write_checkpoint, Audiobook, Checkpoint, Saved, Stage, CHECK, CHECKPOINT_KEY, SAMPLE_NAME,
        SAMPLE_TITLE, SECRETS,
    };
    use crate::pipeline;
    use kobo_sdk::{action_id, StandardState, CLARA_BW_METRICS, MAX_ROWS};
    use kobo_sdk::{Context, KoboApp, StoreResult};

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
            let issues = app
                .screen(&kobo_sdk::Context::default())
                .validate(&CLARA_BW_METRICS);
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
            hint: Some("That is too short. Add a few words."),
            ..Audiobook::default()
        };
        let screen = app.screen(&kobo_sdk::Context::default());
        let drawn = format!("{screen:?}");
        assert!(drawn.contains("That is too short"), "{drawn}");
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
    }

    /// Every stage the library can be in has to fit, including a shelf with
    /// more audiobooks on it than one panel holds.
    #[test]
    fn every_library_screen_fits_a_clara() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let context = runner.context();
        let mut app = Audiobook::default();
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
        app.library = Some(Vec::new());
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
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
            let issues = app
                .screen(&kobo_sdk::Context::default())
                .validate(&CLARA_BW_METRICS);
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
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
    }

    /// The preflight answer names every missing key at once, with the exact
    /// command for each, while the reader has spent nothing.
    #[test]
    fn a_missing_key_is_named_with_the_command_that_installs_it() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Setup,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        app.checked_secrets(&mut context, &["openai".to_owned()]);
        assert_eq!(app.stage, Stage::Keys);
        assert!(app.task.is_none(), "nothing was spent");
        let drawn = format!("{:?}", app.screen(&Context::default()));
        assert!(drawn.contains("secret set exa"), "{drawn}");
        assert!(drawn.contains("secret set elevenlabs"), "{drawn}");
        assert!(!drawn.contains("secret set openai"), "{drawn}");
        assert!(drawn.contains("Check again"), "{drawn}");
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
    }

    /// The dead end this replaced: the only way out threw the topic away.
    /// Check again asks the runtime once more and keeps what was typed.
    #[test]
    fn checking_again_keeps_the_topic_and_asks_the_runtime_once_more() {
        let mut runner = kobo_sdk::AppRunner::new(Audiobook {
            stage: Stage::Keys,
            missing: vec!["exa"],
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        });
        let commands = runner.action(action_id(CHECK));
        assert_eq!(runner.app().stage, Stage::Setup);
        assert_eq!(runner.app().topic.text(), "the moon");
        assert!(commands.iter().any(|command| matches!(
            command,
            kobo_sdk::Command::Device(kobo_sdk::DeviceRequest::CheckSecrets { .. })
        )));
    }

    /// Every key present leads to the estimate, not to the first payment.
    #[test]
    fn the_cost_and_time_are_shown_before_anything_is_spent() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Setup,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        };
        let all = SECRETS
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        app.checked_secrets(&mut context, &all);
        assert_eq!(app.stage, Stage::Confirm);
        assert!(app.task.is_none(), "nothing is spent before Create");
        let drawn = format!("{:?}", app.screen(&Context::default()));
        assert!(drawn.contains("minutes"), "{drawn}");
        assert!(drawn.contains("ElevenLabs characters"), "{drawn}");
        let estimate = pipeline::Estimate::for_a_book();
        assert!(
            drawn.contains(&format!(
                "${} to ${}",
                estimate.dollars.0, estimate.dollars.1
            )),
            "{drawn}"
        );
        assert!(drawn.contains("An estimate"), "{drawn}");
        app.start_creation(&mut context);
        assert_eq!(app.stage, Stage::Research);
        assert!(app.task.is_some(), "Create starts the research");
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
        assert_eq!(
            app.stage,
            Stage::Confirm,
            "the estimate is still shown first"
        );
        assert!(app.task.is_none());
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

    fn store_requests(commands: &[kobo_sdk::Command]) -> Vec<kobo_sdk::StoreRequest> {
        commands
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::Store(request) => Some(request.clone()),
                _ => None,
            })
            .collect()
    }

    fn speech_requests(commands: &[kobo_sdk::Command]) -> Vec<String> {
        commands
            .iter()
            .filter_map(|command| match command {
                kobo_sdk::Command::Spawn {
                    work: kobo_sdk::Task::Post { url, body, .. },
                    ..
                } if url.contains("text-to-speech") => Some(body.clone()),
                _ => None,
            })
            .collect()
    }

    fn moon_checkpoint(saved: usize) -> Checkpoint {
        Checkpoint {
            topic: "the moon".to_owned(),
            language: pipeline::Language::default(),
            title: "The Moon".to_owned(),
            summary: String::new(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            next_part: saved,
        }
    }

    /// After a restart the parts already narrated are read back off the
    /// shelf, and only the part after them is paid for. This used to start
    /// again from part one and pay for every part a second time.
    #[test]
    fn a_stored_checkpoint_reads_saved_parts_back_instead_of_narrating_them_again() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Compose,
            checkpoint: Some(moon_checkpoint(2)),
            ..Audiobook::default()
        };
        app.resume(&mut context);
        assert_eq!(app.stage, Stage::Restore);
        let commands = context.take_commands();
        assert!(
            speech_requests(&commands).is_empty(),
            "nothing is paid for yet"
        );
        assert!(store_requests(&commands).iter().any(|request| matches!(
            request,
            kobo_sdk::StoreRequest::ShelfRead { name, .. } if name == "the-moon.part-001"
        )));
        for (index, name) in ["the-moon.part-001", "the-moon.part-002"]
            .iter()
            .enumerate()
        {
            app.on_store(
                &mut context,
                StoreResult::ShelfRead {
                    name: (*name).to_owned(),
                    offset: 0,
                    bytes: vec![u8::try_from(index).unwrap(); 300],
                    size: 300,
                },
            );
        }
        assert_eq!(app.stage, Stage::Narrate);
        assert_eq!(app.tracks.len(), 2, "both saved parts came back");
        assert_eq!(app.next_part, 2);
        let speech = speech_requests(&context.take_commands());
        assert_eq!(speech.len(), 1, "{speech:?}");
        assert!(speech[0].contains("three"), "only part three is narrated");
    }

    /// A part the shelf cannot give back is narrated again, and so is
    /// everything after it, but nothing before it.
    #[test]
    fn a_saved_part_that_is_missing_is_narrated_again_from_there() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Compose,
            checkpoint: Some(moon_checkpoint(2)),
            ..Audiobook::default()
        };
        app.resume(&mut context);
        app.on_store(
            &mut context,
            StoreResult::ShelfRead {
                name: "the-moon.part-001".to_owned(),
                offset: 0,
                bytes: vec![1; 300],
                size: 300,
            },
        );
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::Denied(kobo_sdk::StoreError::Missing),
        );
        assert_eq!(app.stage, Stage::Narrate);
        assert_eq!(app.next_part, 1);
        let speech = speech_requests(&context.take_commands());
        assert_eq!(speech.len(), 1);
        assert!(speech[0].contains("two"), "{speech:?}");
    }

    /// Each narrated part is written to the shelf, and the checkpoint moves
    /// only once it is there, so a restart is never promised audio that was
    /// not saved.
    #[test]
    fn a_narrated_part_is_kept_on_the_shelf_before_the_checkpoint_counts_it() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Narrate,
            title: "The Moon".to_owned(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned()],
            ..Audiobook::default()
        };
        app.received_voice(&mut context, vec![7; 300]);
        let requests = store_requests(&context.take_commands());
        assert!(requests.iter().any(|request| matches!(
            request,
            kobo_sdk::StoreRequest::ShelfWrite { name, last: true, .. } if name == "the-moon.part-001"
        )));
        assert!(
            !requests.iter().any(|request| matches!(
                request,
                kobo_sdk::StoreRequest::Save { key, .. } if key == CHECKPOINT_KEY
            )),
            "the checkpoint counted a part before it was saved"
        );
        app.on_store(
            &mut context,
            StoreResult::ShelfWritten {
                name: "the-moon.part-001".to_owned(),
                size: 300,
            },
        );
        let saved = store_requests(&context.take_commands())
            .into_iter()
            .find_map(|request| match request {
                kobo_sdk::StoreRequest::Save { key, value } if key == CHECKPOINT_KEY => {
                    read_checkpoint(&value)
                }
                _ => None,
            })
            .expect("the checkpoint moved");
        assert_eq!(saved.next_part, 1);
        assert_eq!(app.checkpoint.map(|kept| kept.next_part), Some(1));
    }

    /// The book's own upload waits for the last part's write, so the two
    /// never answer on the same shelf at once.
    #[test]
    fn the_book_is_packaged_only_once_the_last_part_is_on_the_shelf() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Narrate,
            title: "The Moon".to_owned(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned()],
            ..Audiobook::default()
        };
        app.received_voice(&mut context, vec![7; 300]);
        assert_eq!(
            app.stage,
            Stage::Narrate,
            "packaged before the part was saved"
        );
        assert!(app.upload.is_none());
        app.on_store(
            &mut context,
            StoreResult::ShelfWritten {
                name: "the-moon.part-001".to_owned(),
                size: 300,
            },
        );
        assert_eq!(app.stage, Stage::Save);
        assert!(app.upload.is_some());
    }

    /// Once the book is saved its parts are inside it, so they go.
    #[test]
    fn a_saved_book_removes_the_parts_it_was_made_from() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Save,
            title: "The Moon".to_owned(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned()],
            checkpoint: Some(moon_checkpoint(2)),
            upload: Some(kobo_sdk::ShelfUpload::new("the-moon.mp3z", vec![1; 10])),
            ..Audiobook::default()
        };
        if let Some(upload) = app.upload.as_mut() {
            upload.start(&mut context);
        }
        let _ = context.take_commands();
        app.on_store(
            &mut context,
            StoreResult::ShelfWritten {
                name: "the-moon.mp3z".to_owned(),
                size: 10,
            },
        );
        let removed = store_requests(&context.take_commands())
            .into_iter()
            .filter_map(|request| match request {
                kobo_sdk::StoreRequest::ShelfRemove { name } => Some(name),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            removed.contains(&"the-moon.part-001".to_owned()),
            "{removed:?}"
        );
        assert!(
            removed.contains(&"the-moon.part-002".to_owned()),
            "{removed:?}"
        );
        assert!(app.checkpoint.is_none());
    }

    /// A cancel after some parts are saved leaves the composer offering a
    /// resume that reads those parts back, not one from the first part.
    #[test]
    fn a_cancel_keeps_the_saved_parts_resumable() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Narrate,
            title: "The Moon".to_owned(),
            archive_name: "the-moon.mp3z".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            ..Audiobook::default()
        };
        app.received_voice(&mut context, vec![7; 300]);
        app.on_store(
            &mut context,
            StoreResult::ShelfWritten {
                name: "the-moon.part-001".to_owned(),
                size: 300,
            },
        );
        app.reset();
        assert_eq!(app.checkpoint.as_ref().map(|kept| kept.next_part), Some(1));
        app.resume(&mut context);
        assert_eq!(app.stage, Stage::Restore);
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
        let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
        assert!(!drawn.contains("Resume"), "{drawn}");
        app.parts = vec!["one".to_owned()];
        let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
        assert!(drawn.contains("Resume"), "{drawn}");
        assert!(drawn.contains("Start over"), "{drawn}");
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
    }

    /// An empty shelf is the moment the sample is for: no accounts, no
    /// network, just proof the player works.
    #[test]
    fn an_empty_library_offers_the_free_sample() {
        let mut app = Audiobook {
            library: Some(Vec::new()),
            ..Audiobook::default()
        };
        let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
        assert!(drawn.contains("Play the sample"), "{drawn}");
        assert!(app
            .screen(&Context::default())
            .validate(&CLARA_BW_METRICS)
            .is_empty());
        // A shelf with books on it does not need the offer; the sample is
        // there once saved, listed like any other book.
        app.titles = vec![(SAMPLE_NAME.to_owned(), SAMPLE_TITLE.to_owned())];
        app.shelved(
            &kobo_sdk::AppRunner::new(Audiobook::default()).context(),
            &[(SAMPLE_NAME.to_owned(), 900_000)],
        );
        let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
        assert!(drawn.contains(SAMPLE_TITLE), "{drawn}");
    }

    /// The sample plays like any book once it is on the shelf, and saves
    /// itself through the same upload when it is not.
    #[test]
    fn the_sample_saves_then_plays_like_any_book() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            library: Some(Vec::new()),
            ..Audiobook::default()
        };
        app.play_sample(&mut context);
        assert_eq!(app.stage, Stage::Save);
        assert!(app.upload.is_some(), "the sample is being saved first");
        assert!(app.saving_sample);
        let mut saved = Audiobook {
            library: Some(vec![Saved {
                name: SAMPLE_NAME.to_owned(),
                title: SAMPLE_TITLE.to_owned(),
                bytes: 900_000,
            }]),
            ..Audiobook::default()
        };
        saved.play_sample(&mut context);
        assert_eq!(saved.stage, Stage::Player);
        assert!(saved.upload.is_none(), "no second save for a saved sample");
    }

    /// A key that goes missing before the script exists gets the keys
    /// screen; one that goes missing during narration keeps the resume and
    /// names the command.
    #[test]
    fn a_key_missing_part_way_names_its_command_and_keeps_what_was_made() {
        let runner = kobo_sdk::AppRunner::new(Audiobook::default());
        let mut context = runner.context();
        let mut app = Audiobook {
            stage: Stage::Research,
            ..Audiobook::default()
        };
        app.task = context.spawn(pipeline::research("the moon"));
        let task = app.task.expect("research in flight");
        app.on_task(
            &mut context,
            task,
            kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );
        assert_eq!(app.stage, Stage::Keys);
        assert_eq!(app.missing, vec!["exa"]);

        let mut app = Audiobook {
            stage: Stage::Narrate,
            title: "The Moon".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned()],
            ..Audiobook::default()
        };
        app.task = context.spawn(pipeline::speech("one", pipeline::Language::default()));
        let task = app.task.expect("narration in flight");
        app.on_task(
            &mut context,
            task,
            kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
        );
        assert_eq!(app.stage, Stage::Failed);
        let (state, advice) = app.trouble.clone().expect("a failure was recorded");
        assert_eq!(state, StandardState::PermissionDenied);
        assert!(advice.contains("kobo secret set elevenlabs"), "{advice}");
        assert!(format!("{:?}", app.screen(&Context::default())).contains("Resume"));
    }

    /// A dropped connection used to leave "Try another topic" as the only
    /// way on, which threw the typed topic away.
    #[test]
    fn a_dropped_connection_offers_to_try_the_same_topic_again() {
        let mut runner = kobo_sdk::AppRunner::new(Audiobook {
            stage: Stage::Research,
            topic: kobo_sdk::keyboard::Keyboard::with_text("the moon"),
            ..Audiobook::default()
        });
        runner.app_mut().fail_task(kobo_sdk::TaskError::Offline);
        let drawn = format!("{:?}", runner.app().screen(&Context::default()));
        assert!(drawn.contains("Try again"), "{drawn}");
        assert!(drawn.contains("New topic"), "{drawn}");
        assert!(!drawn.contains("Try another topic"), "{drawn}");
        runner.action(action_id(CHECK));
        assert_eq!(runner.app().stage, Stage::Setup);
        assert_eq!(runner.app().topic.text(), "the moon");
    }

    /// The resume button stays one short word, and the message says where
    /// it carries on.
    #[test]
    fn a_failure_with_something_to_resume_says_which_part_it_carries_on_with() {
        let app = Audiobook {
            stage: Stage::Failed,
            title: "A researched history of the night sky and the people who mapped it".to_owned(),
            parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
            next_part: 1,
            trouble: Some((
                StandardState::Error,
                "ElevenLabs did not answer.".to_owned(),
            )),
            ..Audiobook::default()
        };
        let drawn = format!("{:?}", app.screen(&Context::default()));
        assert!(drawn.contains("carries on from part 2 of 3"), "{drawn}");
        assert!(!drawn.contains("Resume 'A researched"), "{drawn}");
    }

    /// A refused key names the provider and the command, and a refusal that
    /// is not about the key never sends the owner to check an address these
    /// fixed endpoints do not have.
    #[test]
    fn a_provider_refusal_names_the_provider_and_never_a_server_address() {
        let mut app = Audiobook {
            stage: Stage::Narrate,
            ..Audiobook::default()
        };
        app.fail_task(kobo_sdk::TaskError::Unauthorized);
        let (_, advice) = app.trouble.clone().expect("a failure");
        assert!(advice.starts_with("ElevenLabs"), "{advice}");
        assert!(advice.contains("kobo secret set elevenlabs"), "{advice}");
        let mut app = Audiobook {
            stage: Stage::Write,
            ..Audiobook::default()
        };
        app.fail_task(kobo_sdk::TaskError::NotFound);
        let (_, advice) = app.trouble.clone().expect("a failure");
        assert!(advice.starts_with("OpenAI"), "{advice}");
        assert!(!advice.contains("address"), "{advice}");
    }
}

#[cfg(test)]
mod compose_tests;
