//! Panels reads your CBZ files locally and queries a Komga OPDS endpoint
//! through runtime-owned HTTP Basic credentials.

mod archive;

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, PictureHandle,
    Screen, ScreenBuilder, Task, TaskId, TaskOutcome, TilePicture,
};
use std::process::ExitCode;

const SIDELOAD: &str = "volume.cbz";
const KOMGA_OPDS: &str = "https://komga.local/opds/v1.2/catalog";
const PAGE: PictureHandle = PictureHandle(1);
const MAX_CBZ: u32 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Library,
    Reader,
}

#[derive(Default)]
struct Panels {
    route: Route,
    bytes: Option<Vec<u8>>,
    comic: Option<archive::Comic>,
    page: usize,
    rtl: bool,
    picture: Option<TilePicture>,
    notice: Option<String>,
    task: Option<TaskId>,
}

impl Panels {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Library));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Library => self.library(),
            Route::Reader => self.reader(),
        }
    }

    fn library(&self) -> Screen {
        let mut screen = ScreenBuilder::new("panels-library").top_bar("Panels");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if let Some(comic) = &self.comic {
            screen = screen
                .section("On this reader")
                .rows([("open", "Sideloaded volume", format!("{} pages", comic.pages.len()), Glyph::Reader)]);
        } else {
            screen = screen.empty_state("A sideloaded CBZ appears here after it is copied as volume.cbz.");
        }
        screen
            .primary_button("load-sideload", "Open sideloaded CBZ")
            .button("browse-komga", "Browse Komga")
            .build()
    }

    fn reader(&self) -> Screen {
        let Some(comic) = &self.comic else {
            return self.library();
        };
        let mut screen = ScreenBuilder::new("panels-reader")
            .top_bar("Sideloaded volume")
            .secondary(format!("Page {} of {}", self.page + 1, comic.pages.len()));
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if let Some(picture) = self.picture {
            screen = screen.unframed_picture(picture, 154);
        } else {
            screen = screen.activity("Decoding page", None);
        }
        screen
            .buttons([("previous", if self.rtl { "Next page" } else { "Previous page" }),
                      ("next", if self.rtl { "Previous page" } else { "Next page" })])
            .button("rtl", if self.rtl { "Reading order: right to left" } else { "Reading order: left to right" })
            .build()
    }

    fn load_sideload(&mut self, context: &mut Context) {
        if let Some(task) = context.spawn(Task::ReadFile { path: SIDELOAD.to_owned() }) {
            self.task = Some(task);
            self.notice = Some("Reading sideloaded CBZ.".to_owned());
        }
    }

    fn display_page(&mut self, context: &mut Context) {
        let (Some(bytes), Some(comic)) = (&self.bytes, &self.comic) else { return };
        match archive::page(bytes, comic, self.page) {
            Ok(picture) => {
                self.picture = context.put_picture(PAGE, picture.width(), picture.height(), picture.grey().to_vec());
                self.notice = self.picture.is_none().then_some("This page is too large for the panel cache.".to_owned());
            }
            Err(error) => self.notice = Some(format!("{error}. Skip to the next page.")),
        }
    }

    fn turn(&mut self, context: &mut Context, forward: bool) {
        let Some(comic) = &self.comic else { return };
        let forward = if self.rtl { !forward } else { forward };
        if forward && self.page + 1 < comic.pages.len() {
            self.page += 1;
        } else if !forward && self.page > 0 {
            self.page -= 1;
        } else {
            return;
        }
        self.picture = None;
        self.notice = None;
        self.display_page(context);
    }
}

impl KoboApp for Panels {
    fn on_start(&mut self, context: &mut Context) { self.show(context); }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.route = Route::Library;
        } else if action == action_id("load-sideload") {
            self.load_sideload(context);
        } else if action == action_id("open") {
            self.route = Route::Reader;
            self.display_page(context);
        } else if action == action_id("next") {
            self.turn(context, true);
        } else if action == action_id("previous") {
            self.turn(context, false);
        } else if action == action_id("rtl") {
            self.rtl = !self.rtl;
        } else if action == action_id("browse-komga") {
            if let Some(task) = context.spawn_retrying(Task::Fetch {
                url: KOMGA_OPDS.to_owned(), offset: 0, max_bytes: MAX_CBZ,
                credential: Some(Credential::basic("komga")), headers: Vec::new(),
            }) {
                self.task = Some(task);
                self.notice = Some("Reading Komga catalog.".to_owned());
            }
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) { return; }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match archive::inspect(&bytes) {
                Ok(comic) => {
                    self.bytes = Some(bytes);
                    self.comic = Some(comic);
                    self.page = 0;
                    self.notice = None;
                }
                Err(_) => self.notice = Some("Komga catalog was fetched. OPDS browsing is not yet decoded by this MVP.".to_owned()),
            },
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential) =>
                self.notice = Some("Install a Komga Basic key with kobo secret set komga.".to_owned()),
            TaskOutcome::Failed(error) => self.notice = Some(Failure::of(error).naming("komga")),
            TaskOutcome::Cancelled => self.notice = Some("The transfer was cancelled.".to_owned()),
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("panels", Panels::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => { eprintln!("panels: {error}"); ExitCode::FAILURE }
    }
}

#[cfg(test)]
mod tests {
    use super::{Panels, Route};
    use kobo_sdk::{action_id, Context, KoboApp};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn library_primary_control_fits_the_actual_panel() {
        let app = Panels::default();
        let layout = app.library().layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("load-sideload")).is_some());
        assert!(app.library().diagnostics(&CLARA_BW_METRICS, &Chrome::default()).issues.is_empty());
    }

    #[test]
    fn rtl_reverses_reader_controls_without_changing_the_page_order() {
        let mut app = Panels::default();
        let mut context = Context::default();
        app.on_action(&mut context, action_id("rtl"));
        assert!(app.rtl);
        app.on_action(&mut context, action_id("open"));
        assert_eq!(app.route, Route::Reader);
    }
}
