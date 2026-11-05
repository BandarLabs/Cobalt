//! Reusable audiobook player UI and Bluetooth-output handoff.
//!
//! The player an application embeds rather than builds: cover, byline and
//! chapter, transport, volume, the chapters of the book, its speed, a sleep
//! timer, and the headphones picker that Play opens when nothing is
//! connected. The application decides what to play and remembers where it
//! got to; everything about playing it is here, so a podcast application and
//! an audiobook application behave the same way on the same reader.

use crate::{
    action_id, ActionId, AudioChapter, AudioDetail, AudioPlaybackState, AudioSource, BandAlign,
    BannerLevel, BluetoothDevice, BluetoothDeviceKind, Context, DenyReason, DeviceError,
    DeviceRequest, DeviceResult, Glyph, RowLead, Screen, ScreenBuilder, SlotWidth, Task, TaskId,
    TaskOutcome, TilePicture,
};
use std::time::{Duration, Instant};

const PLAY: &str = "audio-play";
const BACK_THIRTY: &str = "audio-back-30";
const FORWARD_THIRTY: &str = "audio-forward-30";
const CHAPTER_BACK: &str = "audio-chapter-back";
const CHAPTER_FORWARD: &str = "audio-chapter-forward";
const VOLUME_DOWN: &str = "audio-volume-down";
const VOLUME_UP: &str = "audio-volume-up";
const OUTPUT: &str = "audio-output";
const CHAPTERS: &str = "audio-chapters";
const SPEED: &str = "audio-speed";
const SLEEP: &str = "audio-sleep";
const CHAPTERS_PREVIOUS: &str = "audio-chapters-previous";
const CHAPTERS_NEXT: &str = "audio-chapters-next";
const CHAPTER_ACTIONS: [&str; 12] = [
    "audio-chapter-0",
    "audio-chapter-1",
    "audio-chapter-2",
    "audio-chapter-3",
    "audio-chapter-4",
    "audio-chapter-5",
    "audio-chapter-6",
    "audio-chapter-7",
    "audio-chapter-8",
    "audio-chapter-9",
    "audio-chapter-10",
    "audio-chapter-11",
];
const SPEED_LESS: &str = "audio-speed-less";
const SPEED_MORE: &str = "audio-speed-more";
/// The speeds offered as one tap, which between them cover what nearly
/// everyone listens at.
const SPEED_PRESETS: [(&str, u16); 5] = [
    ("audio-speed-75", 75),
    ("audio-speed-100", 100),
    ("audio-speed-125", 125),
    ("audio-speed-150", 150),
    ("audio-speed-200", 200),
];
/// One step of the speed stepper. Fine enough that every common speed is
/// on the ladder, coarse enough that the ladder is short.
const SPEED_STEP: u16 = 5;
pub const MIN_SPEED_PERCENT: u16 = 50;
pub const MAX_SPEED_PERCENT: u16 = 300;
const SLEEP_PREVIOUS: &str = "audio-sleep-previous";
const SLEEP_NEXT: &str = "audio-sleep-next";
const SLEEP_OPTIONS: [(&str, Sleep); 6] = [
    ("audio-sleep-off", Sleep::Off),
    ("audio-sleep-15", Sleep::Minutes(15)),
    ("audio-sleep-30", Sleep::Minutes(30)),
    ("audio-sleep-45", Sleep::Minutes(45)),
    ("audio-sleep-60", Sleep::Minutes(60)),
    ("audio-sleep-chapter", Sleep::EndOfChapter),
];
const BLUETOOTH_TOGGLE: &str = "audio-bluetooth-toggle";
const BLUETOOTH_SCAN: &str = "audio-bluetooth-scan";
const DEVICES_PREVIOUS: &str = "audio-devices-previous";
const DEVICES_NEXT: &str = "audio-devices-next";
const DEVICE_ACTIONS: [&str; 6] = [
    "audio-device-0",
    "audio-device-1",
    "audio-device-2",
    "audio-device-3",
    "audio-device-4",
    "audio-device-5",
];
/// How often a playing book is asked where it is. Every answer that moved
/// repaints the position line, and on a panel that is paper the fewer of
/// those the better; ten seconds keeps the clock honest to the nearest
/// glance.
const POLL_SECONDS: u32 = 10;
/// How often a loading book is asked whether it is ready, which is sooner:
/// the reader is waiting for it.
const LOADING_POLL_SECONDS: u32 = 2;
const SCAN_SECONDS: u32 = 3;
const SKIP_MS: u32 = 30_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioMetadata {
    pub title: String,
    pub author: Option<String>,
    pub chapter: Option<String>,
}

impl AudioMetadata {
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            author: None,
            chapter: None,
        }
    }

    #[must_use]
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }

    #[must_use]
    pub fn chapter(mut self, chapter: impl Into<String>) -> Self {
        self.chapter = Some(chapter.into());
        self
    }
}

/// When the sleep timer pauses the book.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Sleep {
    #[default]
    Off,
    Minutes(u16),
    EndOfChapter,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Player,
    Bluetooth,
    Chapters,
    Speed,
    Sleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Availability {
    Available,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Switch {
    Off,
    On,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadState {
    Unloaded,
    Loaded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlayIntent {
    Manual,
    Autoplay,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingBluetooth {
    Connect(String),
}

/// How much of the player gives way so it fits the panel, from nothing to
/// the most. See [`AudioPlayer::screen_for`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "four things that each give way on their own, not four states"
)]
struct Shape {
    cover: bool,
    chapter_keys: bool,
    one_row: bool,
    side_by_side: bool,
}

impl Shape {
    const FULL: Self = Self {
        cover: true,
        chapter_keys: true,
        one_row: false,
        side_by_side: false,
    };
    const IN_ORDER: [Self; 5] = [
        Self::FULL,
        Self {
            cover: false,
            ..Self::FULL
        },
        Self {
            cover: false,
            chapter_keys: false,
            ..Self::FULL
        },
        Self {
            cover: false,
            chapter_keys: false,
            one_row: true,
            side_by_side: false,
        },
        Self {
            cover: false,
            chapter_keys: false,
            one_row: true,
            side_by_side: true,
        },
    ];
}

/// A complete audiobook surface: album art, transport controls, position,
/// volume, chapters, speed, a sleep timer, and the Bluetooth audio picker
/// used when Play has no output.
#[derive(Clone, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts the player has learned, not alternate states"
)]
pub struct AudioPlayer {
    source: AudioSource,
    metadata: AudioMetadata,
    cover: Option<TilePicture>,
    view: View,
    playback: AudioPlaybackState,
    position_ms: u32,
    duration_ms: u32,
    volume: u8,
    speed_percent: u16,
    loaded: LoadState,
    /// What the book said about itself, once the runtime has been asked.
    detail: Option<AudioDetail>,
    detail_asked: bool,
    /// A load has been answered and the book has not been seen ready yet.
    /// The runtime answers a load with Loading and says Ready later, or with
    /// Ready at once; the work that follows a load happens exactly once
    /// either way.
    pending_ready: bool,
    /// A cover the book carried, waiting for the application to draw it.
    cover_bytes: Option<Vec<u8>>,
    /// Where to go once the book is ready, before it plays.
    resume_ms: Option<u32>,
    /// When the book was last paused, for the rewind on resuming.
    paused_since: Option<Instant>,
    chapters_page: usize,
    sleep_page: usize,
    sleep: Sleep,
    sleep_task: Option<TaskId>,
    sleep_until: Option<Instant>,
    /// Set when the sleep timer paused the book, until the next press.
    slept: bool,
    audio_available: Availability,
    bluetooth_available: Availability,
    bluetooth_enabled: Switch,
    /// Whether the runtime has said that leaving will reboot the reader.
    restart_on_exit: bool,
    /// Whether the runtime has answered about Bluetooth yet. Until it has,
    /// a Play waits for the answer rather than opening the picker over
    /// headphones that are already connected.
    bluetooth_known: bool,
    devices: Vec<BluetoothDevice>,
    page: usize,
    pending_bluetooth: Option<PendingBluetooth>,
    delayed_scan: Option<TaskId>,
    poll: Option<TaskId>,
    autoplay: PlayIntent,
    trouble: Option<String>,
    secondary_action: Option<(String, String, Glyph)>,
    owns_back: bool,
    /// Whether the title the application gave stands even when the book
    /// says another. A listing names a book by its file; a book made by
    /// the application itself has a name its files do not carry.
    keep_title: bool,
}

impl AudioPlayer {
    #[must_use]
    pub fn new(source: AudioSource, metadata: AudioMetadata) -> Self {
        Self {
            source,
            metadata,
            cover: None,
            view: View::Player,
            playback: AudioPlaybackState::Idle,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
            speed_percent: 100,
            loaded: LoadState::Unloaded,
            detail: None,
            detail_asked: false,
            pending_ready: false,
            cover_bytes: None,
            resume_ms: None,
            paused_since: None,
            chapters_page: 0,
            sleep_page: 0,
            sleep: Sleep::Off,
            sleep_task: None,
            sleep_until: None,
            slept: false,
            audio_available: Availability::Available,
            bluetooth_available: Availability::Available,
            bluetooth_enabled: Switch::Off,
            restart_on_exit: false,
            bluetooth_known: false,
            devices: Vec::new(),
            page: 0,
            pending_bluetooth: None,
            delayed_scan: None,
            poll: None,
            autoplay: PlayIntent::Manual,
            trouble: None,
            secondary_action: None,
            owns_back: false,
            keep_title: false,
        }
    }

    #[must_use]
    pub fn shelf(name: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(AudioSource::Shelf(name.into()), AudioMetadata::new(title))
    }

    #[must_use]
    pub fn stream(url: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(AudioSource::Stream(url.into()), AudioMetadata::new(title))
    }

    /// A player for an audiobook the runtime listed, by its identifier.
    #[must_use]
    pub fn library(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::new(AudioSource::Library(id.into()), AudioMetadata::new(title))
    }

    #[must_use]
    pub fn metadata(mut self, metadata: AudioMetadata) -> Self {
        self.metadata = metadata;
        self
    }

    #[must_use]
    pub fn cover(mut self, cover: TilePicture) -> Self {
        self.cover = Some(cover);
        self
    }

    pub fn set_cover(&mut self, cover: Option<TilePicture>) {
        self.cover = cover;
    }

    /// Asks for the runtime's back control to be offered to the application
    /// first, for a player that was reached from a list rather than opened as
    /// the application's own front door. Without it a player at the root of a
    /// single-application session is drawn with no way back to the shelf that
    /// opened it, which strands whoever tapped a book.
    ///
    /// The application must answer [`ActionId::BACK`] with the screen to
    /// return to; if it does not, the runtime leaves the application anyway.
    #[must_use]
    pub const fn owns_back(mut self, owns_back: bool) -> Self {
        self.owns_back = owns_back;
        self
    }

    /// Adds one application-owned action beside the output picker. The player
    /// deliberately does not consume it; the embedding application receives
    /// the action normally.
    ///
    /// The mark is required rather than optional because the bar it lands in
    /// draws one: an action beside a marked one is the odd word out.
    #[must_use]
    pub fn secondary_action(
        mut self,
        name: impl Into<String>,
        label: impl Into<String>,
        glyph: Glyph,
    ) -> Self {
        self.secondary_action = Some((name.into(), label.into(), glyph));
        self
    }

    /// Keeps the title given even when the book says another: for a book
    /// whose name the application knows better than the file's tags do,
    /// such as one it made itself. Without this the book's own title wins,
    /// which is right for a file named by whoever copied it over.
    #[must_use]
    pub const fn keep_title(mut self) -> Self {
        self.keep_title = true;
        self
    }

    /// The speed to play at, as a percentage, sent to the runtime once the
    /// book is loaded and kept across loads.
    #[must_use]
    pub fn speed(mut self, percent: u16) -> Self {
        self.speed_percent = percent.clamp(MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
        self
    }

    /// Where to carry on from once the book is ready: the place an
    /// application remembered. Applied before the first Play, so the book
    /// never starts from the beginning and jumps.
    #[must_use]
    pub fn resuming_at(mut self, position: Duration) -> Self {
        self.resume_ms = Some(u32::try_from(position.as_millis()).unwrap_or(u32::MAX));
        self
    }

    /// Begins observation without loading audio or turning Bluetooth on.
    pub fn start(&mut self, context: &mut Context) {
        context.device().read_audio();
        context.device().read_bluetooth();
    }

    #[must_use]
    pub const fn is_playing(&self) -> bool {
        matches!(self.playback, AudioPlaybackState::Playing)
    }

    #[must_use]
    pub const fn is_finished(&self) -> bool {
        matches!(self.playback, AudioPlaybackState::Finished)
    }

    #[must_use]
    pub const fn playback(&self) -> AudioPlaybackState {
        self.playback
    }

    #[must_use]
    pub const fn position(&self) -> Duration {
        Duration::from_millis(self.position_ms as u64)
    }

    #[must_use]
    pub const fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms as u64)
    }

    #[must_use]
    pub const fn speed_percent(&self) -> u16 {
        self.speed_percent
    }

    /// What the book said about itself, once the runtime answered.
    #[must_use]
    pub const fn detail(&self) -> Option<&AudioDetail> {
        self.detail.as_ref()
    }

    /// The chapters of the book, in order, empty until the runtime answered
    /// or when the book has none.
    #[must_use]
    pub fn chapters(&self) -> &[AudioChapter] {
        self.detail
            .as_ref()
            .map_or(&[], |detail| detail.chapters.as_slice())
    }

    /// The chapter the position is in, and its index.
    #[must_use]
    pub fn chapter(&self) -> Option<(usize, &AudioChapter)> {
        let chapters = self.chapters();
        let index = chapters
            .iter()
            .rposition(|chapter| chapter.start_ms <= self.position_ms)?;
        Some((index, &chapters[index]))
    }

    /// The cover the book carried, handed over once for the application to
    /// decode and draw with [`Self::set_cover`]. The player cannot decode a
    /// picture itself; the application already knows how, and chooses the
    /// size.
    pub fn take_cover_bytes(&mut self) -> Option<Vec<u8>> {
        self.cover_bytes.take()
    }

    /// Sets the speed now, for a book that is loaded, and for the next one.
    pub fn set_speed(&mut self, context: &mut Context, percent: u16) {
        self.speed_percent = percent.clamp(MIN_SPEED_PERCENT, MAX_SPEED_PERCENT);
        if self.loaded == LoadState::Loaded {
            context.device().set_audio_speed(self.speed_percent);
        }
        self.schedule_sleep(context);
    }

    /// Skips half a minute either way, which is what the reader's page keys
    /// mean on a player.
    pub fn skip(&mut self, context: &mut Context, forward: bool) {
        if self.loaded != LoadState::Loaded {
            return;
        }
        let target = if forward {
            self.position_ms
                .saturating_add(SKIP_MS)
                .min(self.duration_ms)
        } else {
            self.position_ms.saturating_sub(SKIP_MS)
        };
        self.seek(context, target);
    }

    /// Plays, as a press on Play would: loading the book first if it is not
    /// loaded, and opening the headphones picker if nothing is connected.
    pub fn play(&mut self, context: &mut Context) {
        self.slept = false;
        if self.playback != AudioPlaybackState::Playing {
            self.request_play(context);
        }
    }

    /// Pauses, for an application that is leaving the player behind.
    pub fn pause(&mut self, context: &mut Context) {
        if self.playback == AudioPlaybackState::Playing {
            context.device().pause_audio();
        }
    }

    /// The player as it is drawn when nothing has to give way: cover,
    /// transport and volume each in their own place. Prefer
    /// [`Self::screen_for`], which knows the panel and the text size.
    #[must_use]
    pub fn screen(&self) -> Screen {
        match self.view {
            View::Player => self.player_screen(Shape::FULL),
            View::Bluetooth => self.bluetooth_screen(&Context::default()),
            View::Chapters => self.chapters_screen(&Context::default()),
            View::Speed => self.speed_screen(),
            View::Sleep => self.sleep_screen(&Context::default(), true),
        }
    }

    /// The player shaped for the panel and text size in `context`.
    ///
    /// At a large text size the full player is taller than a six inch panel,
    /// and the runtime refuses a screen that runs off it, which leaves the
    /// owner who needs big type with no player at all. So the parts that say
    /// the least give way first, one at a time and only as far as the panel
    /// needs: the cover, which repeats the title in a picture; the chapter
    /// keys, whose chapters are a tap away on their own screen; and then the
    /// separate volume row, whose two buttons join the transport and whose
    /// level becomes a fact beside the output it applies to. Every word and
    /// every control stays on the screen.
    #[must_use]
    pub fn screen_for(&self, context: &Context) -> Screen {
        match self.view {
            View::Bluetooth => return self.bluetooth_screen(context),
            View::Chapters => return self.chapters_screen(context),
            View::Speed => return self.speed_screen(),
            View::Sleep => {
                // The line saying when the timer fires is the first thing
                // to give way on a short panel; the choice itself never does.
                return first_that_fits(
                    context,
                    [
                        self.sleep_screen(context, true),
                        self.sleep_screen(context, false),
                    ],
                );
            }
            View::Player => {}
        }
        first_that_fits(
            context,
            Shape::IN_ORDER
                .into_iter()
                .map(|shape| self.player_screen(shape)),
        )
    }

    fn title(&self) -> &str {
        if self.keep_title && !self.metadata.title.is_empty() {
            return &self.metadata.title;
        }
        self.detail
            .as_ref()
            .map(|detail| detail.title.as_str())
            .filter(|title| !title.is_empty())
            .unwrap_or(&self.metadata.title)
    }

    /// The byline: the author the application gave, else the one the book
    /// gave, with its narrator when that is a different person.
    fn byline(&self) -> Option<String> {
        if let Some(author) = &self.metadata.author {
            return Some(author.clone());
        }
        let detail = self.detail.as_ref()?;
        // A narrator who is the author is one name, not two.
        let narrator_is_author = detail.narrator == detail.author;
        match (
            detail.author.is_empty(),
            detail.narrator.is_empty() || narrator_is_author,
        ) {
            (true, true) => None,
            (false, true) => Some(detail.author.clone()),
            (true, false) => Some(format!("Read by {}", detail.narrator)),
            (false, false) => Some(format!("{}, read by {}", detail.author, detail.narrator)),
        }
    }

    /// The chapter fact: the application's word for it, else the book's.
    fn chapter_fact(&self) -> Option<String> {
        if let Some(chapter) = self.metadata.chapter.as_deref() {
            return Some(chapter.to_owned());
        }
        let (index, chapter) = self.chapter()?;
        let count = self.chapters().len();
        Some(if chapter.title.is_empty() {
            format!("{} of {count}", index + 1)
        } else {
            format!("{} of {count} · {}", index + 1, chapter.title)
        })
    }

    /// How long is left in the book at the current speed.
    fn time_left(&self) -> String {
        let left_ms = u64::from(self.duration_ms.saturating_sub(self.position_ms));
        let at_speed = left_ms * 100 / u64::from(self.speed_percent.max(1));
        format!("{} left", span(u32::try_from(at_speed).unwrap_or(u32::MAX)))
    }

    fn player_screen(&self, shape: Shape) -> Screen {
        let output = self
            .connected_output()
            .map_or("Not connected", |device| device.name.as_str());
        // The author is the hero's byline, immediately under the title, which is
        // where a reader looks for it. Repeating it as an "Author" fact two lines
        // lower says the same sentence twice and reads like a bug, so the facts
        // list carries only what the byline cannot.
        let mut facts = Vec::new();
        if let Some(chapter) = self.chapter_fact() {
            facts.push(("Chapter", chapter));
        }
        if let Some(sleep) = self.sleep_fact() {
            facts.push(("Sleep", sleep));
        }
        facts.push(("Output", output.to_owned()));
        let controls_shown = self.audio_available != Availability::Unavailable;
        if controls_shown && shape.one_row {
            facts.push(("Volume", format!("{}%", self.volume)));
        }
        let mut screen = ScreenBuilder::new("audio-player")
            .top_bar("Now playing")
            .owns_back(self.owns_back);
        // The unavailable/trouble banner leads the content: appended last it
        // lands under the volume controls, where the fixed bottom bar clips
        // it on the smallest panels at extra-large text. Leading, it reads
        // first and the hero absorbs the space pressure instead.
        if self.audio_available == Availability::Unavailable {
            screen = screen.banner(BannerLevel::Attention, "This firmware cannot play audio.");
        } else if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble.clone());
        } else if self.slept {
            screen = screen.banner(BannerLevel::Info, "Paused by the sleep timer.");
        } else if self.playback == AudioPlaybackState::Finished {
            screen = screen.banner(BannerLevel::Success, "Finished. Play starts it again.");
        }
        let title = self.title().to_owned();
        let byline = self.byline();
        let hero = |screen: ScreenBuilder| {
            screen.hero(
                self.cover.filter(|_| shape.cover),
                26,
                &title,
                byline,
                facts,
            )
        };
        let transport = |screen: ScreenBuilder| self.transport(screen, shape);
        screen = if shape.side_by_side {
            // Held sideways the panel is wide and short, so the book and its
            // controls stand beside each other rather than one above the
            // other. On a panel too narrow for two readable columns the band
            // stacks itself, which is the shape before this one.
            screen.band(
                BandAlign::Top,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(hero) as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (SlotWidth::Fill, Box::new(transport)),
                ],
            )
        } else {
            transport(hero(screen))
        };
        // Marked, because these are the two verbs a player screen offers and
        // both have a picture everyone already reads. The words stay: they are
        // what the control is called in a test, a log and a preview.
        screen = match &self.secondary_action {
            Some((name, label, glyph)) => screen.action_bar_marked([
                (
                    OUTPUT.to_owned(),
                    "Bluetooth output".to_owned(),
                    Some(Glyph::Bluetooth),
                ),
                (name.clone(), label.clone(), Some(*glyph)),
            ]),
            None => screen.bottom_action_marked(OUTPUT, "Bluetooth audio output", Glyph::Bluetooth),
        };
        screen.build()
    }

    /// The progress, the position and the controls under the book.
    ///
    /// Where playback is unavailable (no audio hardware, as on the
    /// black-and-white panels), the transport and volume controls would be
    /// inert replicas of what the banner already says, and their two rows
    /// push the screen past the small panels at extra-large text. The
    /// position and progress stay: they are the saved state, not controls.
    fn transport(&self, screen: ScreenBuilder, shape: Shape) -> ScreenBuilder {
        let progress = if self.duration_ms == 0 {
            0
        } else {
            u8::try_from(
                u64::from(self.position_ms)
                    .saturating_mul(100)
                    .checked_div(u64::from(self.duration_ms))
                    .unwrap_or(0),
            )
            .unwrap_or(100)
            .min(100)
        };
        // The transport is drawn as pictures, so the button can no longer
        // spell "Loading…" at anybody. The word moves to the line that was
        // going to change anyway: the control says what it does, and the
        // position says what is happening. This label is still the action's
        // name, which is what a reader would be told out loud.
        let play_label = match self.playback {
            AudioPlaybackState::Playing => "Pause",
            _ => "Play",
        };
        // Loading keeps the play triangle rather than borrowing another
        // picture: the button is about to become Play, and swapping the icon
        // for a third shape mid-fetch reads as a different control.
        let play_glyph = match self.playback {
            AudioPlaybackState::Playing => Glyph::Pause,
            _ => Glyph::Play,
        };
        let position = if self.playback == AudioPlaybackState::Loading {
            "Loading\u{2026}".to_owned()
        } else {
            format!("{} of {}", clock(self.position_ms), clock(self.duration_ms))
        };
        let controls_shown = self.audio_available != Availability::Unavailable;
        let screen = screen
            .progress(progress)
            .section_with_value("Position", position);
        if !controls_shown {
            return screen;
        }
        let screen = if shape.one_row {
            // Quieter and Louder at the two ends, where a thumb finds
            // them without looking, and the transport in the middle as
            // before.
            screen.controls(
                5,
                [
                    (VOLUME_DOWN, "Quieter", Glyph::VolumeDown),
                    (BACK_THIRTY, "Back 30 sec", Glyph::Rewind30),
                    (PLAY, play_label, play_glyph),
                    (FORWARD_THIRTY, "Forward 30 sec", Glyph::Forward30),
                    (VOLUME_UP, "Louder", Glyph::VolumeUp),
                ],
            )
        } else {
            let screen = if shape.chapter_keys && !self.chapters().is_empty() {
                screen.controls(
                    5,
                    [
                        (CHAPTER_BACK, "Previous chapter", Glyph::Previous),
                        (BACK_THIRTY, "Back 30 sec", Glyph::Rewind30),
                        (PLAY, play_label, play_glyph),
                        (FORWARD_THIRTY, "Forward 30 sec", Glyph::Forward30),
                        (CHAPTER_FORWARD, "Next chapter", Glyph::Next),
                    ],
                )
            } else {
                screen.controls(
                    3,
                    [
                        (BACK_THIRTY, "Back 30 sec", Glyph::Rewind30),
                        (PLAY, play_label, play_glyph),
                        (FORWARD_THIRTY, "Forward 30 sec", Glyph::Forward30),
                    ],
                )
            };
            // The volume is stated once, above the pair, rather than
            // printed on both buttons. Two buttons carrying the same
            // number is the same fault as a byline stated twice: it reads
            // as two facts.
            screen
                .section_with_value("Volume", format!("{}%", self.volume))
                .controls(
                    2,
                    [
                        (VOLUME_DOWN, "Quieter", Glyph::VolumeDown),
                        (VOLUME_UP, "Louder", Glyph::VolumeUp),
                    ],
                )
        };
        // Three words rather than three rows: the chapter is a fact in the
        // hero, the speed is on its own button, and the sleep timer says
        // when it is set. What these open is where the details live.
        screen.buttons([
            (CHAPTERS, "Chapters".to_owned()),
            (SPEED, speed_label(self.speed_percent)),
            (SLEEP, "Sleep".to_owned()),
        ])
    }

    fn sleep_fact(&self) -> Option<String> {
        match self.sleep {
            Sleep::Off => None,
            Sleep::Minutes(_) => {
                let left = self
                    .sleep_until
                    .map(|until| until.saturating_duration_since(Instant::now()))?;
                let minutes = (left.as_secs() + 59) / 60;
                Some(if minutes <= 1 {
                    "Pauses in under a minute".to_owned()
                } else {
                    format!("Pauses in {minutes} min")
                })
            }
            Sleep::EndOfChapter => Some(if self.chapters().is_empty() {
                "Pauses at the end of the book".to_owned()
            } else {
                "Pauses at the end of this chapter".to_owned()
            }),
        }
    }

    /// The chapters, a page at a time, with the one playing said so.
    fn chapters_screen(&self, context: &Context) -> Screen {
        let screen = ScreenBuilder::new("audio-chapters")
            .top_bar("Chapters")
            .owns_back(true);
        let chapters = self.chapters();
        if chapters.is_empty() {
            return screen
                .empty_state("This book has no chapters. Skip through it with the arrows.")
                .build();
        }
        let current = self.chapter().map(|(index, _)| index);
        let rows = self.chapter_rows(current);
        let pages = Self::chapter_pages(context, &rows);
        let page = self.chapters_page.min(pages.len().saturating_sub(1));
        let showing = pages.get(page).map_or(&[][..], Vec::as_slice);
        let mut screen =
            screen.rows_with_trailing(showing.iter().enumerate().filter_map(|(slot, index)| {
                let (title, summary, trailing) = rows.get(*index)?;
                Some((
                    CHAPTER_ACTIONS.get(slot)?.to_owned(),
                    title.clone(),
                    summary.clone(),
                    RowLead::Number(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                    trailing.clone(),
                ))
            }));
        if pages.len() > 1 {
            screen = screen
                .page_turns(CHAPTERS_PREVIOUS, CHAPTERS_NEXT)
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
        }
        screen.build()
    }

    /// Each chapter as a row: its name, how long it is and whether it is
    /// playing, and where it starts.
    fn chapter_rows(&self, current: Option<usize>) -> Vec<(String, String, String)> {
        let chapters = self.chapters();
        chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| {
                let end = chapters
                    .get(index + 1)
                    .map_or(self.duration_ms, |next| next.start_ms);
                let length = span(end.saturating_sub(chapter.start_ms));
                let title = if chapter.title.is_empty() {
                    format!("Chapter {}", index + 1)
                } else {
                    chapter.title.clone()
                };
                let summary = if current == Some(index) {
                    format!("Now · {length}")
                } else {
                    length
                };
                (title, summary, clock(chapter.start_ms))
            })
            .collect()
    }

    fn chapter_pages(context: &Context, rows: &[(String, String, String)]) -> Vec<Vec<usize>> {
        let measured = rows
            .iter()
            .map(|(title, summary, trailing)| (title.as_str(), summary.as_str(), trailing.as_str()))
            .collect::<Vec<_>>();
        let pages = context
            .paginate_rows_with_trailing_at(&measured, false, crate::Position::AtTheFoot)
            .into_iter()
            .flat_map(|page| {
                page.chunks(CHAPTER_ACTIONS.len())
                    .map(<[usize]>::to_vec)
                    .collect::<Vec<_>>()
            })
            .filter(|page| !page.is_empty())
            .collect::<Vec<_>>();
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// The speed as a stepper over a line, with the speeds nearly everyone
    /// uses as one tap each.
    fn speed_screen(&self) -> Screen {
        let percent = self.speed_percent;
        let along = u8::try_from(
            u32::from(percent - MIN_SPEED_PERCENT) * 100
                / u32::from(MAX_SPEED_PERCENT - MIN_SPEED_PERCENT),
        )
        .unwrap_or(100);
        ScreenBuilder::new("audio-speed")
            .top_bar("Speed")
            .owns_back(true)
            .stepper(
                speed_label(percent),
                SPEED_LESS,
                Glyph::Minus,
                SPEED_MORE,
                Glyph::Plus,
            )
            .stepper_ends(percent > MIN_SPEED_PERCENT, percent < MAX_SPEED_PERCENT)
            .stepper_track(along)
            .chips(SPEED_PRESETS.map(|(name, preset)| {
                (name, speed_label(preset), preset == percent)
            }))
            .secondary(format!(
                "{} at this speed. The voice keeps its pitch; speech stays clear to about twice as fast.",
                self.time_left()
            ))
            .build()
    }

    /// When to pause, as one choice, paged by what the panel holds.
    fn sleep_screen(&self, context: &Context, with_fact: bool) -> Screen {
        let chosen = SLEEP_OPTIONS
            .iter()
            .position(|(_, sleep)| *sleep == self.sleep)
            .unwrap_or(0);
        let chapters = !self.chapters().is_empty();
        let pages = Self::sleep_pages(context);
        let page = self.sleep_page.min(pages.saturating_sub(1));
        let mut screen = ScreenBuilder::new("audio-sleep")
            .top_bar("Sleep timer")
            .owns_back(true)
            .option_list(
                "Pause playback",
                u16::try_from(page).unwrap_or(u16::MAX),
                SLEEP_PREVIOUS,
                SLEEP_NEXT,
                SLEEP_OPTIONS.map(|(name, sleep)| {
                    (
                        name,
                        match sleep {
                            Sleep::Off => "Not on a timer".to_owned(),
                            Sleep::Minutes(minutes) => format!("In {minutes} minutes"),
                            Sleep::EndOfChapter if chapters => {
                                "At the end of this chapter".to_owned()
                            }
                            Sleep::EndOfChapter => "At the end of the book".to_owned(),
                        },
                        String::new(),
                    )
                }),
            )
            .chosen(chosen);
        if let Some(fact) = self.sleep_fact().filter(|_| with_fact) {
            screen = screen.secondary(format!("{fact}."));
        }
        screen.build()
    }

    fn sleep_pages(context: &Context) -> usize {
        kobo_ui::options_pages(SLEEP_OPTIONS.len(), &context.metrics(), false)
    }

    /// The page the current answer is on.
    fn sleep_page_of_answer(&self, context: &Context) -> usize {
        let chosen = SLEEP_OPTIONS
            .iter()
            .position(|(_, sleep)| *sleep == self.sleep)
            .unwrap_or(0);
        chosen / kobo_ui::options_per_page(&context.metrics(), false).max(1)
    }

    /// Everything on the output screen above the list of devices.
    fn bluetooth_header(&self) -> ScreenBuilder {
        let on = self.bluetooth_enabled == Switch::On;
        let mut screen = ScreenBuilder::new("audio-output")
            .top_bar("Bluetooth audio")
            .owns_back(true);
        if self.bluetooth_available == Availability::Unavailable {
            screen = screen.banner(
                BannerLevel::Attention,
                "Bluetooth is unavailable on this firmware.",
            );
        } else if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble);
        } else if self.restart_on_exit {
            // Said before it happens, not after. Bluetooth and Wi-Fi share one
            // radio here whose driver starts once per boot, so the only safe
            // way back to the reader is a restart. Unannounced, that is
            // indistinguishable from a crash, which is exactly how it was read.
            screen = screen.banner(
                BannerLevel::Info,
                "The reader will restart itself when you leave. Nothing saved is lost.",
            );
        }
        // One switch that shows its state, in place of a line saying "On"
        // and a button saying the opposite. Switched on, the list under it
        // says what is connected, so the switch says nothing more and leaves
        // that line to the list.
        let summary = if on {
            ""
        } else {
            "Turn on to find headphones and speakers"
        };
        screen.toggle(BLUETOOTH_TOGGLE, "Bluetooth", summary, on)
    }

    /// The devices on each page of the output screen, as many as the panel
    /// holds under the switch and over the scan button at this text size.
    fn device_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let devices = self.audio_devices().collect::<Vec<_>>();
        if devices.is_empty() {
            return vec![Vec::new()];
        }
        let rows = devices
            .iter()
            .map(|device| (device.name.as_str(), device_state(device)))
            .collect::<Vec<_>>();
        let placed = self.bluetooth_header().section("Audio devices").build();
        let pages = context
            .paginate_rows_under(&rows, true, crate::Position::AtTheFoot, &placed)
            .into_iter()
            .flat_map(|page| {
                page.chunks(DEVICE_ACTIONS.len())
                    .map(<[usize]>::to_vec)
                    .collect::<Vec<_>>()
            })
            .filter(|page| !page.is_empty())
            .collect::<Vec<_>>();
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn bluetooth_screen(&self, context: &Context) -> Screen {
        let mut screen = self.bluetooth_header();
        if self.bluetooth_enabled != Switch::On {
            return screen.build();
        }
        let devices = self.audio_devices().collect::<Vec<_>>();
        if devices.is_empty() {
            screen = screen
                .text("Put headphones or a speaker in pairing mode, then scan for audio devices.");
        } else {
            let pages = self.device_pages(context);
            let page = self.page.min(pages.len() - 1);
            screen = screen
                .section("Audio devices")
                .rows(pages[page].iter().enumerate().map(|(slot, &index)| {
                    let device = devices[index];
                    (
                        DEVICE_ACTIONS[slot],
                        device.name.as_str(),
                        device_state(device),
                        RowLead::from(if device.connected {
                            Glyph::Check
                        } else {
                            Glyph::Circle
                        }),
                    )
                }));
            if pages.len() > 1 {
                screen = screen
                    .page_turns(DEVICES_PREVIOUS, DEVICES_NEXT)
                    .page_position(
                        u16::try_from(page + 1).unwrap_or(u16::MAX),
                        u16::try_from(pages.len()).unwrap_or(u16::MAX),
                    );
            }
        }
        screen
            .bottom_action(BLUETOOTH_SCAN, "Scan for headphones and speakers")
            .build()
    }

    /// Handles one action if it belongs to the player. Returns whether it was
    /// consumed so an embedding application can handle everything else.
    pub fn press(&mut self, context: &mut Context, action: ActionId) -> bool {
        self.slept = false;
        if action == ActionId::BACK && self.view != View::Player {
            self.view = View::Player;
            self.trouble = None;
            return true;
        }
        if action == action_id(OUTPUT) {
            self.view = View::Bluetooth;
            self.trouble = None;
            context.device().read_bluetooth();
            return true;
        }
        if action == action_id(CHAPTERS) {
            self.view = View::Chapters;
            // Open on the page the playing chapter is on.
            self.chapters_page = self.page_of_current_chapter(context);
            return true;
        }
        if action == action_id(SPEED) {
            self.view = View::Speed;
            return true;
        }
        if action == action_id(SLEEP) {
            self.view = View::Sleep;
            self.sleep_page = self.sleep_page_of_answer(context);
            return true;
        }
        match self.view {
            View::Bluetooth => return self.press_bluetooth(context, action),
            View::Chapters => return self.press_chapters(context, action),
            View::Speed => return self.press_speed(context, action),
            View::Sleep => return self.press_sleep(context, action),
            View::Player => {}
        }
        if action == action_id(PLAY) {
            if self.playback == AudioPlaybackState::Playing {
                context.device().pause_audio();
            } else {
                self.request_play(context);
            }
            return true;
        }
        if action == action_id(BACK_THIRTY) {
            self.skip(context, false);
            return true;
        }
        if action == action_id(FORWARD_THIRTY) {
            self.skip(context, true);
            return true;
        }
        if action == action_id(CHAPTER_BACK) {
            self.chapter_step(context, false);
            return true;
        }
        if action == action_id(CHAPTER_FORWARD) {
            self.chapter_step(context, true);
            return true;
        }
        if action == action_id(VOLUME_DOWN) {
            context
                .device()
                .set_audio_volume(self.volume.saturating_sub(10));
            return true;
        }
        if action == action_id(VOLUME_UP) {
            context
                .device()
                .set_audio_volume(self.volume.saturating_add(10).min(100));
            return true;
        }
        false
    }

    fn page_of_current_chapter(&self, context: &Context) -> usize {
        let Some((current, _)) = self.chapter() else {
            return 0;
        };
        let rows = self.chapter_rows(Some(current));
        Self::chapter_pages(context, &rows)
            .iter()
            .position(|page| page.contains(&current))
            .unwrap_or(0)
    }

    /// To the start of the previous or next chapter. Back within the first
    /// few seconds of a chapter goes to the one before, as every player
    /// does; later it goes to the start of this one.
    fn chapter_step(&mut self, context: &mut Context, forward: bool) {
        let chapters = self.chapters();
        if chapters.is_empty() {
            self.skip(context, forward);
            return;
        }
        let Some((index, chapter)) = self.chapter() else {
            self.seek(context, 0);
            return;
        };
        let target = if forward {
            chapters
                .get(index + 1)
                .map_or(self.duration_ms, |next| next.start_ms)
        } else if self.position_ms.saturating_sub(chapter.start_ms) > 3_000 || index == 0 {
            chapter.start_ms
        } else {
            chapters[index - 1].start_ms
        };
        self.seek(context, target);
    }

    fn press_chapters(&mut self, context: &mut Context, action: ActionId) -> bool {
        let rows = self.chapter_rows(self.chapter().map(|(index, _)| index));
        let pages = Self::chapter_pages(context, &rows);
        if action == action_id(CHAPTERS_PREVIOUS) {
            self.chapters_page = self.chapters_page.saturating_sub(1);
            return true;
        }
        if action == action_id(CHAPTERS_NEXT) {
            self.chapters_page = (self.chapters_page + 1).min(pages.len().saturating_sub(1));
            return true;
        }
        let Some(slot) = CHAPTER_ACTIONS
            .iter()
            .position(|name| action == action_id(name))
        else {
            return false;
        };
        let Some(start_ms) = pages
            .get(self.chapters_page.min(pages.len().saturating_sub(1)))
            .and_then(|page| page.get(slot))
            .and_then(|&index| self.chapters().get(index))
            .map(|chapter| chapter.start_ms)
        else {
            return true;
        };
        self.view = View::Player;
        self.seek(context, start_ms);
        if self.playback != AudioPlaybackState::Playing {
            self.request_play(context);
        }
        true
    }

    fn press_speed(&mut self, context: &mut Context, action: ActionId) -> bool {
        let wanted = if action == action_id(SPEED_LESS) {
            Some(self.speed_percent.saturating_sub(SPEED_STEP))
        } else if action == action_id(SPEED_MORE) {
            Some(self.speed_percent.saturating_add(SPEED_STEP))
        } else {
            SPEED_PRESETS
                .iter()
                .find(|(name, _)| action == action_id(name))
                .map(|(_, preset)| *preset)
        };
        let Some(wanted) = wanted else {
            return false;
        };
        self.set_speed(context, wanted);
        true
    }

    fn press_sleep(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(SLEEP_PREVIOUS) {
            self.sleep_page = self.sleep_page.saturating_sub(1);
            return true;
        }
        if action == action_id(SLEEP_NEXT) {
            self.sleep_page =
                (self.sleep_page + 1).min(Self::sleep_pages(context).saturating_sub(1));
            return true;
        }
        let Some((_, sleep)) = SLEEP_OPTIONS
            .iter()
            .find(|(name, _)| action == action_id(name))
        else {
            return false;
        };
        self.sleep = *sleep;
        self.schedule_sleep(context);
        self.view = View::Player;
        true
    }

    /// Arranges to be woken when the sleep timer is due, or cancels the
    /// arrangement. Called again whenever what "due" means has changed: a
    /// seek, a chapter change, a new speed.
    fn schedule_sleep(&mut self, context: &mut Context) {
        if let Some(task) = self.sleep_task.take() {
            context.cancel(task);
        }
        // A timer set in minutes keeps counting from where it was: a seek
        // or a new speed changes when a chapter ends, not when the
        // listener asked to stop.
        let remaining = self.sleep_until.take().map(|until| {
            u32::try_from(until.saturating_duration_since(Instant::now()).as_secs())
                .unwrap_or(u32::MAX)
                .max(1)
        });
        let seconds = match self.sleep {
            Sleep::Off => return,
            Sleep::Minutes(minutes) => remaining.unwrap_or(u32::from(minutes) * 60),
            Sleep::EndOfChapter => {
                if self.playback != AudioPlaybackState::Playing {
                    // Scheduled once the book plays, from wherever it is then.
                    return;
                }
                let end = self.chapter().map_or(self.duration_ms, |(index, _)| {
                    self.chapters()
                        .get(index + 1)
                        .map_or(self.duration_ms, |next| next.start_ms)
                });
                let left_ms = u64::from(end.saturating_sub(self.position_ms));
                let wall_ms = left_ms * 100 / u64::from(self.speed_percent.max(1));
                u32::try_from(wall_ms / 1_000).unwrap_or(u32::MAX).max(1)
            }
        };
        self.sleep_until = Some(Instant::now() + Duration::from_secs(u64::from(seconds)));
        self.sleep_task = context.spawn(Task::Sleep { seconds });
        if self.sleep_task.is_none() {
            self.sleep = Sleep::Off;
            self.sleep_until = None;
        }
    }

    fn press_bluetooth(&mut self, context: &mut Context, action: ActionId) -> bool {
        if action == action_id(BLUETOOTH_TOGGLE) {
            context
                .device()
                .set_bluetooth(self.bluetooth_enabled == Switch::Off);
            return true;
        }
        if action == action_id(BLUETOOTH_SCAN) {
            self.scan(context);
            return true;
        }
        let pages = self.device_pages(context);
        if action == action_id(DEVICES_PREVIOUS) {
            self.page = self.page.saturating_sub(1);
            return true;
        }
        if action == action_id(DEVICES_NEXT) {
            self.page = (self.page + 1).min(pages.len() - 1);
            return true;
        }
        let Some(slot) = DEVICE_ACTIONS
            .iter()
            .position(|name| action == action_id(name))
        else {
            return false;
        };
        let Some(device) = pages
            .get(self.page.min(pages.len() - 1))
            .and_then(|page| page.get(slot))
            .and_then(|&index| self.audio_devices().nth(index))
            .cloned()
        else {
            return true;
        };
        self.trouble = None;
        if device.connected {
            if self.autoplay == PlayIntent::Autoplay {
                self.view = View::Player;
                self.begin_audio(context);
            }
        } else if device.paired {
            context.device().connect_bluetooth(device.address);
        } else {
            self.pending_bluetooth = Some(PendingBluetooth::Connect(device.address.clone()));
            context.device().pair_bluetooth(device.address);
        }
        true
    }

    fn request_play(&mut self, context: &mut Context) {
        self.autoplay = PlayIntent::Autoplay;
        self.trouble = None;
        if !self.bluetooth_known {
            // The reading asked for at the start carries on from here.
            return;
        }
        if self.connected_output().is_some() {
            self.begin_audio(context);
        } else {
            self.view = View::Bluetooth;
            if self.bluetooth_enabled == Switch::On {
                self.scan(context);
            } else {
                context.device().set_bluetooth(true);
            }
        }
    }

    fn begin_audio(&mut self, context: &mut Context) {
        self.view = View::Player;
        if self.loaded == LoadState::Loaded {
            // Picking a paused book up again a little before where it
            // stopped is how a listener finds the thread. A short pause
            // needs nothing; an hour wants a sentence; a day wants a few.
            let rewind = rewind_after(self.paused_since.map(|since| since.elapsed()));
            if rewind > 0 && self.playback == AudioPlaybackState::Paused {
                self.seek(context, self.position_ms.saturating_sub(rewind));
            }
            context.device().play_audio();
        } else {
            context.device().load_audio(self.source.clone());
        }
    }

    fn seek(&mut self, context: &mut Context, position_ms: u32) {
        let position_ms = position_ms.min(self.duration_ms);
        self.position_ms = position_ms;
        context
            .device()
            .seek_audio(Duration::from_millis(u64::from(position_ms)));
        self.schedule_sleep(context);
    }

    fn scan(&mut self, context: &mut Context) {
        self.page = 0;
        self.trouble = None;
        context.device().scan_bluetooth();
    }

    /// Handles one device result if it belongs to audio or Bluetooth.
    pub fn on_device_result(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) -> bool {
        if is_audio_request(request) {
            self.audio_result(context, request, result);
            return true;
        }
        if is_bluetooth_request(request) {
            self.bluetooth_result(context, request, result);
            return true;
        }
        false
    }

    fn audio_result(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) {
        if matches!(request, DeviceRequest::ReadAudioDetail) {
            // A description that cannot be had is not trouble: an older
            // runtime has none to give, and the book still plays.
            if let DeviceResult::AudioDetail(detail) = result {
                self.cover_bytes = (!detail.cover.is_empty()).then(|| detail.cover.clone());
                let mut detail = detail.clone();
                detail.cover.clear();
                self.detail = Some(detail);
            }
            return;
        }
        match result {
            DeviceResult::Audio {
                available,
                state,
                position_ms,
                duration_ms,
                volume,
            } => {
                self.audio_available = if *available {
                    Availability::Available
                } else {
                    Availability::Unavailable
                };
                self.volume = *volume;
                if matches!(request, DeviceRequest::LoadAudio { .. }) {
                    self.loaded = if *state == AudioPlaybackState::Idle {
                        LoadState::Unloaded
                    } else {
                        LoadState::Loaded
                    };
                    self.pending_ready = self.loaded == LoadState::Loaded;
                    self.detail_asked = false;
                }
                // Position and duration describe whatever the backend last
                // loaded, which before our own load is the previous book. A
                // player that has not loaded anything yet must not adopt them,
                // or opening a thirty second file straight after a ten minute
                // one shows ten minutes until decoding finishes. Availability
                // and volume are properties of the device rather than the
                // book, so they are always ours to take.
                if self.loaded == LoadState::Unloaded {
                    return;
                }
                let was = self.playback;
                self.playback = *state;
                self.position_ms = *position_ms;
                self.duration_ms = *duration_ms;
                self.trouble = None;
                if *state == AudioPlaybackState::Paused && was != AudioPlaybackState::Paused {
                    self.paused_since = Some(Instant::now());
                } else if *state == AudioPlaybackState::Playing {
                    self.paused_since = None;
                }
                if self.pending_ready && *state != AudioPlaybackState::Loading {
                    self.pending_ready = false;
                    let resuming = self.resume_ms.is_some();
                    self.just_loaded(context);
                    if resuming {
                        // The seek's answer carries on from here.
                        return;
                    }
                }
                if self.autoplay == PlayIntent::Autoplay && *state == AudioPlaybackState::Ready {
                    context.device().play_audio();
                } else if poll_worthy(*state) {
                    if *state == AudioPlaybackState::Playing {
                        self.autoplay = PlayIntent::Manual;
                        if self.sleep == Sleep::EndOfChapter && self.sleep_task.is_none() {
                            self.schedule_sleep(context);
                        }
                    }
                    self.schedule_poll(context, *state);
                }
                if *state == AudioPlaybackState::Finished {
                    self.sleep = Sleep::Off;
                    self.schedule_sleep(context);
                }
            }
            DeviceResult::Denied(reason) => {
                self.audio_available = if *reason == DenyReason::Unsupported {
                    Availability::Unavailable
                } else {
                    Availability::Available
                };
                self.trouble = Some(format!("Audio was refused: {reason}."));
                self.autoplay = PlayIntent::Manual;
            }
            DeviceResult::Failed(error) => {
                if matches!(request, DeviceRequest::LoadAudio { .. }) {
                    self.loaded = LoadState::Unloaded;
                    self.playback = AudioPlaybackState::Idle;
                }
                // A sink that went away leaves the book paused where it
                // was. The complaint is shown; the place is kept.
                if *error == DeviceError::Unreachable && self.loaded == LoadState::Loaded {
                    if self.playback == AudioPlaybackState::Playing {
                        self.paused_since = Some(Instant::now());
                    }
                    self.playback = AudioPlaybackState::Paused;
                }
                self.trouble = Some(audio_error(*error).to_owned());
                self.autoplay = PlayIntent::Manual;
            }
            _ => {}
        }
    }

    /// The book is ready: learn what it says about itself, set its speed,
    /// and go to where the listener left it.
    fn just_loaded(&mut self, context: &mut Context) {
        if !self.detail_asked {
            self.detail_asked = true;
            context.device().read_audio_detail();
        }
        if self.speed_percent != 100 {
            context.device().set_audio_speed(self.speed_percent);
        }
        if let Some(resume_ms) = self.resume_ms.take() {
            self.seek(context, resume_ms);
        }
    }

    fn bluetooth_result(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) {
        let first_reading = !self.bluetooth_known;
        self.bluetooth_known = true;
        match result {
            DeviceResult::Bluetooth {
                available,
                enabled,
                devices,
                restart_on_exit,
            } => {
                self.bluetooth_available = if *available {
                    Availability::Available
                } else {
                    Availability::Unavailable
                };
                self.bluetooth_enabled = if *enabled { Switch::On } else { Switch::Off };
                // Latched, never cleared. Once the shared radio has been
                // touched the reboot is owed for the rest of the session, so a
                // later reading that happens to report false must not withdraw
                // a warning the reader has already been given.
                self.restart_on_exit |= *restart_on_exit;
                self.devices.clone_from(devices);
                self.page = self.page.min(self.device_pages(context).len() - 1);
                self.trouble = None;

                if matches!(request, DeviceRequest::PairBluetooth { .. }) {
                    if let Some(PendingBluetooth::Connect(address)) = self.pending_bluetooth.take()
                    {
                        context.device().connect_bluetooth(address);
                        return;
                    }
                }
                if self.autoplay == PlayIntent::Autoplay && self.connected_output().is_some() {
                    // Whatever brought the headphones: a connect that was
                    // asked for, a radio switched on that found its old
                    // pair, or a scan that saw them arrive. The Play that
                    // was waiting for an output has one.
                    self.begin_audio(context);
                } else if matches!(request, DeviceRequest::SetBluetooth { enabled: true }) {
                    self.scan(context);
                } else if first_reading && self.autoplay == PlayIntent::Autoplay {
                    // A Play that was waiting to learn what is connected.
                    self.request_play(context);
                }
            }
            DeviceResult::Done => {
                if matches!(request, DeviceRequest::PairBluetooth { .. }) {
                    if let Some(PendingBluetooth::Connect(address)) = self.pending_bluetooth.take()
                    {
                        context.device().connect_bluetooth(address);
                    }
                } else if matches!(request, DeviceRequest::ConnectBluetooth { .. }) {
                    context.device().read_bluetooth();
                }
            }
            DeviceResult::Denied(reason) => {
                self.bluetooth_available = if *reason == DenyReason::Unsupported {
                    Availability::Unavailable
                } else {
                    Availability::Available
                };
                self.trouble = Some(format!("Bluetooth was refused: {reason}."));
                if first_reading && self.autoplay == PlayIntent::Autoplay {
                    // Nothing to learn about outputs here; the sink itself
                    // says whether sound can go anywhere.
                    self.begin_audio(context);
                }
            }
            DeviceResult::Failed(error) => {
                self.trouble = Some(bluetooth_failure(request, *error));
                if first_reading && self.autoplay == PlayIntent::Autoplay {
                    self.begin_audio(context);
                }
            }
            _ => {}
        }
        if matches!(request, DeviceRequest::ScanBluetooth) {
            self.delayed_scan = context.spawn(Task::Sleep {
                seconds: SCAN_SECONDS,
            });
            if self.delayed_scan.is_none() {
                context.device().read_bluetooth();
            }
        }
    }

    /// Handles delayed scan collection, position polling and the sleep
    /// timer.
    pub fn on_task(&mut self, context: &mut Context, task: TaskId, _outcome: &TaskOutcome) -> bool {
        if self.delayed_scan == Some(task) {
            self.delayed_scan = None;
            context.device().read_bluetooth();
            return true;
        }
        if self.poll == Some(task) {
            self.poll = None;
            if poll_worthy(self.playback) {
                context.device().read_audio();
            }
            return true;
        }
        if self.sleep_task == Some(task) {
            self.sleep_task = None;
            self.sleep_until = None;
            if self.playback == AudioPlaybackState::Playing {
                context.device().pause_audio();
                self.slept = true;
            }
            self.sleep = Sleep::Off;
            return true;
        }
        false
    }

    fn schedule_poll(&mut self, context: &mut Context, state: AudioPlaybackState) {
        if self.poll.is_none() {
            let seconds = if state == AudioPlaybackState::Loading {
                LOADING_POLL_SECONDS
            } else {
                POLL_SECONDS
            };
            self.poll = context.spawn(Task::Sleep { seconds });
        }
    }

    fn audio_devices(&self) -> impl Iterator<Item = &BluetoothDevice> {
        self.devices
            .iter()
            .filter(|device| device.kind == BluetoothDeviceKind::Audio)
    }

    fn connected_output(&self) -> Option<&BluetoothDevice> {
        self.audio_devices().find(|device| device.connected)
    }
}

/// The first of `candidates` the panel in `context` draws without error,
/// or the last of them when none does: every candidate is the same screen
/// giving a little more way, so the last is the one that has given the
/// most.
fn first_that_fits(context: &Context, candidates: impl IntoIterator<Item = Screen>) -> Screen {
    let metrics = context.metrics();
    let chrome = crate::Chrome::measuring(true);
    let mut last = None;
    for screen in candidates {
        let fits = !screen
            .diagnostics(&metrics, &chrome)
            .issues
            .iter()
            .any(|issue| issue.severity == crate::DiagnosticSeverity::Error);
        if fits {
            return screen;
        }
        last = Some(screen);
    }
    last.unwrap_or_else(|| ScreenBuilder::new("audio-player").build())
}

/// Whether this playback state is one the backend will move on from by
/// itself, and so one worth asking about again.
///
/// Loading belongs here as much as Playing does. It was once missing, and the
/// consequence was that the state which schedules a poll and the state which
/// honours one had drifted apart: a player would arrange to ask again while
/// decoding, then refuse its own question when the answer came due, and sit on
/// "Loading…" forever. Pressing play a second time appeared to fix it, because
/// that took the already-loaded path. Both sides ask this function now so they
/// cannot disagree again.
const fn poll_worthy(state: AudioPlaybackState) -> bool {
    matches!(
        state,
        AudioPlaybackState::Loading | AudioPlaybackState::Playing
    )
}

/// How far back to go when a paused book is played again.
///
/// Nothing after a short pause: the listener was interrupted and knows
/// exactly where they were. Ten seconds after a longer one, which is a
/// sentence or two. Half a minute after an hour or more, which is a
/// paragraph, and what the leading players do.
const fn rewind_after(paused: Option<Duration>) -> u32 {
    match paused {
        None => 0,
        Some(paused) if paused.as_secs() < 60 => 0,
        Some(paused) if paused.as_secs() < 3_600 => 10_000,
        Some(_) => 30_000,
    }
}

/// What a device row says under the device's name.
const fn device_state(device: &BluetoothDevice) -> &'static str {
    if device.connected {
        "Connected · Ready to listen"
    } else if device.paired {
        "Paired · Tap to connect"
    } else {
        "Available · Tap to pair"
    }
}

/// A position as a clock: "1:15" and "1:01:15".
#[must_use]
pub fn clock(milliseconds: u32) -> String {
    let seconds = milliseconds / 1_000;
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if hours == 0 {
        format!("{minutes}:{seconds:02}")
    } else {
        format!("{hours}:{minutes:02}:{seconds:02}")
    }
}

/// A length of time in words: "7 h 38 min", "38 min", "45 s".
#[must_use]
pub fn span(milliseconds: u32) -> String {
    let seconds = milliseconds / 1_000;
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    if hours > 0 {
        format!("{hours} h {minutes:02} min")
    } else if minutes > 0 {
        format!("{minutes} min")
    } else {
        format!("{seconds} s")
    }
}

/// A speed as it is said: "1.0×", "1.25×", "0.75×".
#[must_use]
pub fn speed_label(percent: u16) -> String {
    let whole = percent / 100;
    let hundredths = percent % 100;
    if hundredths % 10 == 0 {
        format!("{whole}.{}×", hundredths / 10)
    } else {
        format!("{whole}.{hundredths:02}×")
    }
}

fn is_audio_request(request: &DeviceRequest) -> bool {
    matches!(
        request,
        DeviceRequest::ReadAudio
            | DeviceRequest::LoadAudio { .. }
            | DeviceRequest::PlayAudio
            | DeviceRequest::PauseAudio
            | DeviceRequest::SeekAudio { .. }
            | DeviceRequest::StopAudio
            | DeviceRequest::SetAudioVolume { .. }
            | DeviceRequest::ReadAudioDetail
            | DeviceRequest::SetAudioSpeed { .. }
    )
}

fn is_bluetooth_request(request: &DeviceRequest) -> bool {
    matches!(
        request,
        DeviceRequest::ReadBluetooth
            | DeviceRequest::SetBluetooth { .. }
            | DeviceRequest::ScanBluetooth
            | DeviceRequest::PairBluetooth { .. }
            | DeviceRequest::ConnectBluetooth { .. }
            | DeviceRequest::DisconnectBluetooth { .. }
            | DeviceRequest::ForgetBluetooth { .. }
    )
}

/// What a failed Bluetooth request means to the listener.
///
/// A radio that cannot be switched on because it is unreachable is a reader
/// whose own software has not started the radio since it was switched on:
/// the runtime will not do that first start itself. The listener can, from
/// the reader's own home screen, so that is what the complaint says.
fn bluetooth_failure(request: &DeviceRequest, error: DeviceError) -> String {
    if matches!(request, DeviceRequest::SetBluetooth { enabled: true })
        && error == DeviceError::Unreachable
    {
        return "Bluetooth's radio is not ready. Turn Bluetooth on once from the Kobo home \
                screen, then come back."
            .to_owned();
    }
    format!("Bluetooth failed: {}.", error.describe())
}

fn audio_error(error: DeviceError) -> &'static str {
    match error {
        DeviceError::Unreachable => {
            "Connect Bluetooth headphones or a speaker, then press Play again"
        }
        DeviceError::InvalidInput => "This reader cannot play this kind of audio file",
        DeviceError::NotFound => "The audio source is no longer on this device",
        DeviceError::TimedOut => "The Bluetooth audio service stopped responding",
        DeviceError::Backend => "This audio file could not be decoded",
        // Integrity is spelled out rather than folded into a wildcard so
        // that the next device error added is routed here deliberately. An
        // audio load never verifies a digest, so this is unreachable today.
        DeviceError::Authentication | DeviceError::Integrity => "Audio playback failed",
    }
}

#[cfg(test)]
mod tests;
