//! An authenticated OPDS 1.2 library client for calibre-web.
mod books;
mod catalog;
mod reading;
mod settings;
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
    Book,
}
struct Calibre {
    view: View,
    keyboard: Keyboard,
    url: String,
    credential: Option<String>,
    task: Option<TaskId>,
    loaded: bool,
    settings: settings::Settings,
    account: kobo_sdk::credentials::CredentialSetup,
    page: Option<catalog::Page>,
    history: Vec<catalog::Page>,
    pending_url: String,
    notice: String,
    selected: usize,
    reading: reading::Reading,
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
            settings: settings::Settings::default(),
            account: kobo_sdk::credentials::CredentialSetup::new("calibre", "calibre-web")
                .with_basic(),
            page: None,
            history: Vec::new(),
            pending_url: String::new(),
            notice: String::new(),
            selected: 0,
            reading: reading::Reading::default(),
        }
    }
}
impl Calibre {
    fn fetch(&mut self, c: &mut Context) {
        if let Some(url) = catalog::endpoint(&self.url) {
            self.fetch_url(c, url);
        }
    }
    fn fetch_url(&mut self, c: &mut Context, url: String) {
        if !catalog::allowed(&self.url, &url) {
            self.notice =
                "This link leaves your library. Its account cannot be sent to another server."
                    .into();
            self.view = View::Failure;
            return;
        }
        if let Some(task) = self.task.take() {
            c.cancel(task);
        }
        self.pending_url.clone_from(&url);
        self.notice.clear();
        self.view = View::Library;
        self.task = c.spawn(Task::Fetch {
            url,
            offset: 0,
            max_bytes: BYTES,
            credential: self.credential.as_ref().map(Credential::basic),
            headers: vec![kobo_sdk::Header::new("Accept", kobo_opds::ACCEPT)],
        });
        if self.task.is_none() {
            self.notice = "The request could not start. Try again.".into();
            self.view = View::Failure;
        }
    }
    fn library_screen(&self) -> Screen {
        let mut s = ScreenBuilder::new("calibre-library")
            .top_bar("Library")
            .owns_back(true);
        if self.task.is_some() {
            return s
                .secondary("Opening catalog…")
                .button("cancel", "Cancel")
                .build();
        }
        let Some(page) = &self.page else {
            return s
                .secondary("No catalog loaded.")
                .button("retry", "Try again")
                .build();
        };
        if self.settings.state == settings::State::Failed {
            s = s
                .secondary("Library setup is not saved.")
                .button("settings-save", "Retry saving setup");
        } else if self.settings.state == settings::State::Saving {
            s = s.secondary("Saving library setup…");
        }
        s = s.secondary(page.feed.title.as_deref().unwrap_or("Catalog"));
        if page.count() == 0 {
            s = s.text("No books or sections in this catalog.");
        }
        for index in page.offset..(page.offset + 2).min(page.count()) {
            let (title, subtitle) = if let Some(nav) = page.feed.navigation.get(index) {
                (nav.title.as_str(), "Browse section".to_owned())
            } else if let Some(book) = page.book(index) {
                (book.title.as_str(), book.authors.join(", "))
            } else {
                continue;
            };
            let title: String = title.chars().take(65).collect();
            let subtitle: String = subtitle.chars().take(75).collect();
            s = s.rows([(
                format!("entry-{index}"),
                title,
                subtitle,
                kobo_sdk::Glyph::Book,
            )]);
        }
        if page.offset > 0 {
            s = s.button("page-back", "Previous entries");
        }
        if page.offset + 2 < page.count() {
            s = s.button("page-next", "More entries");
        } else if page.feed.next().is_some() {
            s = s.button("catalog-next", "Next catalog page");
        }
        s.bottom_action("libraries", "Libraries").build()
    }
    fn setup_or_library_action(&mut self, c: &mut Context, a: ActionId) {
        match self.view {
            View::Url => {
                if let Some(Pressed::Submitted) = self.keyboard.press(a) {
                    let url = self.keyboard.take();
                    if let Some(url) = catalog::endpoint(&url) {
                        self.url = url;
                        self.notice.clear();
                        self.view = View::Credential;
                    } else {
                        self.keyboard = Keyboard::with_text(&url);
                        self.notice =
                            "Use an HTTPS OPDS address without a username or password.".into();
                    }
                }
            }
            _ => {
                if a == action_id("add") {
                    self.view = View::Url;
                    self.notice.clear();
                    self.keyboard = Keyboard::with_text("https://");
                } else if a == action_id("library") {
                    self.view = View::Library;
                    self.page = None;
                    self.history.clear();
                    self.fetch(c);
                } else if a == action_id("libraries") || a == ActionId::BACK {
                    self.view = View::Libraries;
                }
            }
        }
    }
    fn settings_action(&mut self, c: &mut Context, a: ActionId) -> bool {
        if self.account.is_open() {
            self.account.on_action(c, a);
            return true;
        }
        if self.view == View::Credential {
            if a == action_id("sign-in") {
                let origin = self.url.split('/').take(3).collect::<Vec<_>>().join("/");
                if self.account.bind_server(&origin).is_ok() {
                    self.account.open();
                }
                return true;
            }
            if a == action_id("public-library") {
                self.credential = None;
                if self.settings.prepare(&self.url, None) {
                    self.fetch(c);
                }
                return true;
            }
        }
        if a == action_id("settings-save") {
            self.settings.retry(c);
            return true;
        }
        if a == action_id("settings-load") {
            self.loaded = false;
            self.settings.state = settings::State::Loading;
            c.store().load(REGISTRY);
            return true;
        }
        a == action_id("add") && !self.settings.can_edit()
    }
    fn catalog_action(&mut self, c: &mut Context, a: ActionId) {
        if self.view == View::Library && self.task.is_none() {
            if let Some(page) = &mut self.page {
                if a == action_id("page-back") {
                    page.offset = page.offset.saturating_sub(2);
                } else if a == action_id("page-next") && page.offset + 2 < page.count() {
                    page.offset += 2;
                } else if a == action_id("catalog-next") {
                    if let Some(link) = page.feed.next() {
                        let url = link.href.clone();
                        self.fetch_url(c, url);
                    }
                } else if let Some(index) = (page.offset..(page.offset + 2).min(page.count()))
                    .find(|i| a == action_id(&format!("entry-{i}")))
                {
                    if let Some(nav) = page.feed.navigation.get(index) {
                        let url = nav.href.clone();
                        self.fetch_url(c, url);
                    } else {
                        self.selected = index;
                        self.view = View::Book;
                    }
                }
            }
        }
    }
    fn screen(&self) -> Screen {
        if self.account.is_open() {
            return self.account.screen("Library account");
        }
        if self.reading.visible {
            return self.reading.screen();
        }
        match self.view {
            View::Libraries => {
                let mut s = ScreenBuilder::new("calibre-libraries")
                    .top_bar("Libraries")
                    .top_bar_action("add", "Add");
                if !self.loaded {
                    s = s.secondary("Loading libraries…");
                } else if self.settings.state == settings::State::Blocked {
                    s = s.text("Saved library settings could not be read. They have been left untouched.")
                        .button("settings-load", "Retry loading settings");
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
                s.button("offline", "Downloaded books").build()
            }
            View::Url => ScreenBuilder::new("calibre-url")
                .top_bar("Library address")
                .heading("Library address")
                .secondary(if self.notice.is_empty() {
                    "Enter the HTTPS OPDS address of your library."
                } else {
                    &self.notice
                })
                .typed(&self.keyboard, "https://library.example")
                .keyboard(&self.keyboard, "Continue")
                .owns_back(true)
                .build(),
            View::Credential => ScreenBuilder::new("calibre-account-choice")
                .top_bar("Library account")
                .owns_back(true)
                .text("Does this library require a username and password?")
                .primary_button("sign-in", "Sign in")
                .button("public-library", "Use public library")
                .build(),
            View::Library => self.library_screen(),
            View::Book => {
                let book = self.page.as_ref().and_then(|p| p.book(self.selected));
                let mut s = ScreenBuilder::new("calibre-book")
                    .top_bar("Book details")
                    .owns_back(true);
                if let Some(book) = book {
                    s = s.text(&book.title).secondary(book.authors.join(", "));
                    s = s.secondary(if book.best_acquisition().is_some() {
                        "EPUB or text edition available"
                    } else {
                        "No supported download in this catalog"
                    });
                }
                if book.is_some_and(|b| b.best_acquisition().is_some()) {
                    s = s.primary_button("download-book", "Download");
                }
                s.bottom_action("catalog-back", "Catalog").build()
            }
            View::Failure => ScreenBuilder::new("calibre-failure")
                .top_bar("Library")
                .owns_back(true)
                .heading("Cannot open library")
                .text(&self.notice)
                .button("retry", "Try again")
                .bottom_action("libraries", "Libraries")
                .build(),
        }
    }
}
impl KoboApp for Calibre {
    fn on_start(&mut self, c: &mut Context) {
        c.store().load(REGISTRY);
        self.reading.start(c);
        c.set_screen(self.screen());
    }
    fn on_load(&mut self, c: &mut Context, key: &str, result: StoreResult) {
        if key == REGISTRY && !matches!(result, StoreResult::Loaded { .. }) {
            self.loaded = true;
            self.settings.state = settings::State::Blocked;
            c.set_screen(self.screen());
        } else if key == books::KEY && !matches!(result, StoreResult::Loaded { .. }) {
            self.reading.load_failed();
            c.set_screen(self.screen());
        } else {
            self.on_store(c, result);
        }
    }
    fn on_device_result(
        &mut self,
        c: &mut Context,
        request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if self.account.on_device_result(&request, &result)
            == Some(kobo_sdk::credentials::CredentialEvent::Saved)
        {
            self.credential = Some("calibre".into());
            if self.settings.prepare(&self.url, self.credential.as_deref()) {
                self.fetch(c);
            }
        }
        c.set_screen(self.screen());
    }
    fn on_save(&mut self, c: &mut Context, key: &str, result: StoreResult) {
        if key == REGISTRY {
            self.settings.finish(&result);
            c.set_screen(self.screen());
        } else {
            self.on_store(c, result);
        }
    }
    fn on_store(&mut self, c: &mut Context, r: StoreResult) {
        if self.reading.store(c, &r) {
            c.set_screen(self.screen());
            return;
        }
        if let StoreResult::Loaded { key, value } = r {
            if key == REGISTRY {
                match value.as_deref().map(settings::decode) {
                    None => {
                        self.settings.state = settings::State::Ready;
                        self.url.clear();
                        self.credential = None;
                    }
                    Some(Some((url, account))) => {
                        self.url = url;
                        self.credential = account;
                        self.settings.state = settings::State::Ready;
                    }
                    Some(None) => self.settings.state = settings::State::Blocked,
                }
                self.loaded = true;
                c.set_screen(self.screen());
            }
        }
    }
    fn on_action(&mut self, c: &mut Context, a: ActionId) {
        if self.settings_action(c, a) {
            c.set_screen(self.screen());
            return;
        }
        if self.reading.visible {
            if a == action_id("offline-repair") {
                self.reading.repair(c, &self.url, self.credential.clone());
            } else {
                self.reading.action(c, a);
            }
            c.set_screen(self.screen());
            return;
        }
        if a == action_id("offline") {
            self.reading.visible = true;
            c.set_screen(self.screen());
            return;
        }
        if a == action_id("download-book") && self.view == View::Book {
            if let Some(book) = self.page.as_ref().and_then(|p| p.book(self.selected)) {
                self.reading
                    .download(c, book, &self.url, self.credential.clone());
            }
            c.set_screen(self.screen());
            return;
        }
        if a == ActionId::BACK || a == action_id("catalog-back") || a == action_id("cancel") {
            if let Some(task) = self.task.take() {
                c.cancel(task);
                self.settings.cancel_check();
            }
            if self.view == View::Book || self.view == View::Failure || a == action_id("cancel") {
                self.view = if self.page.is_some() {
                    View::Library
                } else {
                    View::Libraries
                };
            } else if self.view == View::Library {
                if let Some(page) = self.history.pop() {
                    self.page = Some(page);
                } else {
                    self.view = View::Libraries;
                }
            } else {
                self.view = View::Libraries;
            }
            c.set_screen(self.screen());
            return;
        }
        if a == action_id("retry") {
            let url = if self.pending_url.is_empty() {
                self.url.clone()
            } else {
                self.pending_url.clone()
            };
            self.fetch_url(c, url);
            c.set_screen(self.screen());
            return;
        }
        self.catalog_action(c, a);
        self.setup_or_library_action(c, a);
        c.set_screen(self.screen());
    }
    fn on_task(&mut self, c: &mut Context, id: TaskId, o: TaskOutcome) {
        if self.reading.task(c, id, &o) {
            c.set_screen(self.screen());
            return;
        }
        if self.task == Some(id) {
            self.task = None;
            match o {
                TaskOutcome::Completed(bytes) => {
                    match catalog::Page::parse(&bytes, &self.pending_url) {
                        Ok(page) => {
                            if let Some(previous) = self.page.take() {
                                if previous.url != page.url {
                                    if self.history.len() == catalog::HISTORY {
                                        self.history.remove(0);
                                    }
                                    self.history.push(previous);
                                }
                            }
                            self.page = Some(page);
                            self.settings.verified(c);
                            self.view = View::Library;
                        }
                        Err(message) => {
                            self.notice = message.into();
                            self.view = View::Failure;
                        }
                    }
                }
                TaskOutcome::Failed(error) => {
                    self.notice = catalog::failure(error);
                    self.view = View::Failure;
                }
                TaskOutcome::Cancelled => {
                    self.notice = "Request cancelled.".into();
                    self.view = View::Failure;
                }
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
    fn loaded_catalog() -> Calibre {
        Calibre {
            loaded: true,
            url: "https://library.test/opds".into(),
            view: View::Library,
            page: Some(
                catalog::Page::parse(
                    include_bytes!("../fixtures/root.xml"),
                    "https://library.test/opds",
                )
                .unwrap(),
            ),
            ..Calibre::default()
        }
    }
    #[test]
    fn catalog_taps_fetch_real_links_and_back_keeps_parent_position() {
        let mut app = loaded_catalog();
        let mut c = Context::default();
        app.on_action(&mut c, action_id("entry-0"));
        assert_eq!(app.pending_url, "https://library.test/opds/authors");
        let task = app.task.unwrap();
        app.on_task(
            &mut c,
            task,
            TaskOutcome::Completed(
                b"<feed xmlns=\"http://www.w3.org/2005/Atom\"><title>Authors</title></feed>"
                    .to_vec(),
            ),
        );
        assert_eq!(app.page.as_ref().unwrap().count(), 0);
        assert_eq!(app.history.len(), 1);
        app.on_action(&mut c, ActionId::BACK);
        assert_eq!(app.page.as_ref().unwrap().count(), 3);
        app.on_action(&mut c, action_id("page-next"));
        app.on_action(&mut c, action_id("entry-2"));
        assert!(app.view == View::Book);
        app.on_action(&mut c, ActionId::BACK);
        assert_eq!(app.page.as_ref().unwrap().offset, 2);
    }
    #[test]
    fn failures_preserve_catalog_and_cancelled_tasks_cannot_replace_it() {
        let mut app = loaded_catalog();
        let mut c = Context::default();
        app.on_action(&mut c, action_id("entry-0"));
        let task = app.task.unwrap();
        app.on_task(
            &mut c,
            task,
            TaskOutcome::Failed(kobo_sdk::TaskError::Unauthorized),
        );
        assert!(app.notice.contains("refused"));
        assert_eq!(app.page.as_ref().unwrap().count(), 3);
        app.on_action(&mut c, action_id("retry"));
        let task = app.task.unwrap();
        app.on_action(&mut c, action_id("cancel"));
        app.on_task(
            &mut c,
            task,
            TaskOutcome::Completed(b"<html>Login</html>".to_vec()),
        );
        assert!(app.view == View::Library);
        assert_eq!(app.page.as_ref().unwrap().count(), 3);
        app.fetch_url(&mut c, "https://different.test/catalog".into());
        assert!(app.task.is_none());
        assert!(app.view == View::Failure);
    }
    #[test]
    fn catalog_and_setup_fit_actual_fonts_at_all_text_scales() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            };
            let _runner = kobo_sdk::AppRunner::with_metrics(Calibre::default(), metrics);
            let mut app = loaded_catalog();
            for view in [
                View::Libraries,
                View::Url,
                View::Credential,
                View::Library,
                View::Failure,
                View::Book,
            ] {
                app.view = view;
                app.selected = 2;
                app.notice = catalog::failure(kobo_sdk::TaskError::Unauthorized);
                let diagnostics = app.screen().diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{text_scale:?}: {:?}",
                    diagnostics.issues
                );
            }
        }
    }
    #[test]
    fn setup_save_retry_and_unreadable_settings_guard() {
        let mut app = loaded_catalog();
        let mut c = Context::default();
        app.settings.state = settings::State::Ready;
        assert!(app.settings.prepare(&app.url, None));
        app.settings.verified(&mut c);
        app.on_save(
            &mut c,
            REGISTRY,
            StoreResult::Denied(kobo_sdk::StoreError::TooFull),
        );
        assert!(app.settings.state == settings::State::Failed);
        app.on_action(&mut c, action_id("settings-save"));
        assert!(app.settings.state == settings::State::Saving);
        app.on_save(
            &mut c,
            REGISTRY,
            StoreResult::Saved {
                key: REGISTRY.into(),
            },
        );
        assert!(app.settings.state == settings::State::Ready);
        app.on_load(
            &mut c,
            REGISTRY,
            StoreResult::Denied(kobo_sdk::StoreError::TooFull),
        );
        assert!(app.settings.state == settings::State::Blocked);
        app.on_action(&mut c, action_id("add"));
        assert!(app.view == View::Library);
    }
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
