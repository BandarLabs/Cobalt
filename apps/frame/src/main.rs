//! A private-shelf photo frame. Images are prepared and atomically published by `kobo frame`.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Heartbeat, KoboApp, PictureHandle, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult, TaskId, TaskOutcome, TilePicture,
};
use std::collections::BTreeSet;
use std::process::ExitCode;
use std::time::Duration;

const MANIFEST: &str = "manifest.v1";
const STATE: &str = "frame-state-v1";
const PHOTO: PictureHandle = PictureHandle(1);
const MAX_PHOTOS: usize = 500;
const MAX_MANIFEST: usize = 256 * 1024;
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

const SHOW: &str = "show";
const NEXT: &str = "next";
const PREVIOUS: &str = "previous";
const MENU: &str = "menu";
const EXIT: &str = "exit";
const MODE: &str = "mode";
const INTERVAL: &str = "interval";
const ORDER: &str = "order";
const LOAD_WATCH_SECS: u32 = 20;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    Opening,
    #[default]
    Home,
    Show,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Photo {
    id: String,
    digest: String,
    taken: u64,
    album: String,
    name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Settings {
    slow: bool,
    interval: u8,
    shuffled: bool,
    position: usize,
}

impl Settings {
    fn interval(&self) -> u8 {
        if self.slow {
            match self.interval {
                1 | 6 | 24 => self.interval,
                _ => 6,
            }
        } else {
            match self.interval {
                5 | 15 | 60 => self.interval,
                _ => 15,
            }
        }
    }

    fn encode(&self) -> String {
        format!(
            "1\n{}\n{}\n{}\n{}",
            u8::from(self.slow),
            self.interval(),
            u8::from(self.shuffled),
            self.position
        )
    }
}

#[derive(Default)]
struct Startup {
    state_loaded: bool,
    manifest_loaded: bool,
    started: bool,
}

#[derive(Default)]
struct Frame {
    view: View,
    settings: Settings,
    photos: Vec<Photo>,
    picture: Option<TilePicture>,
    manifest_load: Option<ShelfDownload>,
    photo_load: Option<ShelfDownload>,
    photo_load_id: Option<String>,
    panel_width: u32,
    panel_height: u32,
    startup: Startup,
    unreadable: BTreeSet<String>,
    overlay: bool,
    settings_open: bool,
    clock: Option<Heartbeat>,
    load_watch: Option<Heartbeat>,
    notice: Option<String>,
}

impl Frame {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Opening => ScreenBuilder::new("frame-opening")
                .top_bar("Frame")
                .activity("Opening your photo shelf", None)
                .build(),
            View::Home => self.home(),
            View::Show => self.photo_screen(),
        }
    }

    fn home(&self) -> Screen {
        let mut screen = ScreenBuilder::new("frame-home").top_bar("Frame");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if !self.startup.manifest_loaded {
            return screen.activity("Opening your photo shelf", None).build();
        }
        if self.photos.is_empty() {
            return screen
                .splash(
                    Some(Glyph::App),
                    "Your frame is empty",
                    "On your computer, run `kobo frame init --device IP`, then `kobo frame push PHOTO_OR_FOLDER --device IP`.",
                )
                .build();
        }
        let index = self.settings.position % self.photos.len().max(1) + 1;
        screen =
            screen.section_with_value("Photographs", format!("{index} of {}", self.photos.len()));
        if let Some(picture) = self.picture {
            screen = screen
                .picture(picture, 76)
                .page_turns(PREVIOUS, NEXT)
                .reading_menu(MENU)
                .buttons([(SHOW, "Open"), (NEXT, "Next")]);
        } else if self.loading_selected() {
            screen = screen.activity("Opening this photograph", None);
        } else if self.photos_all_unreadable() {
            screen = screen.splash(
                Some(Glyph::App),
                "No photograph can be opened",
                "Re-push the album from your computer.",
            );
        } else {
            screen = screen
                .secondary("This photograph could not be opened.")
                .buttons([(SHOW, "Try again"), (NEXT, "Next photo")]);
        }
        if !self.unreadable.is_empty() {
            screen = screen.banner(
                BannerLevel::Attention,
                format!(
                    "{} photo(s) unreadable — re-push them from your computer.",
                    self.unreadable.len()
                ),
            );
        }
        if self.settings_open {
            let (mode, interval, order) = self.setting_labels();
            screen = screen.modal("Settings", |overlay| {
                overlay.buttons([(MODE, mode), (INTERVAL, interval), (ORDER, order)])
            });
        }
        screen.build()
    }

    fn setting_labels(&self) -> (String, String, String) {
        let mode = if self.settings.slow {
            "Slow slideshow"
        } else {
            "Frame mode"
        };
        let interval = if self.settings.slow {
            format!("Every {} hours", self.settings.interval())
        } else {
            format!("Every {} minutes", self.settings.interval())
        };
        let order = if self.settings.shuffled {
            "Shuffle"
        } else {
            "By date"
        };
        (mode.to_owned(), interval, order.to_owned())
    }

    fn photo_screen(&self) -> Screen {
        let Some(picture) = self.picture else {
            let screen = ScreenBuilder::new("frame-show").top_bar("Frame");
            if self.loading_selected() {
                return screen
                    .activity("Loading photo", None)
                    .build()
                    .with_own_back(true);
            }
            return screen
                .banner(
                    BannerLevel::Attention,
                    self.notice
                        .clone()
                        .unwrap_or_else(|| "This photograph could not be opened.".to_owned()),
                )
                .buttons([(SHOW, "Try again"), (NEXT, "Next photo")])
                .build()
                .with_own_back(true);
        };
        let mut screen = ScreenBuilder::new("frame-show")
            .unframed_picture(picture, 500)
            .page_turns(PREVIOUS, NEXT)
            .reading_menu(MENU);
        if self.overlay {
            let selected = self.selected().expect("visible photo has a selection");
            screen = screen.modal("Photo", |overlay| {
                overlay
                    .facts([
                        ("File", selected.name.clone()),
                        ("Album", selected.album.clone()),
                        ("Taken", selected.taken.to_string()),
                    ])
                    .buttons([(PREVIOUS, "Previous"), (NEXT, "Next"), (EXIT, "Exit")])
            });
        }
        screen.build().with_own_back(true)
    }

    fn selected(&self) -> Option<&Photo> {
        let choices = self.ordered_indices();
        choices
            .get(self.settings.position % choices.len().max(1))
            .and_then(|index| self.photos.get(*index))
    }

    fn ordered_indices(&self) -> Vec<usize> {
        let mut indices = (0..self.photos.len()).collect::<Vec<_>>();
        if self.settings.shuffled {
            indices.sort_by_key(|index| stable_hash(&self.photos[*index].id));
        }
        indices
    }

    fn loading_selected(&self) -> bool {
        let Some(id) = self.photo_load_id.as_deref() else {
            return false;
        };
        self.photo_load.is_some() && self.selected().is_some_and(|photo| photo.id == id)
    }

    fn photos_all_unreadable(&self) -> bool {
        !self.photos.is_empty() && self.unreadable.len() >= self.photos.len()
    }

    fn abandon_photo_load(&mut self, context: &mut Context) {
        self.photo_load = None;
        self.photo_load_id = None;
        self.stop_load_watch(context);
    }

    fn stop_load_watch(&mut self, context: &mut Context) {
        if let Some(watch) = &mut self.load_watch {
            watch.stop(context);
        }
        self.load_watch = None;
    }

    fn arm_load_watch(&mut self, context: &mut Context) {
        self.stop_load_watch(context);
        let mut watch = Heartbeat::every(LOAD_WATCH_SECS);
        watch.start(context);
        self.load_watch = Some(watch);
    }

    fn start_current(&mut self, context: &mut Context) {
        if self.photos.is_empty() {
            return;
        }
        let Some(id) = self.selected().map(|photo| photo.id.clone()) else {
            return;
        };
        if self.unreadable.contains(&id) {
            return;
        }
        if self.loading_selected() {
            return;
        }
        self.abandon_photo_load(context);
        let mut load = ShelfDownload::new(format!("{id}.png")).at_most(MAX_FRAME_BYTES);
        load.start(context);
        self.photo_load = Some(load);
        self.photo_load_id = Some(id);
        self.arm_load_watch(context);
    }

    fn advance(&mut self, context: &mut Context, forward: bool) {
        if self.photos.is_empty() {
            return;
        }
        let count = self.photos.len();
        self.settings.position = if forward {
            self.settings.position.saturating_add(1) % count
        } else {
            self.settings.position.checked_sub(1).unwrap_or(count - 1)
        };
        self.overlay = false;
        self.settings_open = false;
        self.picture = None;
        self.save(context);
        self.start_current(context);
    }

    fn skip_unreadable(&mut self, context: &mut Context, id: &str, error: String) {
        self.abandon_photo_load(context);
        self.unreadable.insert(id.to_owned());
        self.picture = None;
        self.notice = Some(error);
        if self.photos_all_unreadable() {
            self.notice = Some("No photo on this shelf can be read. Re-push the album.".to_owned());
            self.view = View::Home;
            return;
        }
        self.advance(context, true);
    }

    fn apply_power_policy(&mut self, context: &mut Context) {
        if self.settings.slow {
            if let Some(clock) = &mut self.clock {
                clock.stop(context);
            }
            self.clock = None;
            context.device().allow_sleep();
            context.device().schedule_wake(Duration::from_secs(
                u64::from(self.settings.interval()) * 3600,
            ));
        } else {
            context.device().cancel_wake();
            context.device().keep_awake(Duration::from_secs(
                u64::from(self.settings.interval()) * 60 + 60,
            ));
            if let Some(clock) = &mut self.clock {
                clock.stop(context);
            }
            let mut clock = Heartbeat::every(u32::from(self.settings.interval()) * 60);
            clock.start(context);
            self.clock = Some(clock);
        }
    }

    fn save(&self, context: &mut Context) {
        context.store().save(STATE, self.settings.encode());
    }

    fn start_when_ready(&mut self, context: &mut Context) {
        if self.startup.state_loaded && self.startup.manifest_loaded && !self.startup.started {
            self.startup.started = true;
            self.settings.position %= self.photos.len().max(1);
            self.apply_power_policy(context);
            self.start_current(context);
            if self.view == View::Opening {
                self.view = View::Home;
            }
        }
    }

    fn accept_picture(&mut self, context: &mut Context, id: &str, bytes: &[u8]) {
        self.stop_load_watch(context);
        let picture = kobo_image::decode(bytes).and_then(|picture| {
            if picture.width() == self.panel_width && picture.height() == self.panel_height {
                Ok(picture)
            } else {
                picture.cover(self.panel_width, self.panel_height)
            }
        });
        match picture {
            Ok(picture) => {
                self.picture = context.put_picture(
                    PHOTO,
                    picture.width(),
                    picture.height(),
                    picture.into_grey(),
                );
                if self.picture.is_none() {
                    self.skip_unreadable(
                        context,
                        id,
                        "The photo exceeds Frame's picture budget.".to_owned(),
                    );
                } else {
                    self.unreadable.remove(id);
                    self.notice = None;
                }
            }
            Err(_) => self.skip_unreadable(
                context,
                id,
                "A photo could not be read and was skipped.".to_owned(),
            ),
        }
    }

    fn advance_manifest(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.manifest_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.manifest_load.take().expect("active manifest").take();
                match decode_manifest(&bytes) {
                    Ok(photos) => {
                        self.photos = photos;
                        self.notice = None;
                    }
                    Err(error) => {
                        self.photos.clear();
                        self.notice = Some(format!("Frame shelf needs re-pushing: {error}"));
                    }
                }
                self.startup.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.manifest_load = None;
                self.startup.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(_) => {
                self.manifest_load = None;
                self.startup.manifest_loaded = true;
                self.notice = Some("Frame shelf could not be opened.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn advance_photo(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.photo_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.photo_load.take().expect("active photo").take();
                let id = self.photo_load_id.take().expect("active photo identity");
                self.stop_load_watch(context);
                if self.selected().is_some_and(|photo| photo.id == id) {
                    self.accept_picture(context, &id, &bytes);
                } else {
                    self.start_current(context);
                }
                true
            }
            ShelfProgress::Failed(_) => {
                let id = self.photo_load_id.take().expect("active photo identity");
                self.photo_load = None;
                self.stop_load_watch(context);
                if self.selected().is_some_and(|photo| photo.id == id) {
                    self.skip_unreadable(
                        context,
                        &id,
                        "A photo is missing or damaged and was skipped.".to_owned(),
                    );
                } else {
                    self.start_current(context);
                }
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
}

impl KoboApp for Frame {
    fn on_start(&mut self, context: &mut Context) {
        self.panel_width = u32::try_from(context.metrics().width).unwrap_or_default();
        self.panel_height = u32::try_from(context.metrics().height).unwrap_or_default();
        self.view = View::Opening;
        context.store().load(STATE);
        let mut manifest = ShelfDownload::new(MANIFEST).at_most(MAX_MANIFEST);
        manifest.start(context);
        self.manifest_load = Some(manifest);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.advance_manifest(context, &result) || self.advance_photo(context, &result) {
            self.start_when_ready(context);
            self.show(context);
            return;
        }
        if let StoreResult::Loaded { key, value } = result {
            if key == STATE {
                self.settings = value
                    .as_deref()
                    .and_then(decode_settings)
                    .unwrap_or_default();
                self.startup.state_loaded = true;
            }
        }
        self.start_when_ready(context);
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK || action == action_id(EXIT) {
            if self.settings_open {
                self.settings_open = false;
            } else if self.view == View::Show {
                self.view = View::Home;
                self.overlay = false;
            } else {
                context.exit();
            }
        } else if action == action_id(SHOW) {
            if let Some(id) = self.selected().map(|photo| photo.id.clone()) {
                self.unreadable.remove(&id);
            }
            if self.picture.is_some() {
                self.view = View::Show;
            }
            self.settings_open = false;
            self.start_current(context);
        } else if action == action_id(NEXT) {
            self.advance(context, true);
        } else if action == action_id(PREVIOUS) {
            self.advance(context, false);
        } else if action == action_id(MENU) {
            if self.view == View::Home {
                self.settings_open = !self.settings_open;
            } else {
                self.overlay = true;
            }
        } else if action == action_id(MODE) {
            self.settings.slow = !self.settings.slow;
            self.settings.interval = if self.settings.slow { 6 } else { 15 };
            self.save(context);
            self.apply_power_policy(context);
        } else if action == action_id(INTERVAL) {
            self.settings.interval = if self.settings.slow {
                match self.settings.interval() {
                    1 => 6,
                    6 => 24,
                    _ => 1,
                }
            } else {
                match self.settings.interval() {
                    5 => 15,
                    15 => 60,
                    _ => 5,
                }
            };
            self.save(context);
            self.apply_power_policy(context);
        } else if action == action_id(ORDER) {
            self.settings.shuffled = !self.settings.shuffled;
            self.settings.position = 0;
            self.picture = None;
            self.save(context);
            self.abandon_photo_load(context);
            self.start_current(context);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self
            .load_watch
            .as_mut()
            .is_some_and(|watch| watch.on_task(context, task, &outcome))
        {
            if let Some(id) = self.photo_load_id.clone() {
                self.skip_unreadable(
                    context,
                    &id,
                    "This photograph is taking too long and was skipped.".to_owned(),
                );
            } else {
                self.stop_load_watch(context);
            }
            self.show(context);
            return;
        }
        if self
            .clock
            .as_mut()
            .is_some_and(|clock| clock.on_task(context, task, &outcome))
        {
            if !self.settings.slow {
                self.advance(context, true);
                self.apply_power_policy(context);
            }
            self.show(context);
        }
    }

    fn on_scheduled_wake(&mut self, context: &mut Context) {
        if self.settings.slow {
            self.advance(context, true);
            self.apply_power_policy(context);
            self.show(context);
        }
    }
}

fn decode_manifest(bytes: &[u8]) -> Result<Vec<Photo>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "manifest is not UTF-8")?;
    let mut lines = text.lines();
    if lines.next() != Some("cobalt-frame-v1") {
        return Err("manifest version is unsupported".to_owned());
    }
    let mut photos = Vec::new();
    let mut ids = BTreeSet::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [id, digest, taken, album, name] = fields.as_slice() else {
            return Err("manifest has a malformed entry".to_owned());
        };
        if !valid_id(id)
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !ids.insert((*id).to_owned())
        {
            return Err("manifest has an invalid photo identity".to_owned());
        }
        let taken = taken.parse().map_err(|_| "manifest has an invalid date")?;
        photos.push(Photo {
            id: (*id).to_owned(),
            digest: (*digest).to_owned(),
            taken,
            album: (*album).to_owned(),
            name: (*name).to_owned(),
        });
    }
    if photos.len() > MAX_PHOTOS {
        return Err(format!("manifest exceeds the {MAX_PHOTOS}-photo limit"));
    }
    Ok(photos)
}

fn decode_settings(bytes: &[u8]) -> Option<Settings> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != "1" {
        return None;
    }
    let slow = lines.next()? == "1";
    let interval = lines.next()?.parse().ok()?;
    let shuffled = lines.next()? == "1";
    let position = lines.next()?.parse().ok()?;
    if lines.next().is_some() {
        return None;
    }
    Some(Settings {
        slow,
        interval,
        shuffled,
        position,
    })
}

fn valid_id(id: &str) -> bool {
    id.starts_with("photo-")
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn main() -> ExitCode {
    kobo_sdk::run("frame", Frame::default()).map_or_else(
        |error| {
            eprintln!("frame: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, DeviceRequest};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    fn photo(id: &str, taken: u64) -> Photo {
        Photo {
            id: id.to_owned(),
            digest: "a".repeat(64),
            taken,
            album: "Family".into(),
            name: format!("{id}.png"),
        }
    }

    #[test]
    fn manifest_and_state_are_bounded_and_round_trip() {
        let manifest = format!(
            "cobalt-frame-v1\nphoto-0123456789abcdef\t{}\t12\tFamily\tone.png\n",
            "a".repeat(64)
        );
        assert_eq!(
            decode_manifest(manifest.as_bytes()).expect("manifest")[0].taken,
            12
        );
        assert!(decode_manifest(b"cobalt-frame-v1\n../bad\ta\t0\tx\ty\n").is_err());
        let settings = Settings {
            slow: true,
            interval: 24,
            shuffled: true,
            position: 9,
        };
        assert_eq!(
            decode_settings(settings.encode().as_bytes()),
            Some(settings)
        );
    }

    #[test]
    fn navigation_is_durable_and_shuffle_is_stable() {
        let mut frame = Frame {
            photos: vec![
                photo("photo-aaaaaaaaaaaaaaaa", 1),
                photo("photo-bbbbbbbbbbbbbbbb", 2),
            ],
            ..Frame::default()
        };
        let before = frame.ordered_indices();
        frame.settings.shuffled = true;
        let shuffled = frame.ordered_indices();
        assert_eq!(shuffled, frame.ordered_indices());
        assert_ne!(before.len(), 0);
        let mut context = Context::default();
        frame.advance(&mut context, true);
        assert_eq!(frame.settings.position, 1);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Store(_))));
    }

    #[test]
    fn requested_capabilities_use_real_wake_apis() {
        let mut frame = Frame {
            settings: Settings {
                slow: true,
                interval: 6,
                ..Settings::default()
            },
            ..Frame::default()
        };
        let mut context = Context::default();
        frame.apply_power_policy(&mut context);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ScheduleWake { .. }))));
        frame.settings.slow = false;
        frame.apply_power_policy(&mut context);
        assert!(context
            .commands()
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::KeepAwake { .. }))));
    }

    #[test]
    fn advancing_abandons_the_in_flight_load_and_starts_the_new_photo() {
        let mut frame = Frame {
            photos: vec![
                photo("photo-aaaaaaaaaaaaaaaa", 1),
                photo("photo-bbbbbbbbbbbbbbbb", 2),
            ],
            panel_width: CLARA_BW_METRICS.width as u32,
            panel_height: CLARA_BW_METRICS.height as u32,
            ..Frame::default()
        };
        let mut context = Context::default();
        frame.start_current(&mut context);
        frame.advance(&mut context, true);
        assert_eq!(
            frame.photo_load_id.as_deref(),
            Some("photo-bbbbbbbbbbbbbbbb")
        );
        assert!(!frame.advance_photo(
            &mut context,
            &StoreResult::ShelfRead {
                name: "photo-aaaaaaaaaaaaaaaa.png".into(),
                offset: 0,
                bytes: b"old photo".to_vec(),
                size: 9,
            }
        ));
        assert!(frame.picture.is_none());
        assert!(frame.unreadable.is_empty());
        assert_eq!(
            frame.photo_load_id.as_deref(),
            Some("photo-bbbbbbbbbbbbbbbb")
        );
    }

    #[test]
    fn a_failed_load_leaves_home_without_the_opening_spinner() {
        let mut frame = Frame {
            startup: Startup {
                manifest_loaded: true,
                state_loaded: true,
                started: true,
            },
            photos: vec![photo("photo-aaaaaaaaaaaaaaaa", 1)],
            panel_width: CLARA_BW_METRICS.width as u32,
            panel_height: CLARA_BW_METRICS.height as u32,
            ..Frame::default()
        };
        let mut context = Context::default();
        frame.start_current(&mut context);
        assert!(frame.advance_photo(
            &mut context,
            &StoreResult::Denied(kobo_sdk::StoreError::Missing)
        ));
        assert!(frame.picture.is_none());
        assert!(frame.photos_all_unreadable());
        assert_eq!(frame.view, View::Home);
        let home = frame.home();
        let labels = home
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_ui::Node::Splash { title, .. } => Some(title.as_str()),
                kobo_ui::Node::Activity { label, .. } => Some(label.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!labels.contains(&"Opening this photograph"));
        assert!(labels.contains(&"No photograph can be opened"));
    }

    #[test]
    fn a_mismatched_panel_size_is_fitted_instead_of_skipped() {
        let mut frame = Frame {
            photos: vec![photo("photo-aaaaaaaaaaaaaaaa", 1)],
            panel_width: CLARA_BW_METRICS.width as u32,
            panel_height: CLARA_BW_METRICS.height as u32,
            ..Frame::default()
        };
        let mut context = Context::default();
        let png = kobo_image::encode_png_grey(32, 24, &vec![180_u8; 32 * 24]).expect("png");
        frame.accept_picture(&mut context, "photo-aaaaaaaaaaaaaaaa", &png);
        assert!(frame.picture.is_some());
        assert!(frame.unreadable.is_empty());
        assert!(frame.notice.is_none());
    }

    #[test]
    fn action_graph_reaches_every_view() {
        use kobo_sdk::AppRunner;
        let manifest = format!(
            "cobalt-frame-v1\nphoto-aaaaaaaaaaaaaaaa\t{}\t12\tFamily\tone.png\n",
            "a".repeat(64)
        );
        let mut runner = AppRunner::new(Frame::default());
        runner.start();
        assert_eq!(runner.app().view, View::Opening);
        runner.store_result(StoreResult::Loaded {
            key: STATE.into(),
            value: None,
        });
        runner.store_result(StoreResult::ShelfRead {
            name: MANIFEST.into(),
            offset: 0,
            bytes: manifest.as_bytes().to_vec(),
            size: u32::try_from(manifest.len()).expect("manifest fits"),
        });
        assert_eq!(runner.app().view, View::Home);
        assert_eq!(runner.app().photos.len(), 1);
        assert!(runner.app().loading_selected());
        runner.action(action_id(MENU));
        assert!(runner.app().settings_open);
        runner.action(action_id(MODE));
        assert!(runner.app().settings.slow);
        runner.action(action_id(INTERVAL));
        runner.action(action_id(ORDER));
        runner.action(ActionId::BACK);
        assert!(!runner.app().settings_open);
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        assert!(runner.app().photos_all_unreadable());
        assert_eq!(runner.app().view, View::Home);
        assert!(!runner.app().loading_selected());
        let png = kobo_image::encode_png_grey(16, 16, &vec![200_u8; 16 * 16]).expect("png");
        runner.app_mut().unreadable.clear();
        runner.app_mut().notice = None;
        runner.action(action_id(SHOW));
        runner.store_result(StoreResult::ShelfRead {
            name: "photo-aaaaaaaaaaaaaaaa.png".into(),
            offset: 0,
            bytes: png.clone(),
            size: u32::try_from(png.len()).expect("png fits"),
        });
        assert!(runner.app().picture.is_some());
        runner.action(action_id(SHOW));
        assert_eq!(runner.app().view, View::Show);
        runner.action(action_id(MENU));
        assert!(runner.app().overlay);
        runner.action(action_id(EXIT));
        assert_eq!(runner.app().view, View::Home);
        assert!(!runner.app().overlay);
    }

    #[test]
    fn home_and_photo_layouts_are_clean() {
        let frame = Frame {
            startup: Startup {
                manifest_loaded: true,
                ..Startup::default()
            },
            photos: vec![photo("photo-aaaaaaaaaaaaaaaa", 1)],
            picture: Some(TilePicture::new(
                PHOTO,
                CLARA_BW_METRICS.width as u32,
                CLARA_BW_METRICS.height as u32,
            )),
            ..Frame::default()
        };
        for screen in [frame.home(), frame.photo_screen()] {
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }
}
