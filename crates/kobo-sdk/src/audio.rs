//! Reusable audiobook player UI and Bluetooth-output handoff.

use crate::{
    action_id, ActionId, AudioPlaybackState, AudioSource, BannerLevel, BluetoothDevice,
    BluetoothDeviceKind, Context, DenyReason, DeviceError, DeviceRequest, DeviceResult, Glyph,
    RowLead, Screen, ScreenBuilder, Task, TaskId, TaskOutcome, TilePicture,
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
const MORE_DEVICES: &str = "audio-more-devices";
const DEVICE_ACTIONS: [&str; 6] = [
    "audio-device-0",
    "audio-device-1",
    "audio-device-2",
    "audio-device-3",
    "audio-device-4",
    "audio-device-5",
];
const PAGE_SIZE: usize = 4;
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
    devices: Vec<BluetoothDevice>,
    page: usize,
    pending_bluetooth: Option<PendingBluetooth>,
    delayed_scan: Option<TaskId>,
    poll: Option<TaskId>,
    autoplay: PlayIntent,
    trouble: Option<String>,
    secondary_action: Option<(String, String)>,
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
    #[must_use]
    pub fn secondary_action(mut self, name: impl Into<String>, label: impl Into<String>) -> Self {
        self.secondary_action = Some((name.into(), label.into()));
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

    #[must_use]
    pub fn screen(&self) -> Screen {
        match self.view {
            View::Player => self.player_screen(),
            View::Bluetooth => self.bluetooth_screen(),
        }
    }

    fn player_screen(&self) -> Screen {
        let output = self
            .connected_output()
            .map_or("No Bluetooth audio device", |device| device.name.as_str());
        // The author is the hero's byline, immediately under the title, which is
        // where a reader looks for it. Repeating it as an "Author" fact two lines
        // lower says the same sentence twice and reads like a bug, so the facts
        // list carries only what the byline cannot.
        let mut facts = Vec::new();
        if let Some(chapter) = self.metadata.chapter.as_deref() {
            facts.push(("Chapter", chapter.to_owned()));
        }
        facts.push(("Output", output.to_owned()));
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
        let play_label = match self.playback {
            AudioPlaybackState::Playing => "Pause",
            AudioPlaybackState::Loading => "Loading…",
            _ => "Play",
        };
        let mut screen = ScreenBuilder::new("audio-player")
            .top_bar("Now playing")
            .owns_back(self.owns_back)
            .hero(
                self.cover,
                26,
                &self.metadata.title,
                self.metadata.author.clone(),
                facts,
            )
            .progress(progress)
            .section_with_value(
                "Position",
                format!("{} / {}", clock(self.position_ms), clock(self.duration_ms)),
            )
            .grid(
                3,
                false,
                [
                    (BACK_THIRTY, "−30 sec"),
                    (PLAY, play_label),
                    (FORWARD_THIRTY, "+30 sec"),
                ],
            )
            .grid(
                2,
                false,
                [
                    (VOLUME_DOWN, format!("Volume −  {}%", self.volume)),
                    (VOLUME_UP, format!("Volume +  {}%", self.volume)),
                ],
            );
        screen = match &self.secondary_action {
            Some((name, label)) => screen.action_bar([
                (OUTPUT.to_owned(), "Bluetooth output".to_owned()),
                (name.clone(), label.clone()),
            ]),
            None => screen.bottom_action(OUTPUT, "Bluetooth audio output"),
        };
        if self.audio_available == Availability::Unavailable {
            screen = screen.banner(
                BannerLevel::Attention,
                "Audio playback is unavailable on this firmware.",
            );
        } else if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble);
        }
        screen.build()
    }

    fn bluetooth_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("audio-output")
            .top_bar("Bluetooth audio")
            .owns_back(true)
            .section_with_value(
                "Bluetooth",
                if self.bluetooth_enabled == Switch::On {
                    "On"
                } else {
                    "Off"
                },
            )
            .button(
                BLUETOOTH_TOGGLE,
                if self.bluetooth_enabled == Switch::On {
                    "Turn Bluetooth off"
                } else {
                    "Turn Bluetooth on"
                },
            );
        if self.bluetooth_available == Availability::Unavailable {
            screen = screen.banner(
                BannerLevel::Attention,
                "Bluetooth is unavailable on this firmware.",
            );
        } else if let Some(trouble) = &self.trouble {
            screen = screen.banner(BannerLevel::Attention, trouble);
        }
        if self.bluetooth_enabled == Switch::On {
            let devices = self.audio_devices().collect::<Vec<_>>();
            if devices.is_empty() {
                screen = screen.text(
                    "Put headphones or a speaker in pairing mode, then scan for audio devices.",
                );
            } else {
                let pages = page_count(devices.len());
                screen = screen
                    .section_with_value("Audio devices", format!("{} / {pages}", self.page + 1))
                    .rows(
                        devices
                            .into_iter()
                            .skip(self.page * PAGE_SIZE)
                            .take(PAGE_SIZE)
                            .enumerate()
                            .map(|(index, device)| {
                                let state = if device.connected {
                                    "Connected · Ready to listen"
                                } else if device.paired {
                                    "Paired · Tap to connect"
                                } else {
                                    "Available · Tap to pair"
                                };
                                (
                                    DEVICE_ACTIONS[index],
                                    device.name.as_str(),
                                    state,
                                    RowLead::from(if device.connected {
                                        Glyph::Check
                                    } else {
                                        Glyph::Circle
                                    }),
                                )
                            }),
                    );
                if pages > 1 {
                    screen = screen.button(MORE_DEVICES, "More devices");
                }
            }
            screen = screen.button(BLUETOOTH_SCAN, "Scan for headphones and speakers");
        }
        screen.build()
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
        if action == action_id(MORE_DEVICES) {
            self.page = (self.page + 1) % page_count(self.audio_devices().count());
            return true;
        }
        let Some(index) = DEVICE_ACTIONS
            .iter()
            .position(|name| action == action_id(name))
        else {
            return false;
        };
        let Some(device) = self
            .audio_devices()
            .nth(self.page * PAGE_SIZE + index)
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
                self.playback = *state;
                self.position_ms = *position_ms;
                self.duration_ms = *duration_ms;
                self.volume = *volume;
                if matches!(request, DeviceRequest::LoadAudio { .. }) {
                    self.loaded = if *state == AudioPlaybackState::Idle {
                        LoadState::Unloaded
                    } else {
                        LoadState::Loaded
                    };
                }
                self.trouble = None;
                if self.autoplay == PlayIntent::Autoplay
                    && *state == AudioPlaybackState::Ready
                    && self.loaded == LoadState::Loaded
                {
                    context.device().play_audio();
                } else if matches!(
                    state,
                    AudioPlaybackState::Loading | AudioPlaybackState::Playing
                ) {
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
            } => {
                self.bluetooth_available = if *available {
                    Availability::Available
                } else {
                    Availability::Unavailable
                };
                self.bluetooth_enabled = if *enabled { Switch::On } else { Switch::Off };
                self.devices.clone_from(devices);
                self.page = self.page.min(page_count(self.audio_devices().count()) - 1);
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
            if self.playback == AudioPlaybackState::Playing {
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

fn page_count(items: usize) -> usize {
    items.max(1).div_ceil(PAGE_SIZE)
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
        DeviceError::Authentication | DeviceError::Backend => "Audio playback failed",
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
    fn clocks_cover_short_and_long_audiobooks() {
        assert_eq!(clock(75_000), "1:15");
        assert_eq!(clock(3_675_000), "1:01:15");
    }

    #[test]
    fn player_and_pairing_screens_fit_a_clara() {
        let mut player = AudioPlayer::shelf("book.mp3z", "A History of the Moon")
            .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"))
            .cover(TilePicture::new(PictureHandle(1), 240, 320))
            .secondary_action("another", "Create another");
        player.playback = AudioPlaybackState::Playing;
        player.position_ms = 75_000;
        player.duration_ms = 600_000;
        player.bluetooth_enabled = super::Switch::On;
        player.devices = vec![headphones(true)];
        assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());

        player.view = super::View::Bluetooth;
        assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());
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
            },
        );
        assert!(context.take_commands().iter().any(|command| matches!(
            command,
            crate::Command::Device(DeviceRequest::LoadAudio { .. })
        )));
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
