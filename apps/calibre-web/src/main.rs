//! An authenticated OPDS 1.2 library client for calibre-web.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, Context, Credential, KoboApp, Screen, ScreenBuilder, StoreResult, Task,
    TaskId, TaskOutcome,
};
use std::process::ExitCode;
const REGISTRY: &str = "catalog";
const BYTES: u32 = 256 * 1024;
#[derive(Clone, Copy, PartialEq)]
enum View {
    Libraries,
    Url,
    Credential,
    Library,
    Failure,
}
struct Calibre {
    view: View,
    keyboard: Keyboard,
    url: String,
    credential: Option<String>,
    task: Option<TaskId>,
    loaded: bool,
}
impl Default for Calibre {
    fn default() -> Self {
        Self {
            view: View::Libraries,
            keyboard: Keyboard::new(),
            url: String::new(),
            credential: None,
            task: None,
            loaded: false,
        }
    }
}
impl Calibre {
    fn fetch(&mut self, c: &mut Context) {
        let credential = self.credential.as_ref().map(Credential::basic);
        self.task = c.spawn(Task::Fetch {
            url: format!("{}/opds", self.url.trim_end_matches('/')),
            offset: 0,
            max_bytes: BYTES,
            credential,
            headers: Vec::new(),
        });
    }
    fn screen(&self) -> Screen {
        match self.view {
            View::Libraries => {
                let mut s = ScreenBuilder::new("calibre-libraries")
                    .top_bar("Libraries")
                    .top_bar_action("add", "Add");
                if !self.loaded {
                    s = s.secondary("Loading libraries…");
                } else if self.url.is_empty() {
                    s = s
                        .splash(
                            Some(kobo_sdk::Glyph::Book),
                            "No libraries",
                            "Add the HTTPS address of your calibre-web library.",
                        )
                        .primary_button("add", "Add library");
                } else {
                    s = s.rows([(
                        "library",
                        self.url.as_str(),
                        self.credential.as_ref().map_or("Open catalog", |n| {
                            if n.is_empty() {
                                "Open catalog"
                            } else {
                                "Account connected"
                            }
                        }),
                        kobo_sdk::Glyph::Book,
                    )]);
                }
                s.build()
            }
            View::Url => ScreenBuilder::new("calibre-url")
                .top_bar("Library address")
                .heading("Library address")
                .secondary("Enter the secure web address for your calibre-web library.")
                .typed(&self.keyboard, "https://library.example")
                .keyboard(&self.keyboard, "Continue")
                .owns_back(true)
                .build(),
            View::Credential => ScreenBuilder::new("calibre-credential")
                .top_bar("Sign in")
                .heading("Optional account")
                .secondary("Enter the account name you set up on your computer, or leave blank for a public library.")
                .keyboard(&self.keyboard, "Add library")
                .owns_back(true)
                .build(),
            View::Library => ScreenBuilder::new("calibre-library")
                .top_bar("Library")
                .secondary(if self.task.is_some() {
                    "Opening…"
                } else {
                    "Library ready"
                })
                .rows([
                    (
                        "new",
                        "New books",
                        "Browse this section",
                        kobo_sdk::Glyph::Book,
                    ),
                    (
                        "authors",
                        "Authors",
                        "Browse by author",
                        kobo_sdk::Glyph::Book,
                    ),
                    (
                        "shelves",
                        "Shelves",
                        "Your calibre-web shelves",
                        kobo_sdk::Glyph::Book,
                    ),
                ])
                .bottom_action("libraries", "Libraries")
                .build(),
            View::Failure => {
                let advice = self.credential.as_deref().map_or_else(
                    || {
                        "This library needs an account. Finish setup on your computer, then add the library again.".to_owned()
                    },
                    |_| "The connected account was refused. Check its setup on your computer.".to_owned(),
                );
                ScreenBuilder::new("calibre-failure")
                    .top_bar("Library")
                    .heading("Library unavailable")
                    .text(advice)
                    .bottom_action("libraries", "Libraries")
                    .build()
            }
        }
    }
}
impl KoboApp for Calibre {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(REGISTRY);
        c.set_screen(self.screen());
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if let StoreResult::Loaded { key, value } = r {
            if key == REGISTRY {
                if let Some(v) = value {
                    if let Ok(s) = String::from_utf8(v) {
                        let mut p = s.splitn(2, '|');
                        self.url = p.next().unwrap_or_default().into();
                        self.credential = p.next().filter(|x| !x.is_empty()).map(str::to_owned);
                    }
                }
                self.loaded = true;
                c.set_screen(self.screen());
            }
        }
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        match self.view {
            View::Url => {
                if let Some(Pressed::Submitted) = self.keyboard.press(a) {
                    let url = self.keyboard.take();
                    if url.starts_with("https://") {
                        self.url = url;
                        self.view = View::Credential;
                    }
                }
            }
            View::Credential => {
                if let Some(Pressed::Submitted) = self.keyboard.press(a) {
                    let n = self.keyboard.take();
                    self.credential = (!n.trim().is_empty()).then_some(n);
                    c.store().save(
                        REGISTRY,
                        format!(
                            "{}|{}",
                            self.url,
                            self.credential.clone().unwrap_or_default()
                        ),
                    );
                    self.view = View::Library;
                    self.fetch(c);
                }
            }
            _ => {
                if a == action_id("add") {
                    self.view = View::Url;
                    self.keyboard = Keyboard::with_text("https://");
                } else if a == action_id("library") {
                    self.view = View::Library;
                    self.fetch(c);
                } else if a == action_id("libraries") || a == ActionId::BACK {
                    self.view = View::Libraries;
                }
            }
        }
        c.set_screen(self.screen());
    }
    fn on_task(&mut self, c: &mut Context, id: TaskId, o: TaskOutcome) {
        if self.task == Some(id) {
            self.task = None;
            if !matches!(o, TaskOutcome::Completed(_)) {
                self.view = View::Failure;
            }
            c.set_screen(self.screen());
        }
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("calibre-web", Calibre::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("calibre-web: {e}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};
    #[test]
    fn basic_credential_never_contains_password() {
        let c = Credential::basic("calibre");
        assert_eq!(c.secret, "calibre");
        assert_eq!(c.header_name(), "Authorization");
    }
    #[test]
    fn library_add_fits() {
        let a = Calibre::default();
        assert!(a
            .screen()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
        assert!(a
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("add"))
            .is_some());
    }
}
