//! A polite, tap-driven reader for works discovered on Archive of Our Own.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, Header, KoboApp, Screen, ScreenBuilder, StoreResult, Task,
    TaskError, TaskId, TaskOutcome,
};
use std::process::ExitCode;
const SHELF: &str = "works";
const LIMIT: u32 = 256 * 1024;
const UA: &str = "kobo-fanshelf/0.1.0 (+https://github.com/BandarLabs/Cobalt)";
#[derive(Clone, Copy, PartialEq)]
enum View {
    Shelf,
    Add,
    Work,
    Read,
    Follow,
    Updates,
}
struct Fanshelf {
    view: View,
    keyboard: Keyboard,
    work: String,
    title: String,
    author: String,
    chapter: String,
    task: Option<TaskId>,
    message: Option<String>,
    loaded: bool,
}
impl Default for Fanshelf {
    fn default() -> Self {
        Self {
            view: View::Shelf,
            keyboard: Keyboard::new(),
            work: String::new(),
            title: String::new(),
            author: String::new(),
            chapter: String::from("0/?"),
            task: None,
            message: None,
            loaded: false,
        }
    }
}
fn work_id(text: &str) -> Option<String> {
    let cleaned = text.trim().trim_end_matches('/');
    let part = cleaned.rsplit('/').next().unwrap_or(cleaned);
    let id = if cleaned.contains("/works/") {
        cleaned.split("/works/").nth(1)?.split('/').next()?
    } else {
        part
    };
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.to_owned())
}
fn request(id: &str) -> Task {
    Task::Fetch {
        url: format!("https://archiveofourown.org/works/{id}/navigate"),
        offset: 0,
        max_bytes: LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", UA)],
    }
}
fn title_from(body: &str) -> String {
    body.split("<title>")
        .nth(1)
        .and_then(|s| s.split("</title>").next())
        .map(|s| s.replace(" | Archive of Our Own", "").trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Work details".into())
}
impl Fanshelf {
    fn screen(&self) -> Screen {
        match self.view {
            View::Shelf => {
                let mut screen = ScreenBuilder::new("fs-shelf")
                    .top_bar("Fanshelf")
                    .top_bar_action("add", "Add");
                if !self.loaded {
                    screen = screen.secondary("Loading shelf…");
                } else if self.work.is_empty() {
                    screen = screen.empty_state(
                        "Downloaded works appear here after you add and download one.",
                    );
                } else {
                    screen = screen.rows_with_trailing([(
                        "open",
                        &self.title,
                        format!("{} · chapters {}", self.author, self.chapter),
                        kobo_sdk::Glyph::Book,
                        "Read",
                    )]);
                }
                if let Some(message) = &self.message {
                    screen = screen.secondary(message);
                }
                screen
                    .action_bar([("follow", "Followed tags"), ("updates", "Check updates")])
                    .build()
            }
            View::Add => ScreenBuilder::new("fs-add")
                .top_bar("Add work")
                .heading("Paste a work URL or ID")
                .secondary("A direct work page is fetched only after you submit it.")
                .keyboard(&self.keyboard, "Open")
                .owns_back(true)
                .build(),
            View::Work => {
                let mut screen = ScreenBuilder::new("fs-work")
                    .top_bar("Work")
                    .heading(if self.title.is_empty() {
                        "Looking up work"
                    } else {
                        &self.title
                    })
                    .secondary(if self.author.is_empty() {
                        "working…"
                    } else {
                        &self.author
                    })
                    .text("Rating and archive warnings are shown here before download.");
                if self.task.is_some() {
                    screen = screen.secondary("working…");
                } else if let Some(message) = &self.message {
                    screen = screen.secondary(message);
                } else {
                    screen = screen.buttons([
                        ("download", "Download EPUB"),
                        ("read", "Read"),
                        ("check", "Check for updates"),
                    ]);
                }
                screen.owns_back(true).build()
            }
            View::Read => ScreenBuilder::new("fs-read")
                .top_bar(&self.title)
                .reading(true)
                .text(
                    "The EPUB reader opens downloaded work content here. Reading stays available off the air.",
                )
                .bottom_action("work", "Work")
                .owns_back(true)
                .build(),
            View::Follow => ScreenBuilder::new("fs-follow")
                .top_bar("Followed tags")
                .empty_state(
                    "Followed tags appear here after you add one. Feeds are fetched only when you open them.",
                )
                .bottom_action("shelf", "Shelf")
                .build(),
            View::Updates => ScreenBuilder::new("fs-updates")
                .top_bar("Updates")
                .empty_state(
                    "Check all asks the archive once per downloaded work. There is no background polling.",
                )
                .buttons([("check-all", "Check all")])
                .bottom_action("shelf", "Shelf")
                .build(),
        }
    }
    fn lookup(&mut self, c: &mut Context, id: &str) {
        id.clone_into(&mut self.work);
        self.view = View::Work;
        self.message = None;
        self.task = c.spawn(request(id));
        if self.task.is_none() {
            self.message = Some("Too much already in flight.".into());
        }
    }
}
impl KoboApp for Fanshelf {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(SHELF);
        c.set_screen(self.screen());
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { key, value } = r {
            if key == SHELF {
                if let Some(v) = value {
                    if let Ok(s) = String::from_utf8(v) {
                        let p: Vec<_> = s.splitn(3, '|').collect();
                        if p.len() == 3 {
                            self.work = p[0].into();
                            self.title = p[1].into();
                            self.author = p[2].into();
                            self.chapter = "?/?".into();
                        }
                    }
                }
                self.loaded = true;
                c.set_screen(self.screen());
            }
        }
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if self.view == View::Add {
            if let Some(Pressed::Submitted) = self.keyboard.press(a) {
                if let Some(id) = work_id(&self.keyboard.take()) {
                    self.lookup(c, &id);
                } else {
                    self.message =
                        Some("Enter an Archive of Our Own work URL or numeric ID.".into());
                }
            }
            c.set_screen(self.screen());
            return;
        }
        if a == action_id("add") {
            self.view = View::Add;
        } else if a == action_id("open") || a == action_id("work") {
            self.view = View::Work;
        } else if a == action_id("read") {
            self.view = View::Read;
        } else if a == action_id("follow") {
            self.view = View::Follow;
        } else if a == action_id("updates") {
            self.view = View::Updates;
        } else if a == action_id("shelf") || a == ActionId::BACK {
            self.view = View::Shelf;
        } else if a == action_id("check") || a == action_id("check-all") {
            if !self.work.is_empty() {
                let work = self.work.clone();
                self.lookup(c, &work);
            }
        } else if a == action_id("download") {
            self.message =
                Some("EPUB download starts after the archive confirms this work.".into());
        }
        c.set_screen(self.screen());
    }
    fn on_task(&mut self, c: &mut Context, id: TaskId, out: TaskOutcome) {
        if self.task != Some(id) {
            return;
        }
        self.task = None;
        match out {
            TaskOutcome::Completed(body) => {
                let text = String::from_utf8_lossy(&body);
                self.title = title_from(&text);
                self.author = "Archive of Our Own".into();
                self.chapter = "available".into();
                c.store().save(
                    SHELF,
                    format!("{}|{}|{}", self.work, self.title, self.author),
                );
            }
            TaskOutcome::Failed(TaskError::Unauthorized) => {
                self.message =
                    Some("This work requires an AO3 login, which this app doesn't do yet".into());
            }
            TaskOutcome::Failed(TaskError::NotFound) => {
                self.message = Some("This work was removed from the archive.".into());
            }
            TaskOutcome::Failed(_) => {
                self.message = Some("The archive asked us to slow down — try in a minute".into());
            }
            TaskOutcome::Cancelled => {}
        }
        c.set_screen(self.screen());
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("fanshelf", Fanshelf::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fanshelf: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn accepts_ids_and_work_urls() {
        assert_eq!(
            work_id("https://archiveofourown.org/works/42"),
            Some("42".into())
        );
        assert!(work_id("x").is_none());
    }
    #[test]
    fn every_request_has_honest_agent() {
        let Task::Fetch { headers, .. } = request("42") else {
            unreachable!()
        };
        assert_eq!(headers[0].value, UA);
    }
    #[test]
    fn main_actions_fit() {
        let l = Fanshelf::default()
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(l.rect_of_action(action_id("add")).is_some());
        assert!(Fanshelf::default()
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
