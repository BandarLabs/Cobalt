//! Offline viewer for snapshots prepared by the host-side `kobo birds` companion.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, PictureHandle, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult, Task, TaskId, TaskOutcome,
    TilePicture,
};
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const SNAPSHOT: &str = "current.json";
const IMAGE: &str = "current.png";
const MAX_JSON: usize = 64 * 1024;
const MAX_IMAGE: usize = 4 * 1024 * 1024;
const PICTURE: PictureHandle = PictureHandle(1);
const REFRESH: &str = "refresh";
const MENU: &str = "menu";
const EXIT: &str = "exit";
/// How often the shelf is looked at again.
///
/// The companion polls the station every five seconds and publishes the
/// moment the birds change, so a reader who has to tap to see that is the
/// only still point in a live chain. Ten seconds keeps the page close to the
/// garden without waking a single-core reader more than it has to; the poll
/// itself reads a two-hundred-byte pointer, and the megabyte of collage
/// behind it is only opened when that pointer names a different picture.
const POLL_SECONDS: u32 = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    generated_at: u64,
    source: String,
    recent: Vec<String>,
    image_checksum: Option<String>,
    /// The content-addressed image this snapshot points at; snapshots written
    /// before the pointer existed fall back to the shared name.
    image: Option<String>,
}

#[derive(Default)]
struct Birds {
    snapshot: Option<Snapshot>,
    // A freshly downloaded snapshot waits here until its image has arrived,
    // decoded, and passed the pairing check; only then are both committed, so
    // a failed update preserves the last complete view.
    pending: Option<Snapshot>,
    picture: Option<TilePicture>,
    snapshot_load: Option<ShelfDownload>,
    image_load: Option<ShelfDownload>,
    loading: bool,
    notice: Option<String>,
    image_bytes: Option<Vec<u8>>,
    identity: Option<kobo_sdk::DeviceIdentity>,
    menu_open: bool,
    panel_width: u32,
    panel_height: u32,
    /// The sleep that becomes the next look at the shelf.
    tick: Option<TaskId>,
}

impl Birds {
    fn screen(&self) -> Screen {
        let Some(snapshot) = &self.snapshot else {
            let screen = ScreenBuilder::new("birds-home");
            return if self.loading {
                screen.activity("Opening the latest birds", None).build()
            } else if let Some(notice) = &self.notice {
                screen
                    .splash(Some(Glyph::App), "The snapshot could not be read", notice)
                    .buttons([(REFRESH, "Refresh")])
                    .build()
            } else {
                screen
                    .splash(
                        Some(Glyph::App),
                        "No birds yet",
                        "On your computer, run `kobo birds listen --source http://HOST:PORT --device IP`.",
                    )
                    .buttons([(REFRESH, "Refresh")])
                    .build()
            };
        };
        let age = unix_seconds().saturating_sub(snapshot.generated_at);
        let mut screen = ScreenBuilder::new("birds-home");
        if let Some(picture) = self.picture {
            // Fugleramme renders the names into the plate. The art is the screen,
            // with no app bar or facts pushing it into a card-sized window.
            screen = screen
                .unframed_picture(picture, 500)
                .page_turns(REFRESH, REFRESH)
                .reading_menu(MENU);
        } else {
            // A snapshot without a usable image must not become an empty
            // screen with no way out: say what happened and keep Refresh and
            // Exit reachable.
            let detail = self
                .notice
                .clone()
                .unwrap_or_else(|| "The latest bird collage is unavailable.".into());
            screen = screen
                .splash(Some(Glyph::App), "The collage needs a refresh", &detail)
                .buttons([(REFRESH, "Refresh"), (EXIT, "Exit")]);
        }
        if self.menu_open {
            let freshness = if age >= 24 * 60 * 60 {
                format!("Stale - last update was {} ago", age_label(age))
            } else {
                format!("Updated {} ago", age_label(age))
            };
            screen = screen.modal("Birds", |overlay| {
                let overlay =
                    overlay.facts([("Status", freshness), ("Source", snapshot.source.clone())]);
                if let Some(notice) = &self.notice {
                    overlay
                        .banner(BannerLevel::Attention, notice)
                        .buttons([(REFRESH, "Refresh"), (EXIT, "Exit")])
                } else {
                    overlay.buttons([(REFRESH, "Refresh"), (EXIT, "Exit")])
                }
            });
        }
        // Fugleramme renders the names into the plate, so a bar across the top
        // of it is somebody else's furniture laid over the art. Only while
        // there is art: the screens that say why there is none keep their bar,
        // because a reader looking at an explanation wants the way out in
        // sight. Touching where the bar would be brings it back, and the shell
        // owns both the hiding and the bringing back.
        screen
            .build()
            .with_reading(true)
            .with_auto_hidden_top_bar(self.picture.is_some())
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    /// Queues the next look at the shelf.
    ///
    /// Always from the end of the last one rather than on a fixed clock, so a
    /// slow read can never stack polls on a reader that is already busy.
    fn schedule_poll(&mut self, context: &mut Context) {
        if self.tick.is_none() {
            self.tick = context.spawn(Task::Sleep {
                seconds: POLL_SECONDS,
            });
        }
    }

    fn reload(&mut self, context: &mut Context) {
        self.loading = true;
        self.notice = None;
        self.snapshot_load = Some(ShelfDownload::new(SNAPSHOT).at_most(MAX_JSON));
        self.snapshot_load.as_mut().expect("set").start(context);
    }

    fn load_image(&mut self, context: &mut Context) {
        let name = self
            .pending
            .as_ref()
            .and_then(|snapshot| snapshot.image.clone())
            .unwrap_or_else(|| IMAGE.to_owned());
        self.image_load = Some(ShelfDownload::new(name).at_most(MAX_IMAGE));
        self.image_load.as_mut().expect("set").start(context);
    }

    fn advance_snapshot(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.snapshot_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.snapshot_load.take().expect("active").take();
                match decode_snapshot(&bytes) {
                    // A poll that finds the same pointer stops here. Opening
                    // and decoding a megabyte of collage every ten seconds to
                    // arrive at the picture already on the panel would cost a
                    // single-core reader its battery and show nothing new.
                    //
                    // Only a content-addressed pointer may be trusted this
                    // way. A snapshot with no `image` names the shared
                    // `current.png`, whose bytes can change underneath an
                    // unchanged snapshot, and skipping the read on that one
                    // would leave the panel stale for as long as it is open.
                    Ok(snapshot)
                        if snapshot.image.is_some()
                            && self.picture.is_some()
                            && self.snapshot.as_ref() == Some(&snapshot) =>
                    {
                        self.loading = false;
                    }
                    Ok(snapshot) => {
                        self.pending = Some(snapshot);
                        self.load_image(context);
                    }
                    Err(error) => {
                        self.loading = false;
                        self.notice = Some(error);
                    }
                }
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.snapshot_load = None;
                self.loading = false;
                true
            }
            ShelfProgress::Failed(_) => {
                self.snapshot_load = None;
                self.loading = false;
                self.notice = Some("The Birds snapshot could not be opened.".into());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn install_picture(&mut self, context: &mut Context) -> bool {
        let Some(bytes) = self.image_bytes.as_deref() else {
            return false;
        };
        let colour = self
            .identity
            .as_ref()
            .is_some_and(kobo_sdk::DeviceIdentity::colour_panel);
        let decoded = if colour {
            kobo_image::decode_colour(bytes)
        } else {
            kobo_image::decode(bytes)
        };
        // The picture byte budget (kobo_protocol::MAX_PICTURE_BYTES) counts
        // RGB at 3 bytes per pixel, so a full-bleed cover of a Libra Colour
        // panel (1264x1680) does not fit. Scale the target down until it
        // does; the art stays full-bleed, just a touch below panel pixels.
        let (mut width, mut height) = (self.panel_width, self.panel_height);
        if colour {
            const PICTURE_BYTE_BUDGET: u64 = 4 * 1072 * 1448;
            while u64::from(width) * u64::from(height) * 3 > PICTURE_BYTE_BUDGET {
                width = width * 99 / 100;
                height = height * 99 / 100;
            }
        }
        match decoded.and_then(|picture| picture.cover(width, height)) {
            Ok(picture) => {
                let width = picture.width();
                let height = picture.height();
                let installed = if colour {
                    picture
                        .into_colour()
                        .and_then(|rgb| context.put_colour_picture(PICTURE, width, height, rgb))
                } else {
                    Some(context.put_picture(PICTURE, width, height, picture.into_grey())).flatten()
                };
                if let Some(installed) = installed {
                    self.picture = Some(installed);
                    self.notice = None;
                    return true;
                }
                self.notice = Some("The bird collage exceeds this reader's picture budget.".into());
            }
            Err(_) => self.notice = Some("The bird collage is damaged.".into()),
        }
        // A failed attempt keeps the previous picture on screen, but reports
        // failure so the caller does not commit new metadata over old art.
        false
    }

    fn advance_image(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.image_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.image_load.take().expect("active").take();
                let pending_checksum = self
                    .pending
                    .as_ref()
                    .and_then(|snapshot| snapshot.image_checksum.clone());
                if let Some(expected) = pending_checksum {
                    if fnv64(&bytes) != expected {
                        // The snapshot and the image on the shelf came from
                        // different generations; keep the last complete view.
                        self.pending = None;
                        self.loading = false;
                        self.notice = Some(
                            "The update is incomplete, so the last complete view is kept.".into(),
                        );
                        return true;
                    }
                }
                self.image_bytes = Some(bytes);
                // Gate the commit on this decode, not on whatever picture a
                // previous generation left behind: a damaged new image must
                // keep the old snapshot's metadata with the old image.
                if self.install_picture(context) {
                    self.snapshot = self.pending.take();
                } else {
                    self.pending = None;
                }
                self.loading = false;
                true
            }
            ShelfProgress::Failed(_) => {
                self.image_load = None;
                self.pending = None;
                self.loading = false;
                self.notice = Some("The bird collage is missing.".into());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
}

impl KoboApp for Birds {
    fn on_start(&mut self, context: &mut Context) {
        self.panel_width = u32::try_from(context.metrics().width).unwrap_or_default();
        self.panel_height = u32::try_from(context.metrics().height).unwrap_or_default();
        context.device().read_identity();
        self.reload(context);
        self.schedule_poll(context);
        self.show(context);
    }

    /// The shelf is looked at again on every tick, so a collage published
    /// while the reader is watching arrives on its own. A reader who never
    /// touches the panel still sees the birds change.
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.tick != Some(task) {
            return;
        }
        self.tick = None;
        // Sleep is paused and cancelled for the whole application when the
        // reader closes the cover, and queueing another one against a paused
        // manager is answered by cancelling that too. So a cancelled poll is
        // not rearmed here; waking is what rearms it, in `on_resume`.
        if matches!(outcome, TaskOutcome::Cancelled) {
            return;
        }
        // A read already in flight is left to finish; the next tick will find
        // whatever it committed.
        if self.snapshot_load.is_none() && self.image_load.is_none() {
            self.reload(context);
        }
        self.schedule_poll(context);
        self.show(context);
    }

    /// Suspending cancels the poll, so waking has to start it again, and a
    /// reader who closed the cover on one bird and opened it on another is
    /// exactly who this app is for: read the shelf now rather than ten
    /// seconds into being awake.
    fn on_resume(&mut self, context: &mut Context) {
        if self.snapshot_load.is_none() && self.image_load.is_none() {
            self.reload(context);
        }
        self.schedule_poll(context);
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if request == kobo_sdk::DeviceRequest::ReadIdentity {
            self.identity = match result {
                kobo_sdk::DeviceResult::Identity(identity) => Some(identity),
                _ => None,
            };
            self.install_picture(context);
            self.show(context);
        }
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.advance_snapshot(context, &result) || self.advance_image(context, &result) {
            self.show(context);
        }
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK || action == action_id(EXIT) {
            if self.menu_open {
                self.menu_open = false;
            } else {
                context.exit();
            }
        } else if action == action_id(MENU) {
            self.menu_open = !self.menu_open;
        } else if action == action_id(REFRESH) {
            self.menu_open = false;
            self.reload(context);
        }
        self.show(context);
    }
}

fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Birds snapshot is not UTF-8")?;
    let value = kobo_json::parse(text).map_err(|_| "Birds snapshot is invalid JSON")?;
    if value.get("format").and_then(kobo_json::Value::as_str) != Some("cobalt-birds-v1") {
        return Err("Birds snapshot has an unsupported version".into());
    }
    let generated_at = value
        .get("generated_at")
        .and_then(kobo_json::Value::as_i64)
        .and_then(|v| u64::try_from(v).ok())
        .ok_or("Birds snapshot has no valid time")?;
    let source = value
        .get("source")
        .and_then(kobo_json::Value::as_str)
        .unwrap_or("Local BirdNET-Go")
        .chars()
        .take(120)
        .collect();
    let recent = value
        .get("recent")
        .and_then(kobo_json::Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(kobo_json::Value::as_str)
        .take(32)
        .map(|s| s.chars().take(100).collect())
        .collect();
    let image_checksum = value
        .get("image_checksum")
        .and_then(kobo_json::Value::as_str)
        .filter(|s| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_owned);
    let image = value
        .get("image")
        .and_then(kobo_json::Value::as_str)
        .filter(|s| {
            s.len() <= 64
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        })
        .map(str::to_owned);
    Ok(Snapshot {
        generated_at,
        source,
        recent,
        image_checksum,
        image,
    })
}

/// The same FNV-1a 64 the companion writes into `image_checksum`.
fn fnv64(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn age_label(seconds: u64) -> String {
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{} min", seconds / 60)
    } else if seconds < 86400 {
        format!("{} hr", seconds / 3600)
    } else {
        format!("{} days", seconds / 86400)
    }
}
fn main() -> ExitCode {
    kobo_sdk::run("birds", Birds::default()).map_or_else(
        |error| {
            eprintln!("birds: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::AppRunner;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn snapshot_is_bounded_and_parsed() {
        let s=decode_snapshot(br#"{"format":"cobalt-birds-v1","generated_at":1,"source":"garden","recent":["Robin","Wren"]}"#).unwrap();
        assert_eq!(s.recent, ["Robin", "Wren"]);
        assert!(decode_snapshot(b"{}").is_err());
    }
    #[test]
    fn snapshot_carries_the_pairing_checksum() {
        let s = decode_snapshot(
            br#"{"format":"cobalt-birds-v1","generated_at":1,"image_checksum":"af63dc4c8601ec8c"}"#,
        )
        .unwrap();
        assert_eq!(s.image_checksum.as_deref(), Some("af63dc4c8601ec8c"));
        // Missing or malformed checksums stay optional so older snapshots load.
        assert!(
            decode_snapshot(br#"{"format":"cobalt-birds-v1","generated_at":1}"#)
                .unwrap()
                .image_checksum
                .is_none()
        );
        assert!(decode_snapshot(
            br#"{"format":"cobalt-birds-v1","generated_at":1,"image_checksum":"zz"}"#
        )
        .unwrap()
        .image_checksum
        .is_none());
        // The app and the companion must agree on the checksum function.
        assert_eq!(fnv64(b""), "cbf29ce484222325");
        assert_eq!(fnv64(b"a"), "af63dc4c8601ec8c");
    }
    #[test]
    fn age_is_readable() {
        assert_eq!(age_label(20), "just now");
        assert_eq!(age_label(7200), "2 hr");
    }
    #[test]
    fn action_graph_reloads() {
        let mut runner = AppRunner::new(Birds::default());
        runner.start();
        assert!(runner.app().snapshot_load.is_some());
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        assert!(!runner.app().loading);
        runner.action(action_id(REFRESH));
        assert!(runner.app().loading);
    }
    #[test]
    fn the_shelf_is_looked_at_again_without_anybody_touching_the_panel() {
        // The companion publishes every few seconds, so a reader who had to
        // tap to see that was the only still point in a live chain.
        let mut runner = AppRunner::new(Birds::default());
        runner.start();
        let first = runner.app().tick.expect("a poll is queued on opening");
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        assert!(!runner.app().loading);
        assert!(runner.app().snapshot_load.is_none());

        runner.task_outcome(first, TaskOutcome::Completed(Vec::new()));
        assert!(
            runner.app().snapshot_load.is_some(),
            "a tick must read the shelf again on its own"
        );
        let second = runner.app().tick.expect("the next poll is queued");
        assert_ne!(second, first, "each tick queues the following one");
    }

    #[test]
    fn a_cancelled_poll_is_not_replaced_and_a_busy_reader_is_not_asked_twice() {
        let mut runner = AppRunner::new(Birds::default());
        runner.start();
        let tick = runner.app().tick.expect("queued");
        // A read is still in flight from opening, so this tick must not start
        // a second one on top of it.
        let before = runner.outstanding_requests();
        runner.task_outcome(tick, TaskOutcome::Completed(Vec::new()));
        assert_eq!(
            runner.outstanding_requests(),
            before,
            "a tick during a read must not queue the same read again"
        );

        let mut runner = AppRunner::new(Birds::default());
        runner.start();
        let tick = runner.app().tick.expect("queued");
        runner.task_outcome(tick, TaskOutcome::Cancelled);
        assert!(
            runner.app().tick.is_none(),
            "queueing a poll against a paused manager only earns another cancellation"
        );
    }

    #[test]
    fn waking_starts_the_poll_the_cover_cancelled() {
        // Closing the cover pauses this application's tasks, which cancels
        // the sleep in flight. Without this the first sleep of the reader's
        // day was the last poll of it, and the app went quietly back to
        // needing a tap.
        let mut runner = AppRunner::new(Birds::default());
        runner.start();
        let tick = runner.app().tick.expect("queued");
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        runner.suspend();
        runner.task_outcome(tick, TaskOutcome::Cancelled);
        assert!(runner.app().tick.is_none());

        runner.resume();
        assert!(
            runner.app().tick.is_some(),
            "waking must start the poll again"
        );
        assert!(
            runner.app().snapshot_load.is_some(),
            "a reader who closed the cover on one bird and opened it on another \
             should not wait a further ten seconds"
        );
    }

    #[test]
    fn layouts_are_clean() {
        let birds = Birds {
            snapshot: Some(Snapshot {
                generated_at: unix_seconds(),
                source: "Garden Mac".into(),
                recent: vec!["European Robin".into(), "Eurasian Wren".into()],
                image_checksum: None,
                image: None,
            }),
            picture: Some(TilePicture::new(PICTURE, 800, 600)),
            ..Birds::default()
        };
        assert!(birds
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
