//! Audiobooks on the reader: every book the owner copied over USB, played
//! with its chapters, at its own speed, with a sleep timer and a place that
//! is kept; and the studio that makes a new one from a topic.

mod pipeline;

use kobo_sdk::audio::{span, AudioMetadata, AudioPlayer};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, typographic_cover, AudioEntry, BannerLevel, Context, DeviceError, DeviceRequest,
    DeviceResult, Failure, Glyph, Heartbeat, KoboApp, LogLevel, PictureHandle, RowLead, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StandardState, StoreResult, TaskId,
    TaskOutcome,
};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AGAIN: &str = "again";
const CANCEL: &str = "cancel";
const NEW: &str = "new";
/// Asks the runtime again which keys are installed, after the owner has run
/// the commands the keys screen named.
const CHECK: &str = "check";
/// Back to the composer with the topic still typed.
const EDIT: &str = "edit";
/// The topic field, which raises the reader's keyboard.
const TOPIC: &str = "topic";
/// Checks the keys for the topic in the field and shows the estimate.
const CREATE: &str = "create";
/// Spends: the one control that starts the paid requests.
const START: &str = "start";
/// The firmware's container for a sideloaded audiobook.
const ARCHIVE_SUFFIX: &str = ".mp3z";
const SHELF: &str = "shelf";
const LIBRARY_BACK: &str = "library-back";
const LIBRARY_NEXT: &str = "library-next";
/// The row at the head of the library that returns to the book playing.
const NOW_PLAYING: &str = "now-playing";
/// What the library says when the listing stopped short.
const TRUNCATED: &str = "Only the first two thousand audiobooks are listed.";
/// Opens a book that would not play once more, after it has been copied
/// over again.
const PLAY_AGAIN: &str = "play-again";

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

/// Where every book's place is kept: how far in, how fast, whether it is
/// finished, when it was last heard, and what opening it taught about the
/// book. One record for the whole library, because a listener has a few
/// books on the go and a shelf of two thousand is still a few kilobytes.
const PLACES_KEY: &str = "places";

/// How far the position moves before the place is written again while a
/// book plays. A write is a file on the card; one a minute is a place that
/// is never more than a minute stale and a card that is not written to
/// every few seconds for hours.
const SAVE_EVERY_MS: u32 = 60_000;

/// The largest narrated part read back after a restart. A part is a thousand
/// characters of speech, about a megabyte of MP3, and narration refuses
/// anything over four.
const MAX_PART_BYTES: usize = 4 * 1024 * 1024;

/// The cover drawn in the player, in pixels. A book's own art is fitted
/// inside this; a book without any gets its title set in type at this size.
const COVER_WIDTH: u32 = 240;
const COVER_HEIGHT: u32 = 320;

/// The field and record separators of the places record. Not tabs and
/// newlines, because a file name may hold a tab and a title certainly may.
const FIELD: char = '\u{1f}';
const RECORD: char = '\u{1e}';

/// One audiobook on the reader, as the runtime lists it.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Book {
    id: String,
    /// The file or folder name without its suffix.
    title: String,
    kind: u8,
    parts: u16,
    bytes: u64,
}

/// Where the listener got to in one book, and what opening it taught.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Place {
    id: String,
    position_ms: u32,
    duration_ms: u32,
    speed_percent: u16,
    finished: bool,
    /// Seconds since the epoch when the book last played, for the rewind on
    /// coming back and for listing what is on the go first.
    last_played: u64,
    /// What the book said it is called, which is better than its file name.
    title: String,
    author: String,
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
    /// What is on the reader. The application opens here rather than on
    /// the composer, because a book is worth more than the next one.
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts about the library and the work in flight, not alternate states"
)]
struct Audiobook {
    stage: Stage,
    /// What the book is about, as last typed.
    topic: String,
    /// The topic, typed on the reader's own keyboard.
    entry: TextEntry,
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
    /// The place record the player is for, so what it learns is kept
    /// against the right book.
    playing: Option<String>,
    /// The position last written for the playing book.
    saved_position_ms: u32,
    /// `None` until the runtime has answered, so the library can say it is
    /// looking rather than claiming to be empty before it knows.
    library: Option<Vec<Book>>,
    /// Whether the listing stopped short of the whole collection.
    truncated: bool,
    /// Archive name to title, oldest first, as it was last saved.
    titles: Vec<(String, String)>,
    /// Every book's place, as last written.
    places: Vec<Place>,
    /// Which library entries belong to which page. Nothing here scrolls.
    pages: Vec<Vec<usize>>,
    page: usize,
    /// Ticks while a provider is thinking, so a stage that takes a hundred
    /// seconds does not sit on an unchanging panel looking crashed.
    clock: Heartbeat,
    /// Set when the runtime refuses to list the audiobooks, so the library
    /// can stop looking. Without this a refusal leaves "Looking" on the
    /// panel for as long as the application is open.
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
    /// Why the book just opened cannot be played at all, when it cannot.
    /// The player is put away for it: play, skip and volume can do nothing
    /// for a file that will not decode.
    unplayable: Option<DeviceError>,
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
                if self.entry.is_open() {
                    return screen
                        .text_entry(&self.entry, "Your topic", "Create")
                        .build();
                }
                if self.has_books() {
                    screen = screen.top_bar_glyph(SHELF, "Audiobooks", Glyph::Headphones);
                }
                if self.checkpoint.is_some() {
                    screen = screen.top_bar_action(RESUME, "Resume");
                }
                // The field carries its own name, and the line under it says
                // what is wrong with the topic, or where an interrupted book
                // would carry on, in the one place either is looked for.
                let mut screen = screen
                    .field(TOPIC, &self.topic, "Type any topic")
                    .field_label("Your topic");
                if let Some(hint) = self.hint {
                    screen = screen.field_error(hint);
                } else if let Some(checkpoint) = &self.checkpoint {
                    let title = if checkpoint.title.is_empty() {
                        "Interrupted audiobook"
                    } else {
                        &checkpoint.title
                    };
                    screen = screen.field_hint(context.one_line_row(
                        &format!(
                            "Resume part {} of {}: {title}",
                            checkpoint.next_part + 1,
                            checkpoint.parts.len().max(1)
                        ),
                        false,
                    ));
                } else {
                    screen = screen.field_hint("Saved here to play offline.");
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
                    .primary_button(CREATE, "Create")
                    .build()
            }
            Stage::Player if self.unplayable.is_some() => self.unplayable_screen(),
            Stage::Player => self.player.as_ref().map_or_else(
                || {
                    ScreenBuilder::new("audiobook-player-missing")
                        .top_bar("Audiobook")
                        .error_state("The player could not be prepared.")
                        .button(SHELF, "Your audiobooks")
                        .build()
                },
                |player| player.screen_for(context),
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

    /// A book that opened and will not play: what is wrong with the file,
    /// what to do about it, and the way back, in place of a player whose
    /// every control would do nothing.
    fn unplayable_screen(&self) -> Screen {
        let advice = match self.unplayable {
            Some(DeviceError::InvalidInput) => {
                "This reader cannot play this kind of file. Audiobooks plays MP3, M4B, M4A, OGG, \
                 Opus, FLAC and WAV."
            }
            Some(DeviceError::NotFound) => {
                "The file is no longer on this reader. It may have been moved or deleted over USB."
            }
            _ => {
                "The file could not be decoded. It may be damaged or only partly copied. Copy it \
                 to the reader again over USB, then try again."
            }
        };
        ScreenBuilder::new("audiobook-unplayable")
            .top_bar(self.playing_title())
            .owns_back(true)
            .splash(
                Some(Glyph::Headphones),
                "This book cannot be played",
                advice,
            )
            .buttons([(PLAY_AGAIN, "Try again"), (SHELF, "Your audiobooks")])
            .build()
    }

    /// Opens the book that would not play once more, as if from its row.
    /// Returns whether the tap was Try again.
    fn play_again(&mut self, context: &mut Context, action: kobo_sdk::ActionId) -> bool {
        if self.stage != Stage::Player || action != action_id(PLAY_AGAIN) {
            return false;
        }
        let again = self.playing.clone().and_then(|id| {
            self.library
                .as_ref()?
                .iter()
                .position(|book| same_book(&book.id, &id))
        });
        self.unplayable = None;
        if let Some(index) = again {
            self.play(context, index);
        }
        self.show(context);
        true
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

    /// What is on the reader, and the way back to what is playing.
    ///
    /// Everything here reads from the card. Nothing on this screen, and
    /// nothing reached from it, needs the network: a book copied over in
    /// March plays in September on a reader that has been in aeroplane mode
    /// since.
    fn library_screen(&self) -> Screen {
        let screen = ScreenBuilder::new("audiobook-library").top_bar("Audiobooks");
        let Some(books) = self.library.as_ref() else {
            return screen.activity("Looking for audiobooks", None).build();
        };
        if books.is_empty() {
            let screen = if self.shelf_unreadable {
                screen.error_state("The audiobooks on this reader could not be listed.")
            } else {
                screen.empty_state(
                    "No audiobooks yet. Over USB, copy MP3, M4B, OGG, FLAC or WAV files into \
                     a folder called Audiobooks, one folder per book. Or make one here.",
                )
            };
            return screen
                .primary_button(NEW, "Create an audiobook")
                .button(SAMPLE, "Play the sample")
                .build();
        }
        // The bottom of the panel is spent on page turns, so the way to the
        // composer is the one action the top bar allows.
        let placed = self.library_head(screen.top_bar_glyph(NEW, "Create", Glyph::Plus));
        let showing = self.pages.get(self.page).map_or(&[][..], Vec::as_slice);
        let playing = self.playing_shown();
        let rows = showing
            .iter()
            .filter_map(|index| books.get(*index).map(|book| (*index, book)))
            .map(|(index, book)| {
                if playing == Some(index) {
                    if let Some(status) = self.player_status() {
                        let status = match self
                            .place_of(&book.id)
                            .map(|place| place.author.as_str())
                            .filter(|author| !author.is_empty())
                        {
                            Some(author) => format!("{author} · {status}"),
                            None => status,
                        };
                        return (
                            NOW_PLAYING.to_owned(),
                            self.title_of(book),
                            status,
                            RowLead::from(Glyph::Headphones),
                        );
                    }
                }
                let place = self.place_of(&book.id);
                (
                    play_action(index),
                    self.title_of(book),
                    Self::status_of(book, place),
                    RowLead::from(match place {
                        Some(place) if place.finished => Glyph::Check,
                        Some(place) if place.position_ms > 0 => Glyph::Headphones,
                        _ => Glyph::Circle,
                    }),
                )
            })
            .collect::<Vec<_>>();
        let mut turning = placed.rows(rows).page_turns(LIBRARY_BACK, LIBRARY_NEXT);
        if self.pages.len() > 1 {
            turning = turning.page_position(
                u16::try_from(self.page + 1).unwrap_or(u16::MAX),
                u16::try_from(self.pages.len()).unwrap_or(u16::MAX),
            );
        }
        turning.build()
    }

    /// Everything the library shows above its rows: a note when the listing
    /// stopped short, and the book playing, which is one tap from wherever
    /// the listener wandered to.
    fn library_head(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let mut screen = screen;
        if self.truncated {
            screen = screen.banner(BannerLevel::Info, TRUNCATED);
        }
        // The book playing has a row of its own on the page that lists it.
        // Above that page it was listed twice, once playing and once as a
        // file not yet opened.
        if self.playing_shown().is_none() {
            screen = self.now_playing(screen);
        }
        screen
    }

    /// The book playing, as a row of its own under its own heading.
    fn now_playing(&self, screen: ScreenBuilder) -> ScreenBuilder {
        match self.player_status() {
            Some(status) => screen.section("Now playing").rows([(
                NOW_PLAYING,
                self.playing_title(),
                status,
                RowLead::from(Glyph::Headphones),
            )]),
            None => screen,
        }
    }

    /// Where the player is in the book playing, as its row says it.
    fn player_status(&self) -> Option<String> {
        let player = self.player.as_ref()?;
        let left = player
            .duration()
            .saturating_sub(player.position())
            .as_millis();
        let left = span(u32::try_from(left).unwrap_or(u32::MAX));
        Some(if player.is_playing() {
            format!("Playing · {left} left")
        } else if player.is_finished() {
            "Finished".to_owned()
        } else {
            format!("Paused · {left} left")
        })
    }

    /// The library index of the book playing, when the page showing lists
    /// it.
    fn playing_shown(&self) -> Option<usize> {
        self.player.as_ref()?;
        let playing = self.playing.as_deref()?;
        let books = self.library.as_ref()?;
        self.pages.get(self.page)?.iter().copied().find(|index| {
            books
                .get(*index)
                .is_some_and(|book| same_book(&book.id, playing))
        })
    }

    /// What the book playing is called, by the same rules as its row: the
    /// studio's name for it, else what it said, else its file name.
    fn playing_title(&self) -> String {
        let Some(id) = self.playing.as_deref() else {
            return String::new();
        };
        if let Some(book) = self
            .library
            .as_ref()
            .and_then(|books| books.iter().find(|book| same_book(&book.id, id)))
        {
            return self.title_of(book);
        }
        let stand_in = Book {
            id: id.to_owned(),
            title: id
                .strip_prefix("shelf:")
                .and_then(|name| name.strip_suffix(ARCHIVE_SUFFIX))
                .unwrap_or(id)
                .replace('-', " "),
            ..Book::default()
        };
        self.title_of(&stand_in)
    }

    /// What a library row says under a book's title: who it is by and how
    /// far in it is, or what kind of file it is when nothing has opened it.
    fn status_of(book: &Book, place: Option<&Place>) -> String {
        let author = place
            .map(|place| place.author.as_str())
            .filter(|author| !author.is_empty());
        let state = match place {
            Some(place) if place.finished => "Finished".to_owned(),
            Some(place) if place.position_ms > 0 && place.duration_ms > 0 => {
                format!(
                    "{} left",
                    span(place.duration_ms.saturating_sub(place.position_ms))
                )
            }
            _ => {
                let mut state = kind_badge(book.kind).to_owned();
                if book.parts > 1 {
                    state.push_str(&format!(" · {} parts", book.parts));
                }
                state.push_str(&format!(" · {}", size_on_disk(book.bytes)));
                state
            }
        };
        match author {
            Some(author) => format!("{author} · {state}"),
            None => state,
        }
    }

    /// What a book is called: the title the studio saved it under, else
    /// what it said when it was opened, else its file name.
    fn title_of(&self, book: &Book) -> String {
        if let Some(title) = self.studio_title(book) {
            return title.to_owned();
        }
        self.place_of(&book.id)
            .map(|place| place.title.as_str())
            .filter(|title| !title.is_empty())
            .map_or_else(|| book.title.clone(), str::to_owned)
    }

    /// The title a book made here was saved under, which its files do not
    /// carry: the narration arrives without tags.
    fn studio_title(&self, book: &Book) -> Option<&str> {
        let file = book
            .id
            .strip_prefix("shelf:")
            .unwrap_or_else(|| book.id.rsplit('/').next().unwrap_or_default());
        self.titles
            .iter()
            .find(|(name, _)| name == file)
            .map(|(_, title)| title.as_str())
    }

    fn place_of(&self, id: &str) -> Option<&Place> {
        self.places.iter().find(|place| same_book(&place.id, id))
    }

    fn has_books(&self) -> bool {
        self.library.as_ref().is_some_and(|books| !books.is_empty())
    }

    /// Opens the library, and asks the runtime what is on the reader.
    ///
    /// Asked every time rather than once at start, because this is also the
    /// screen somebody arrives at after copying a book over USB or making
    /// one here, and the card is the only thing that knows what landed.
    fn open_library(&mut self, context: &mut Context) {
        self.stage = Stage::Library;
        context.device().list_audio();
        self.show(context);
    }

    /// Builds the library from what the runtime listed.
    fn listed(&mut self, context: &Context, entries: &[AudioEntry], truncated: bool) {
        self.library = Some(
            entries
                .iter()
                .map(|entry| Book {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    kind: entry.kind,
                    parts: entry.parts,
                    bytes: entry.bytes,
                })
                .collect(),
        );
        self.truncated = truncated;
        self.shelf_unreadable = false;
        self.repage(context);
    }

    /// Lays the library out in pages: the books on the go first, newest
    /// listening first, then the ones not started, then the finished ones,
    /// each group by title.
    fn repage(&mut self, context: &Context) {
        let Some(books) = self.library.as_ref() else {
            return;
        };
        let mut order = (0..books.len()).collect::<Vec<_>>();
        let rank = |index: usize| -> (u8, u64, String) {
            let book = &books[index];
            let place = self.place_of(&book.id);
            let title = self.title_of(book).to_lowercase();
            match place {
                Some(place) if place.finished => (2, 0, title),
                Some(place) if place.position_ms > 0 => (0, u64::MAX - place.last_played, title),
                _ => (1, 0, title),
            }
        };
        order.sort_by_key(|index| rank(*index));
        let rows = order
            .iter()
            .map(|index| {
                let book = &books[*index];
                (
                    self.title_of(book),
                    Self::status_of(book, self.place_of(&book.id)),
                )
            })
            .collect::<Vec<_>>();
        let measured = rows
            .iter()
            .map(|(title, status)| (title.as_str(), status.as_str()))
            .collect::<Vec<_>>();
        // Measured with the Now playing row whenever a book plays. Whether
        // a page shows it depends on whether that page lists the book, which
        // is not known until the pages exist, and a page measured without it
        // pushed its last row under the page turns and off the panel.
        let mut placed = ScreenBuilder::new("audiobook-library")
            .top_bar("Audiobooks")
            .top_bar_glyph(NEW, "Create", Glyph::Plus);
        if self.truncated {
            placed = placed.banner(BannerLevel::Info, TRUNCATED);
        }
        let placed = self.now_playing(placed).build();
        let pages =
            context.paginate_rows_under(&measured, false, kobo_sdk::Position::AtTheFoot, &placed);
        self.pages = pages
            .into_iter()
            .map(|page| page.into_iter().map(|row| order[row]).collect())
            .collect();
        self.page = self.page.min(self.pages.len().saturating_sub(1));
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

    /// Opens a listed book where the listener left it, and plays.
    fn play(&mut self, context: &mut Context, index: usize) {
        let Some(book) = self
            .library
            .as_ref()
            .and_then(|books| books.get(index))
            .cloned()
        else {
            return;
        };
        let title = self.title_of(&book);
        let place = self.place_of(&book.id).cloned().unwrap_or_default();
        let mut metadata = AudioMetadata::new(&title);
        if !place.author.is_empty() {
            metadata = metadata.author(place.author.clone());
        }
        let mut player = match book.id.strip_prefix("shelf:") {
            Some(name) => AudioPlayer::shelf(name, &title),
            None => AudioPlayer::library(&book.id, &title),
        }
        .metadata(metadata)
        .owns_back(true);
        if self.studio_title(&book).is_some() {
            player = player.keep_title();
        }
        if place.speed_percent > 0 {
            player = player.speed(place.speed_percent);
        }
        if !place.finished && place.position_ms > 0 {
            let since = now_secs().saturating_sub(place.last_played);
            player = player.resuming_at(Duration::from_millis(u64::from(resume_from(
                place.position_ms,
                Duration::from_secs(since),
            ))));
        }
        let author = (!place.author.is_empty()).then_some(place.author.as_str());
        player.set_cover(context.put_picture(
            PictureHandle(1),
            COVER_WIDTH,
            COVER_HEIGHT,
            typographic_cover(&title, author, COVER_WIDTH, COVER_HEIGHT),
        ));
        self.open_player(context, player, book.id);
    }

    /// The free sample, playable before any account exists.
    ///
    /// Once saved it is an ordinary book on the reader and lists like one.
    /// Before that, saving it is the whole errand: the same upload and the
    /// same player a created book uses, because a sample that took a
    /// shortcut would prove the shortcut, not the book.
    fn play_sample(&mut self, context: &mut Context) {
        SAMPLE_TITLE.clone_into(&mut self.title);
        SAMPLE_NAME.clone_into(&mut self.archive_name);
        let already_saved = self.library.as_ref().is_some_and(|books| {
            books.iter().any(|book| {
                book.id.ends_with(&format!("/{SAMPLE_NAME}"))
                    || book.id == format!("shelf:{SAMPLE_NAME}")
            })
        });
        if already_saved {
            self.open_shelf_player(context);
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

    /// The player for an audiobook made here, straight from the shelf it
    /// was just saved to: the listing that will name it has not arrived yet,
    /// and nobody should wait for a list to hear what they just made.
    fn open_shelf_player(&mut self, context: &mut Context) {
        let title = self.title.clone();
        let mut player = AudioPlayer::shelf(&self.archive_name, &title)
            .metadata(
                AudioMetadata::new(&title)
                    .author("Researched, written and narrated on this reader"),
            )
            .owns_back(true)
            .keep_title();
        player.set_cover(context.put_picture(
            PictureHandle(1),
            COVER_WIDTH,
            COVER_HEIGHT,
            // The title alone. The line beside the cover already says where
            // the book was made, and a byline set at cover size had room for
            // "Made on this..." and no more.
            typographic_cover(&title, None, COVER_WIDTH, COVER_HEIGHT),
        ));
        let id = format!("shelf:{}", self.archive_name);
        self.open_player(context, player, id);
    }

    /// Shows a player and starts it. One function for a book made a minute
    /// ago and one copied over months ago, because the two must not drift:
    /// the second is the one somebody uses fifty times.
    fn open_player(&mut self, context: &mut Context, mut player: AudioPlayer, id: String) {
        if let Some(old) = self.player.as_mut() {
            old.pause(context);
        }
        self.note_place(context, true);
        player.start(context);
        player.play(context);
        self.saved_position_ms = 0;
        self.player = Some(player);
        self.playing = Some(id);
        self.stage = Stage::Player;
    }

    /// Draws the cover the book carried, if it carried one worth drawing.
    fn draw_cover(&mut self, context: &mut Context) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let Some(bytes) = player.take_cover_bytes() else {
            return;
        };
        let Some((width, height, grey)) = cover_picture(&bytes) else {
            return;
        };
        let cover = context.put_picture(PictureHandle(2), width, height, grey);
        player.set_cover(cover);
    }

    /// Writes where the playing book is, when it has moved enough to be
    /// worth writing or when `force` says this is a moment that matters: a
    /// pause, a seek, leaving the player, the reader going to sleep.
    fn note_place(&mut self, context: &mut Context, force: bool) {
        let Some(player) = self.player.as_ref() else {
            return;
        };
        let Some(id) = self.playing.clone() else {
            return;
        };
        let duration_ms = u32::try_from(player.duration().as_millis()).unwrap_or(u32::MAX);
        if duration_ms == 0 {
            return;
        }
        let position_ms = u32::try_from(player.position().as_millis()).unwrap_or(u32::MAX);
        let finished = player.is_finished();
        let moved = position_ms.abs_diff(self.saved_position_ms) >= SAVE_EVERY_MS;
        let known = self.place_of(&id).cloned().unwrap_or_default();
        let detail_title = player
            .detail()
            .map(|detail| detail.title.clone())
            .unwrap_or_default();
        let detail_author = player
            .detail()
            .map(|detail| detail.author.clone())
            .unwrap_or_default();
        let learned = known.title != detail_title || known.author != detail_author;
        let speed_changed = known.speed_percent != player.speed_percent();
        if !(force || moved || finished || learned || speed_changed) {
            return;
        }
        let place = Place {
            id: id.clone(),
            position_ms: if finished { 0 } else { position_ms },
            duration_ms,
            speed_percent: player.speed_percent(),
            finished,
            last_played: now_secs(),
            title: detail_title,
            author: detail_author,
        };
        self.places.retain(|kept| !same_book(&kept.id, &id));
        self.places.push(place);
        self.saved_position_ms = position_ms;
        context.store().save(PLACES_KEY, write_places(&self.places));
    }

    /// Back from the player to the library. The book keeps playing: the
    /// library's first row is the way back to it. The studio starts afresh,
    /// as it did when leaving the player meant stopping the book: a finished
    /// creation is not a draft to come back to.
    fn leave_player(&mut self, context: &mut Context) {
        // A book that would not play is not playing, and is not listed as
        // the one Now playing either.
        if self.unplayable.take().is_some() {
            self.player = None;
            self.playing = None;
        }
        self.note_place(context, true);
        self.clock.stop(context);
        self.reset();
        self.open_library(context);
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
            self.topic.clone_from(&checkpoint.topic);
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
            topic: self.topic.clone(),
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
        let topic = self.topic.trim();
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
        self.task = context.spawn(pipeline::research(self.topic.trim()));
        if self.task.is_none() {
            self.fail("The runtime is already busy.");
        }
        self.show(context);
    }

    fn start_writing(&mut self, context: &mut Context, research: &[u8]) {
        match pipeline::write_book(&self.topic, self.language, research) {
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

    /// A tap on the library: the book playing, a book, the sample, a new
    /// book or a page turn.
    fn on_library_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
        if action == action_id(NOW_PLAYING) && self.player.is_some() {
            self.stage = Stage::Player;
            self.show(context);
            return;
        }
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

    /// Back to a blank composer, keeping the library and the book playing.
    ///
    /// `Default` clears everything, and everything used to be the right
    /// amount, because the application forgot each audiobook the moment it
    /// finished. It no longer does, so what is on the reader, where the
    /// listener is in it, and the book playing all have to outlive a cancel.
    fn reset(&mut self) {
        let library = self.library.take();
        let titles = std::mem::take(&mut self.titles);
        let places = std::mem::take(&mut self.places);
        let pages = std::mem::take(&mut self.pages);
        let shelf_unreadable = self.shelf_unreadable;
        let truncated = self.truncated;
        // A cancel stops the work, not the record of it: the checkpoint the
        // store holds stays offered, here as much as after a restart.
        let checkpoint = self.checkpoint.take();
        // A person who narrates in Hindi will narrate in Hindi again.
        let language = self.language;
        let player = self.player.take();
        let playing = self.playing.take();
        let saved_position_ms = self.saved_position_ms;
        *self = Self {
            language,
            player,
            playing,
            saved_position_ms,
            library,
            truncated,
            titles,
            places,
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
        context.store().load(LIBRARY_KEY);
        context.store().load(PLACES_KEY);
        context.store().load(CHECKPOINT_KEY);
        context.device().list_audio();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
        if self.stage == Stage::Compose && self.entry.is_open() {
            // Back from the keyboard puts it away and keeps the composer.
            if action == kobo_sdk::ActionId::BACK {
                self.entry.close();
                self.show(context);
                return;
            }
            if let Some(typing) = self.entry.handle(action) {
                if let Typing::Submitted(topic) = typing {
                    self.topic = topic;
                    self.begin(context);
                } else {
                    self.show(context);
                }
                return;
            }
        }
        if self.stage == Stage::Compose
            && (action == kobo_sdk::ActionId::BACK || action == action_id(SHELF))
        {
            // Navigation does not discard an unsubmitted topic or language.
            self.open_library(context);
            return;
        }
        if self.play_again(context, action) {
            return;
        }
        if self.stage == Stage::Player
            && self.unplayable.is_none()
            && self
                .player
                .as_mut()
                .is_some_and(|player| player.press(context, action))
        {
            self.note_place(context, false);
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
            self.clock.stop(context);
            // Starting over from a failure retires what was interrupted:
            // the person has seen it fail and chosen a different book.
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
        // The player is always reached from the library, so the runtime's
        // back control belongs to the library here rather than to leaving
        // the application. The book keeps playing; the library's first row
        // leads back to it.
        if action == action_id(SHELF)
            || (self.stage == Stage::Player && action == kobo_sdk::ActionId::BACK)
        {
            if self.stage == Stage::Player {
                self.leave_player(context);
            } else {
                self.clock.stop(context);
                self.reset();
                self.open_library(context);
            }
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
            if action == action_id(TOPIC) {
                self.entry.open_with(self.topic.clone());
                self.show(context);
            } else if action == action_id(CREATE) {
                self.begin(context);
            }
        }
    }

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.stage == Stage::Player {
            if let Some(player) = self.player.as_mut() {
                player.skip(context, forward);
            }
            self.show(context);
        }
    }

    fn on_background(&mut self, context: &mut Context) {
        self.note_place(context, true);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.note_place(context, true);
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
            if matches!(self.stage, Stage::Player | Stage::Library) {
                self.show(context);
            }
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
                    context.device().list_audio();
                    self.open_shelf_player(context);
                    self.show(context);
                    return;
                }
                ShelfProgress::Failed(error) => {
                    self.clock.stop(context);
                    self.fail_with(Failure::storing(error));
                    self.show(context);
                    return;
                }
                // Not the upload's answer. It is one of the ones the library
                // asks for, so fall through rather than dropping it.
                ShelfProgress::Elsewhere => {}
            }
        }
        match result {
            StoreResult::Loaded { key, value } if key == LIBRARY_KEY => {
                self.titles = parse_index(value.as_deref().unwrap_or_default());
                self.repage(context);
                if self.stage == Stage::Library {
                    self.show(context);
                }
            }
            StoreResult::Loaded { key, value } if key == PLACES_KEY => {
                self.places = parse_places(value.as_deref().unwrap_or_default());
                self.repage(context);
                if self.stage == Stage::Library {
                    self.show(context);
                }
            }
            StoreResult::Loaded { key, value } if key == CHECKPOINT_KEY => {
                self.checkpoint = value.as_deref().and_then(read_checkpoint);
                if self.stage == Stage::Compose {
                    self.show(context);
                }
            }
            StoreResult::Shelf(blobs) => {
                let entries = blobs
                    .iter()
                    .filter(|(name, _)| name.ends_with(ARCHIVE_SUFFIX))
                    .map(|(name, bytes)| AudioEntry {
                        id: format!("shelf:{name}"),
                        title: name
                            .strip_suffix(ARCHIVE_SUFFIX)
                            .unwrap_or(name)
                            .replace('-', " "),
                        kind: 7,
                        parts: 1,
                        bytes: u64::from(*bytes),
                    })
                    .collect::<Vec<_>>();
                self.listed(context, &entries, false);
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
        if request == DeviceRequest::ListAudio {
            match &result {
                DeviceResult::AudioLibrary { entries, truncated } => {
                    self.listed(context, entries, *truncated);
                }
                // A runtime that cannot list the reader's audiobooks, as a
                // simulator without an audio backend cannot, can still list
                // this application's own shelf, where the books made here
                // and the sample live.
                DeviceResult::Denied(_) => context.shelf().list(),
                DeviceResult::Failed(_) => {
                    if self.library.is_none() {
                        self.shelf_unreadable = true;
                        self.library = Some(Vec::new());
                    }
                }
                _ => {}
            }
            if self.stage == Stage::Library {
                self.show(context);
            }
            return;
        }
        // A file that will not decode, cannot be played here or has gone
        // is not helped by any control on the player, so the player gives
        // way to a screen that says what to do. An output that is missing
        // is another matter: the player asks for headphones itself.
        if let (DeviceRequest::LoadAudio { .. }, DeviceResult::Failed(error)) = (&request, &result)
        {
            if matches!(
                error,
                DeviceError::Backend | DeviceError::InvalidInput | DeviceError::NotFound
            ) && self.stage == Stage::Player
            {
                self.unplayable = Some(*error);
            }
        }
        if self
            .player
            .as_mut()
            .is_some_and(|player| player.on_device_result(context, &request, &result))
        {
            self.draw_cover(context);
            // A pause, a seek and the end of the book are moments the place
            // must survive; a reading that only moved the clock is written
            // once a minute.
            let moment = matches!(
                request,
                DeviceRequest::PauseAudio | DeviceRequest::SeekAudio { .. }
            ) || self.player.as_ref().is_some_and(|player| {
                player.is_finished() || player.playback() == kobo_sdk::AudioPlaybackState::Paused
            });
            self.note_place(context, moment);
            if matches!(self.stage, Stage::Player | Stage::Library) {
                self.show(context);
            }
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

/// The name of the row that plays the `index`th audiobook.
fn play_action(index: usize) -> String {
    format!("play-{index}")
}

/// The name of the chip that selects a narration language.
fn language_action(language: pipeline::Language) -> String {
    format!("language-{}", language.name())
}

/// The short name of a listed kind, as the wire numbers them.
const fn kind_badge(kind: u8) -> &'static str {
    match kind {
        2 => "M4B",
        3 => "OGG",
        4 => "OPUS",
        5 => "FLAC",
        6 => "WAV",
        // A book made here is kept as its MP3 parts in one archive. The
        // archive is how it is stored, not what anybody listens to, so it
        // is named for the parts inside.
        _ => "MP3",
    }
}

/// The size of a book on the card, in the coarsest honest unit.
fn size_on_disk(bytes: u64) -> String {
    const MEGABYTE: u64 = 1024 * 1024;
    const GIGABYTE: u64 = 1024 * MEGABYTE;
    if bytes >= GIGABYTE {
        let tenths = bytes * 10 / GIGABYTE;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else if bytes >= MEGABYTE {
        format!("{} MB", bytes / MEGABYTE)
    } else {
        format!("{} KB", (bytes / 1024).max(1))
    }
}

/// Whether two identifiers name one book. A book made here is first played
/// off the shelf as `shelf:name` and later listed by the runtime under a
/// path ending in that name; the place is one place either way.
fn same_book(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let shelf_name = |id: &str| id.strip_prefix("shelf:").map(str::to_owned);
    match (shelf_name(left), shelf_name(right)) {
        (Some(name), None) => right.ends_with(&format!("/{name}")),
        (None, Some(name)) => left.ends_with(&format!("/{name}")),
        _ => false,
    }
}

/// Where to carry on from, for a book put down `since` ago.
///
/// Nothing after a short break, a sentence after an hour, a paragraph
/// after a day: the longer the gap, the more of the thread to pick up.
fn resume_from(position_ms: u32, since: Duration) -> u32 {
    let rewind = if since.as_secs() < 60 {
        0
    } else if since.as_secs() < 3_600 {
        10_000
    } else {
        30_000
    };
    position_ms.saturating_sub(rewind)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A book's own cover, fitted to the player and dithered for the panel.
///
/// A picture too small to be a cover is left out: blown up to the hero it
/// is a grey smear, which is worse than the lettered cover already there.
fn cover_picture(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    const MIN_COVER_PX: u32 = 96;
    let picture = kobo_image::decode(bytes).ok()?;
    if picture.width() < MIN_COVER_PX || picture.height() < MIN_COVER_PX {
        return None;
    }
    let mut fitted = picture.fit(COVER_WIDTH, COVER_HEIGHT).ok()?;
    fitted.dither(kobo_image::PANEL_GREYS);
    Some((fitted.width(), fitted.height(), fitted.into_grey()))
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

/// Writes every book's place as one record.
fn write_places(places: &[Place]) -> Vec<u8> {
    let field = |text: &str| {
        text.chars()
            .filter(|character| *character != FIELD && *character != RECORD)
            .collect::<String>()
    };
    places
        .iter()
        .filter(|place| !place.id.contains(FIELD) && !place.id.contains(RECORD))
        .map(|place| {
            [
                "v1".to_owned(),
                place.id.clone(),
                place.position_ms.to_string(),
                place.duration_ms.to_string(),
                place.speed_percent.to_string(),
                u8::from(place.finished).to_string(),
                place.last_played.to_string(),
                field(&place.title),
                field(&place.author),
            ]
            .join(&FIELD.to_string())
        })
        .collect::<Vec<_>>()
        .join(&RECORD.to_string())
        .into_bytes()
}

/// Reads the places record. A line that does not parse is one book whose
/// place is lost, not a library whose places are.
fn parse_places(saved: &[u8]) -> Vec<Place> {
    String::from_utf8_lossy(saved)
        .split(RECORD)
        .filter_map(|line| {
            let fields = line.split(FIELD).collect::<Vec<_>>();
            if fields.len() != 9 || fields[0] != "v1" || fields[1].is_empty() {
                return None;
            }
            Some(Place {
                id: fields[1].to_owned(),
                position_ms: fields[2].parse().ok()?,
                duration_ms: fields[3].parse().ok()?,
                speed_percent: fields[4].parse().ok()?,
                finished: fields[5] == "1",
                last_played: fields[6].parse().ok()?,
                title: fields[7].to_owned(),
                author: fields[8].to_owned(),
            })
        })
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
mod tests;

#[cfg(test)]
mod compose_tests;
