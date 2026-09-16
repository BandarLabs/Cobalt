//! Offline viewer for snapshots prepared by the host-side `kobo birds` companion.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, PictureHandle, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult, TilePicture,
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    generated_at: u64,
    source: String,
    recent: Vec<String>,
}

#[derive(Default)]
struct Birds {
    snapshot: Option<Snapshot>,
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
}

impl Birds {
    fn screen(&self) -> Screen {
        let Some(snapshot) = &self.snapshot else {
            let screen = ScreenBuilder::new("birds-home");
            return if self.loading {
                screen.activity("Opening the latest birds", None).build()
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
        screen.build().with_reading(true)
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn reload(&mut self, context: &mut Context) {
        self.loading = true;
        self.notice = None;
        self.snapshot_load = Some(ShelfDownload::new(SNAPSHOT).at_most(MAX_JSON));
        self.snapshot_load.as_mut().expect("set").start(context);
    }

    fn load_image(&mut self, context: &mut Context) {
        self.image_load = Some(ShelfDownload::new(IMAGE).at_most(MAX_IMAGE));
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
                    Ok(snapshot) => {
                        self.snapshot = Some(snapshot);
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

    fn install_picture(&mut self, context: &mut Context) {
        let Some(bytes) = self.image_bytes.as_deref() else {
            return;
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
        match decoded.and_then(|picture| picture.cover(self.panel_width, self.panel_height)) {
            Ok(picture) => {
                let width = picture.width();
                let height = picture.height();
                self.picture = if colour {
                    picture
                        .into_colour()
                        .and_then(|rgb| context.put_colour_picture(PICTURE, width, height, rgb))
                } else {
                    Some(context.put_picture(PICTURE, width, height, picture.into_grey())).flatten()
                };
                if self.picture.is_none() {
                    self.notice =
                        Some("The bird collage exceeds this reader's picture budget.".into());
                } else {
                    self.notice = None;
                }
            }
            Err(_) => self.notice = Some("The bird collage is damaged.".into()),
        }
    }

    fn advance_image(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.image_load else {
            return false;
        };
        match load.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.image_load.take().expect("active").take();
                self.image_bytes = Some(bytes);
                self.install_picture(context);
                self.loading = false;
                true
            }
            ShelfProgress::Failed(_) => {
                self.image_load = None;
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
    Ok(Snapshot {
        generated_at,
        source,
        recent,
    })
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
    fn layouts_are_clean() {
        let birds = Birds {
            snapshot: Some(Snapshot {
                generated_at: unix_seconds(),
                source: "Garden Mac".into(),
                recent: vec!["European Robin".into(), "Eurasian Wren".into()],
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
