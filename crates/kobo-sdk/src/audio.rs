//! Reusable audiobook player UI and Bluetooth-output handoff.

use crate::{
    action_id, ActionId, AudioPlaybackState, AudioSource, BandAlign, BannerLevel, BluetoothDevice,
    BluetoothDeviceKind, Context, DenyReason, DeviceError, DeviceRequest, DeviceResult, Glyph,
    RowLead, Screen, ScreenBuilder, SlotWidth, Task, TaskId, TaskOutcome, TilePicture,
};
use std::time::Duration;

const PLAY: &str = "audio-play";
const BACK_THIRTY: &str = "audio-back-30";
const FORWARD_THIRTY: &str = "audio-forward-30";
const VOLUME_DOWN: &str = "audio-volume-down";
const VOLUME_UP: &str = "audio-volume-up";
const OUTPUT: &str = "audio-output";
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
const POLL_SECONDS: u32 = 5;
const SCAN_SECONDS: u32 = 3;

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Player,
    Bluetooth,
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
struct Shape {
    cover: bool,
    one_row: bool,
    side_by_side: bool,
}

impl Shape {
    const FULL: Self = Self {
        cover: true,
        one_row: false,
        side_by_side: false,
    };
    const IN_ORDER: [Self; 4] = [
        Self::FULL,
        Self {
            cover: false,
            ..Self::FULL
        },
        Self {
            cover: false,
            one_row: true,
            side_by_side: false,
        },
        Self {
            cover: false,
            one_row: true,
            side_by_side: true,
        },
    ];
}

/// A complete audiobook surface: album art, transport controls, position,
/// volume, and the Bluetooth audio picker used when Play has no output.
#[derive(Clone, Debug)]
pub struct AudioPlayer {
    source: AudioSource,
    metadata: AudioMetadata,
    cover: Option<TilePicture>,
    view: View,
    playback: AudioPlaybackState,
    position_ms: u32,
    duration_ms: u32,
    volume: u8,
    loaded: LoadState,
    audio_available: Availability,
    bluetooth_available: Availability,
    bluetooth_enabled: Switch,
    /// Whether the runtime has said that leaving will reboot the reader.
    restart_on_exit: bool,
    devices: Vec<BluetoothDevice>,
    page: usize,
    pending_bluetooth: Option<PendingBluetooth>,
    delayed_scan: Option<TaskId>,
    poll: Option<TaskId>,
    autoplay: PlayIntent,
    trouble: Option<String>,
    secondary_action: Option<(String, String, Glyph)>,
    owns_back: bool,
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
            loaded: LoadState::Unloaded,
            audio_available: Availability::Available,
            bluetooth_available: Availability::Available,
            bluetooth_enabled: Switch::Off,
            restart_on_exit: false,
            devices: Vec::new(),
            page: 0,
            pending_bluetooth: None,
            delayed_scan: None,
            poll: None,
            autoplay: PlayIntent::Manual,
            trouble: None,
            secondary_action: None,
            owns_back: false,
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
    pub const fn position(&self) -> Duration {
        Duration::from_millis(self.position_ms as u64)
    }

    #[must_use]
    pub const fn duration(&self) -> Duration {
        Duration::from_millis(self.duration_ms as u64)
    }

    /// The player as it is drawn when nothing has to give way: cover,
    /// transport and volume each in their own place. Prefer
    /// [`Self::screen_for`], which knows the panel and the text size.
    #[must_use]
    pub fn screen(&self) -> Screen {
        match self.view {
            View::Player => self.player_screen(Shape::FULL),
            View::Bluetooth => self.bluetooth_screen(&Context::default()),
        }
    }

    /// The player shaped for the panel and text size in `context`.
    ///
    /// At a large text size the full player is taller than a six inch panel,
    /// and the runtime refuses a screen that runs off it, which leaves the
    /// owner who needs big type with no player at all. So the parts that say
    /// the least give way first, one at a time and only as far as the panel
    /// needs: the cover, which repeats the title in a picture, and then the
    /// separate volume row, whose two buttons join the transport and whose
    /// level becomes a fact beside the output it applies to. Every word and
    /// every control stays on the screen.
    #[must_use]
    pub fn screen_for(&self, context: &Context) -> Screen {
        if self.view == View::Bluetooth {
            return self.bluetooth_screen(context);
        }
        let metrics = context.metrics();
        let chrome = crate::Chrome::measuring(true);
        let mut shaped = None;
        for shape in Shape::IN_ORDER {
            let screen = self.player_screen(shape);
            let fits = !screen
                .diagnostics(&metrics, &chrome)
                .issues
                .iter()
                .any(|issue| issue.severity == crate::DiagnosticSeverity::Error);
            shaped = Some(screen);
            if fits {
                break;
            }
        }
        shaped.unwrap_or_else(|| self.player_screen(Shape::FULL))
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
        if let Some(chapter) = self.metadata.chapter.as_deref() {
            facts.push(("Chapter", chapter.to_owned()));
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
        }
        let hero = |screen: ScreenBuilder| {
            screen.hero(
                self.cover.filter(|_| shape.cover),
                26,
                &self.metadata.title,
                self.metadata.author.clone(),
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
            format!("{} / {}", clock(self.position_ms), clock(self.duration_ms))
        };
        let controls_shown = self.audio_available != Availability::Unavailable;
        let screen = screen
            .progress(progress)
            .section_with_value("Position", position);
        if !controls_shown {
            return screen;
        }
        if shape.one_row {
            // Quieter and Louder at the two ends, where a thumb finds
            // them without looking, and the transport in the middle as
            // before.
            return screen.controls(
                5,
                [
                    (VOLUME_DOWN, "Quieter", Glyph::VolumeDown),
                    (BACK_THIRTY, "Back 30 sec", Glyph::Rewind30),
                    (PLAY, play_label, play_glyph),
                    (FORWARD_THIRTY, "Forward 30 sec", Glyph::Forward30),
                    (VOLUME_UP, "Louder", Glyph::VolumeUp),
                ],
            );
        }
        screen
            .controls(
                3,
                [
                    (BACK_THIRTY, "Back 30 sec", Glyph::Rewind30),
                    (PLAY, play_label, play_glyph),
                    (FORWARD_THIRTY, "Forward 30 sec", Glyph::Forward30),
                ],
            )
            // The volume is stated once, above the pair, rather than
            // printed on both buttons. Two buttons carrying the same
            // number is the same fault as a byline stated twice: it reads
            // as two facts.
            .section_with_value("Volume", format!("{}%", self.volume))
            .controls(
                2,
                [
                    (VOLUME_DOWN, "Quieter", Glyph::VolumeDown),
                    (VOLUME_UP, "Louder", Glyph::VolumeUp),
                ],
            )
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
        if self.view == View::Bluetooth && action == ActionId::BACK {
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
        if self.view == View::Bluetooth {
            return self.press_bluetooth(context, action);
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
            context.device().seek_audio(Duration::from_millis(u64::from(
                self.position_ms.saturating_sub(30_000),
            )));
            return true;
        }
        if action == action_id(FORWARD_THIRTY) {
            context.device().seek_audio(Duration::from_millis(u64::from(
                self.position_ms
                    .saturating_add(30_000)
                    .min(self.duration_ms),
            )));
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
            context.device().play_audio();
        } else {
            context.device().load_audio(self.source.clone());
        }
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
                self.playback = *state;
                self.position_ms = *position_ms;
                self.duration_ms = *duration_ms;
                self.trouble = None;
                if self.autoplay == PlayIntent::Autoplay && *state == AudioPlaybackState::Ready {
                    context.device().play_audio();
                } else if poll_worthy(*state) {
                    if *state == AudioPlaybackState::Playing {
                        self.autoplay = PlayIntent::Manual;
                    }
                    self.schedule_poll(context);
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
                self.trouble = Some(audio_error(*error).to_owned());
                self.autoplay = PlayIntent::Manual;
            }
            _ => {}
        }
    }

    fn bluetooth_result(
        &mut self,
        context: &mut Context,
        request: &DeviceRequest,
        result: &DeviceResult,
    ) {
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
                if matches!(request, DeviceRequest::ConnectBluetooth { .. })
                    && self.connected_output().is_some()
                    && self.autoplay == PlayIntent::Autoplay
                {
                    self.begin_audio(context);
                } else if matches!(request, DeviceRequest::SetBluetooth { enabled: true }) {
                    self.scan(context);
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
            }
            DeviceResult::Failed(error) => {
                self.trouble = Some(format!("Bluetooth failed: {}.", error.describe()));
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

    /// Handles delayed scan collection and position polling.
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
        false
    }

    fn schedule_poll(&mut self, context: &mut Context) {
        if self.poll.is_none() {
            self.poll = context.spawn(Task::Sleep {
                seconds: POLL_SECONDS,
            });
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

fn clock(milliseconds: u32) -> String {
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

fn audio_error(error: DeviceError) -> &'static str {
    match error {
        DeviceError::Unreachable => {
            "Connect Bluetooth headphones or a speaker, then press Play again"
        }
        DeviceError::InvalidInput => "This audio source is not a supported bounded MP3 or MP3Z",
        DeviceError::NotFound => "The audio source is no longer on this device",
        DeviceError::TimedOut => "The Bluetooth audio service stopped responding",
        // Integrity is spelled out rather than folded into a wildcard so
        // that the next device error added is routed here deliberately. An
        // audio load never verifies a digest, so this is unreachable today.
        DeviceError::Authentication | DeviceError::Backend | DeviceError::Integrity => {
            "Audio playback failed"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, AudioMetadata, AudioPlayer};
    use crate::{
        action_id, AudioPlaybackState, BluetoothDevice, BluetoothDeviceKind, DeviceRequest,
        DeviceResult, PictureHandle, TaskOutcome, TilePicture, CLARA_BW_METRICS,
    };

    fn headphones(connected: bool) -> BluetoothDevice {
        BluetoothDevice {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
            name: "Headphones".to_owned(),
            kind: BluetoothDeviceKind::Audio,
            paired: connected,
            connected,
        }
    }

    #[test]
    fn an_author_is_stated_once_not_twice() {
        // It is the hero's byline. A facts row repeating it word for word two
        // lines below reads as a rendering bug, and did on the reader.
        let player = AudioPlayer::shelf("book.mp3z", "A History of the Moon")
            .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"));
        let drawn = format!("{:?}", player.screen());
        assert_eq!(drawn.matches("Cobalt Audio").count(), 1, "{drawn}");
        assert!(!drawn.contains("Author"), "{drawn}");
    }

    #[test]
    fn a_player_reached_from_a_shelf_keeps_the_way_back() {
        let alone = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
        assert!(!alone.screen().owns_back);
        let from_a_shelf = AudioPlayer::shelf("book.mp3z", "A History of the Moon").owns_back(true);
        assert!(from_a_shelf.screen().owns_back);
    }

    #[test]
    fn a_volume_is_stated_once_not_on_both_buttons() {
        // Both volume buttons used to print the current level, so a reader saw
        // "Volume -  40%" beside "Volume +  40%" and had to work out that the
        // two numbers were one number. Same fault as the doubled byline above.
        let mut player = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
        player.volume = 40;
        let drawn = format!("{:?}", player.screen());
        assert_eq!(drawn.matches("40%").count(), 1, "{drawn}");
    }

    #[test]
    fn the_play_control_shows_the_picture_of_what_it_will_do() {
        // Stopped, the button plays, so it draws a triangle. Playing, it
        // pauses, so it draws two bars. A control whose icon describes the
        // current state rather than the pending action is the classic way to
        // get this backwards.
        let stopped = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
        let drawn = format!("{:?}", stopped.screen());
        assert!(drawn.contains("Play"), "{drawn}");
        assert!(!drawn.contains("Pause"), "{drawn}");

        let mut playing = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
        playing.playback = AudioPlaybackState::Playing;
        let drawn = format!("{:?}", playing.screen());
        assert!(drawn.contains("Pause"), "{drawn}");
    }

    #[test]
    fn clocks_cover_short_and_long_audiobooks() {
        assert_eq!(clock(75_000), "1:15");
        assert_eq!(clock(3_675_000), "1:01:15");
    }

    #[test]
    fn player_and_pairing_screens_fit_a_clara() {
        let mut player = AudioPlayer::shelf("book.mp3z", "A History of the Moon")
            .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"))
            .cover(TilePicture::new(PictureHandle(1), 240, 320))
            .secondary_action("another", "Create another", crate::Glyph::Plus);
        player.playback = AudioPlaybackState::Playing;
        player.position_ms = 75_000;
        player.duration_ms = 600_000;
        player.bluetooth_enabled = super::Switch::On;
        player.devices = vec![headphones(true)];
        assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());

        player.view = super::View::Bluetooth;
        assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());
    }

    /// An application that shows the player, so the player is measured
    /// through a runner's context, at the size and against the type the
    /// runtime will draw it with.
    struct Shows;

    impl crate::KoboApp for Shows {
        fn on_start(&mut self, _context: &mut crate::Context) {}
        fn on_action(&mut self, _context: &mut crate::Context, _action: crate::ActionId) {}
    }

    /// Every state the player is drawn in, at its longest: a title of two
    /// lines, a byline, a chapter, a connected output, a trouble banner, and
    /// the secondary action beside the output picker.
    fn every_player() -> Vec<(&'static str, AudioPlayer)> {
        let full = || {
            let mut player = AudioPlayer::shelf("book.mp3z", "The Quiet Shelf")
                .metadata(
                    AudioMetadata::new("A History of the Moon")
                        .author("Made on this reader")
                        .chapter("Part 3 of 12"),
                )
                .cover(TilePicture::new(PictureHandle(1), 240, 320))
                .secondary_action("another", "Create another", crate::Glyph::Plus);
            player.position_ms = 3_675_000;
            player.duration_ms = 7_200_000;
            player.bluetooth_enabled = super::Switch::On;
            player.devices = vec![headphones(true)];
            player
        };
        let playing = {
            let mut player = full();
            player.playback = AudioPlaybackState::Playing;
            player
        };
        let unavailable = {
            let mut player = full();
            player.audio_available = super::Availability::Unavailable;
            player.devices.clear();
            player
        };
        let trouble = {
            let mut player = full();
            player.trouble = Some("The Bluetooth audio service stopped responding.".to_owned());
            player
        };
        let alone = AudioPlayer::shelf("book.mp3z", "The Quiet Shelf").cover(TilePicture::new(
            PictureHandle(1),
            240,
            320,
        ));
        vec![
            ("playing", playing),
            ("unavailable", unavailable),
            ("trouble", trouble),
            ("without a secondary action", alone),
        ]
    }

    #[test]
    fn the_player_keeps_every_control_on_a_clara_at_every_text_size_held_either_way() {
        use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, Orientation, TextScale};
        for text_scale in TextScale::STEPS {
            for orientation in [Orientation::Portrait, Orientation::Landscape] {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..CLARA_BW_METRICS
                }
                .oriented(orientation);
                let context = crate::AppRunner::with_metrics(Shows, metrics).context();
                for (state, player) in every_player() {
                    let at = format!("{state}, {text_scale:?}, {orientation:?}");
                    let screen = player.screen_for(&context);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    let errors = diagnostics
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(errors.is_empty(), "{at}: {errors:?}");
                    let mut wanted = vec![super::OUTPUT];
                    if player.audio_available == super::Availability::Available {
                        wanted.extend([
                            super::PLAY,
                            super::BACK_THIRTY,
                            super::FORWARD_THIRTY,
                            super::VOLUME_DOWN,
                            super::VOLUME_UP,
                        ]);
                    }
                    for name in wanted {
                        assert!(
                            diagnostics.layout.rect_of_action(action_id(name)).is_some(),
                            "{at}: {name} is not on the screen"
                        );
                    }
                    let drawn = format!("{screen:?}");
                    if player.audio_available == super::Availability::Available {
                        assert!(drawn.contains(&format!("{}%", player.volume)), "{at}");
                    }
                    if text_scale == TextScale::Default && orientation == Orientation::Portrait {
                        // Where the panel has the room, nothing gives way.
                        assert_eq!(screen, player.screen(), "{at}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_output_screen_pages_its_devices_to_fit_a_clara_at_every_text_size_held_either_way() {
        use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, Orientation, TextScale};
        let device = |index: usize| BluetoothDevice {
            address: format!("AA:BB:CC:DD:EE:{index:02}"),
            name: format!("Travel headphones {index}"),
            kind: BluetoothDeviceKind::Audio,
            paired: index < 2,
            connected: index == 0,
        };
        for text_scale in TextScale::STEPS {
            for orientation in [Orientation::Portrait, Orientation::Landscape] {
                let metrics = DisplayMetrics {
                    text_scale,
                    ..CLARA_BW_METRICS
                }
                .oriented(orientation);
                let runner = crate::AppRunner::with_metrics(Shows, metrics);
                for (state, restart, trouble, count) in [
                    ("restart owed", true, false, 9),
                    ("trouble", false, true, 9),
                    ("one device", true, false, 1),
                    ("nothing found", true, false, 0),
                ] {
                    let at = format!("{state}, {text_scale:?}, {orientation:?}");
                    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
                    player.view = super::View::Bluetooth;
                    player.bluetooth_enabled = super::Switch::On;
                    player.restart_on_exit = restart;
                    if trouble {
                        player.trouble =
                            Some("The Bluetooth audio service stopped responding.".to_owned());
                    }
                    player.devices = (0..count).map(device).collect();
                    let mut seen = Vec::new();
                    loop {
                        let context = runner.context();
                        let screen = player.screen_for(&context);
                        let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                        let errors = diagnostics
                            .issues
                            .iter()
                            .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                            .collect::<Vec<_>>();
                        assert!(errors.is_empty(), "{at}: {errors:?}");
                        for name in [super::BLUETOOTH_TOGGLE, super::BLUETOOTH_SCAN] {
                            assert!(
                                diagnostics.layout.rect_of_action(action_id(name)).is_some(),
                                "{at}: {name}"
                            );
                        }
                        let drawn = format!("{screen:?}");
                        seen.extend((0..count).filter(|&index| {
                            drawn.contains(&format!("\"Travel headphones {index}\""))
                        }));
                        if diagnostics
                            .layout
                            .rect_of_action(action_id(super::DEVICES_NEXT))
                            .is_none()
                        {
                            break;
                        }
                        assert!(player.press(&mut runner.context(), action_id(super::DEVICES_NEXT)));
                    }
                    seen.sort_unstable();
                    seen.dedup();
                    assert_eq!(seen, (0..count).collect::<Vec<_>>(), "{at}");
                }
            }
        }
    }

    #[test]
    fn play_without_an_output_opens_bluetooth_and_turns_it_on() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        let mut context = crate::Context::default();
        assert!(player.press(&mut context, action_id(super::PLAY)));
        assert_eq!(player.view, super::View::Bluetooth);
        assert!(context.take_commands().iter().any(|command| matches!(
            command,
            crate::Command::Device(DeviceRequest::SetBluetooth { enabled: true })
        )));
    }

    #[test]
    fn connecting_after_play_loads_the_source_automatically() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        player.autoplay = super::PlayIntent::Autoplay;
        let mut context = crate::Context::default();
        player.on_device_result(
            &mut context,
            &DeviceRequest::ConnectBluetooth {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
            },
            &DeviceResult::Bluetooth {
                available: true,
                enabled: true,
                devices: vec![headphones(true)],
                restart_on_exit: false,
            },
        );
        assert!(context.take_commands().iter().any(|command| matches!(
            command,
            crate::Command::Device(DeviceRequest::LoadAudio { .. })
        )));
    }

    /// The reboot this warns about used to happen with no warning at all, and
    /// was reported as the operating system crashing. The warning is owed only
    /// when the runtime says the radio has actually been touched, so the
    /// quieter case must stay quiet or the warning stops meaning anything.
    #[test]
    fn the_coming_restart_is_announced_only_when_it_is_really_coming() {
        fn reading(restart_on_exit: bool) -> String {
            let mut player = AudioPlayer::shelf("book.mp3z", "Book");
            player.view = super::View::Bluetooth;
            player.on_device_result(
                &mut crate::Context::default(),
                &DeviceRequest::ReadBluetooth,
                &DeviceResult::Bluetooth {
                    available: true,
                    enabled: true,
                    devices: vec![headphones(false)],
                    restart_on_exit,
                },
            );
            format!("{:?}", player.screen())
        }

        assert!(reading(true).contains("restart itself"));
        assert!(!reading(false).contains("restart itself"));
    }

    /// The debt is owed for the rest of the session once the radio is touched.
    /// A later reading that answers false must not withdraw a promise the
    /// reader has already been shown.
    #[test]
    fn the_restart_warning_is_never_withdrawn() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        player.view = super::View::Bluetooth;
        let mut context = crate::Context::default();
        for restart_on_exit in [true, false] {
            player.on_device_result(
                &mut context,
                &DeviceRequest::ReadBluetooth,
                &DeviceResult::Bluetooth {
                    available: true,
                    enabled: true,
                    devices: vec![headphones(false)],
                    restart_on_exit,
                },
            );
        }
        assert!(format!("{:?}", player.screen()).contains("restart itself"));
    }

    /// Reported from the reader as "clicking play a second time started it".
    /// The player asked to be woken while the file was decoding and then threw
    /// its own wake-up away, so nothing ever asked the backend again and the
    /// screen sat on "Loading…" until a second press took the loaded path.
    #[test]
    fn a_book_that_is_still_decoding_is_asked_about_again() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        let mut context = crate::Context::default();

        // The load is answered while decoding is still running.
        player.on_device_result(
            &mut context,
            &DeviceRequest::LoadAudio {
                source: crate::AudioSource::Shelf("book.mp3z".to_owned()),
            },
            &DeviceResult::Audio {
                available: true,
                state: AudioPlaybackState::Loading,
                position_ms: 0,
                duration_ms: 0,
                volume: 70,
            },
        );
        let poll = player.poll.expect("decoding should schedule a poll");
        drop(context.take_commands());

        // When that poll comes due the player must ask again, not go quiet.
        assert!(player.on_task(&mut context, poll, &TaskOutcome::Completed(Vec::new())));
        assert!(
            context
                .take_commands()
                .iter()
                .any(|command| matches!(command, crate::Command::Device(DeviceRequest::ReadAudio))),
            "a decoding player must keep asking or it never learns it is ready"
        );
    }

    /// The same press must also finish the job: once decoding lands, the play
    /// the reader already asked for has to happen without a second press.
    #[test]
    fn the_press_that_started_a_load_still_plays_when_decoding_lands() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        player.autoplay = super::PlayIntent::Autoplay;
        player.loaded = super::LoadState::Loaded;
        let mut context = crate::Context::default();
        player.on_device_result(
            &mut context,
            &DeviceRequest::ReadAudio,
            &DeviceResult::Audio {
                available: true,
                state: AudioPlaybackState::Ready,
                position_ms: 0,
                duration_ms: 30_000,
                volume: 70,
            },
        );
        assert!(context
            .take_commands()
            .iter()
            .any(|command| matches!(command, crate::Command::Device(DeviceRequest::PlayAudio))));
    }

    /// Opening a thirty second book straight after a ten minute one showed ten
    /// minutes, because the backend still held the previous book's numbers and
    /// the fresh player adopted them on its opening read.
    #[test]
    fn a_new_player_does_not_wear_the_last_books_duration() {
        let mut player = AudioPlayer::shelf("short.mp3z", "Book");
        let mut context = crate::Context::default();
        player.on_device_result(
            &mut context,
            &DeviceRequest::ReadAudio,
            &DeviceResult::Audio {
                available: true,
                state: AudioPlaybackState::Ready,
                position_ms: 44_000,
                duration_ms: 658_000,
                volume: 55,
            },
        );

        assert_eq!(
            player.duration_ms, 0,
            "that duration belongs to another book"
        );
        assert_eq!(player.position_ms, 0);
        assert_eq!(player.playback, AudioPlaybackState::Idle);
        // Volume and availability describe the device, not the book.
        assert_eq!(player.volume, 55);
        let drawn = format!("{:?}", player.screen());
        assert!(!drawn.contains("10:58"), "{drawn}");
    }

    #[test]
    fn only_the_players_own_sleep_is_consumed() {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        let mut context = crate::Context::default();
        assert!(!player.on_task(
            &mut context,
            crate::TaskId(99),
            &TaskOutcome::Completed(Vec::new())
        ));
    }
}
