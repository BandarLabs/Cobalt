//! Readeck on the public Cobalt SDK: server-bound token, inbox and article reader.
use kobo_bookview::{BookView, Step};
use kobo_json::Value;
use kobo_read::{Memory, Outcome};
use kobo_sdk::credentials::{CredentialEvent, CredentialSetup};
use kobo_sdk::failure::{Problem, Service};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, DeviceRequest, DeviceResult, FieldKind,
    Glyph, KoboApp, Position, RowLead, Screen, ScreenBuilder, StoreResult, Task, TaskId,
    TaskOutcome, UpdateMethod,
};
use kobo_ui::TextScale;
use std::fmt::Write as _;
use std::process::ExitCode;

const SERVER: &str = "server";
const SCALE: &str = "reading.scale";
/// The fix for every account problem, said where the owner will read it.
const ACCOUNT_FIX: &str = "Run kobo connect readeck on your computer.";
/// The largest article kept for reading offline. A store value holds at most
/// 256 KiB; an article past this is read online only rather than cut short.
const KEEP_LIMIT: usize = 240 * 1024;
const LIST_PATH: &str = "/api/bookmarks?limit=50&offset=0&is_archived=false&read_status=unread&read_status=reading&type=article&type=photo&sort=-created";
const LIMIT: usize = 50;
/// The connect flow, named on each of its screens: the server address, the
/// API token kept by the runtime, and the first request that checks both.
const STEPS: [&str; 3] = ["Address", "Token", "Check"];
/// Where a Readeck server makes API tokens, under the owner's own address.
const TOKEN_PAGE: &str = "/profile/tokens";
/// What a check of the store says when the computer has not written a server.
/// Checking again is the answer, so the banner that says it carries that
/// action.
const NO_SERVER_YET: &str = "No server has arrived from the computer yet.";

/// What the last check for a server written from the computer found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServerCheck {
    /// No check yet, or a fresh one under way.
    Unchecked,
    /// A check found no server; the banner says so and carries Check again.
    FoundNothing,
}

/// How much the first connect step says around its field, from the most to
/// the least; each shape leaves out one more thing than the one before.
/// Held sideways at the larger text sizes the sentence, then the field's
/// hint, then the step line give way, so the command, Check again, the field
/// and Use saved token stay on the panel. A refused address keeps its reason
/// under the field in every shape.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ServerShape {
    Full,
    WithoutSentence,
    WithoutHint,
    WithoutSteps,
}

/// The token flow. `connecting` puts it in its place among [`STEPS`]; signing
/// in again from the inbox is the token alone, so it carries none.
fn account(connecting: bool) -> CredentialSetup {
    let account = CredentialSetup::new("readeck", "Readeck API token")
        .with_computer_command("kobo connect readeck")
        .with_key_page(
            TOKEN_PAGE,
            "Opens the API token page on your Readeck server.",
        );
    if connecting {
        account.with_steps(1, STEPS)
    } else {
        account
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Bookmark {
    id: String,
    title: String,
    summary: String,
    minutes: u16,
    marked: bool,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Loading,
    Server,
    Inbox,
    Search,
    Reading,
    Controls,
    Delete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mutation {
    Read,
    Archive,
    Favorite(bool),
    Delete,
}
impl Mutation {
    fn body(self) -> &'static str {
        match self {
            Self::Read => r#"{"read_progress":100}"#,
            Self::Archive => r#"{"is_archived":true}"#,
            Self::Favorite(true) => r#"{"is_marked":true}"#,
            Self::Favorite(false) => r#"{"is_marked":false}"#,
            Self::Delete => r#"{"is_deleted":true}"#,
        }
    }
}
struct Search {
    query: String,
    offset: usize,
    more: bool,
    inbox: Vec<Bookmark>,
    page: usize,
}
struct ListRequest {
    task: TaskId,
    offset: usize,
    last_page: bool,
}
struct ArticleRequest {
    id: String,
    task: Option<TaskId>,
}

struct Readeck {
    server: Option<String>,
    pending_server: Option<String>,
    saving_server: bool,
    account: CredentialSetup,
    /// The server address being typed on the runtime's keyboard.
    address: TextEntry,
    /// The address shown in the field, kept between tries so a typing slip
    /// is mended rather than typed again.
    draft: String,
    /// Why the address in the field was refused, said under the field.
    address_error: Option<String>,
    search_entry: TextEntry,
    view: View,
    bookmarks: Vec<Bookmark>,
    page: usize,
    search: Option<Search>,
    list: Option<ListRequest>,
    article: Option<ArticleRequest>,
    mutation: Option<(TaskId, String, Mutation)>,
    open: Option<String>,
    memory: Option<Memory>,
    book: BookView,
    scale: TextScale,
    more_open: bool,
    /// What the last check for a server written from the computer found.
    server_check: ServerCheck,
    problem: Option<String>,
    /// Why the last refresh failed, shown over whatever inbox is on screen.
    failure: Option<Problem>,
    /// A reload of the server address asked for on coming back to the
    /// front, which must not pull the owner out of an article.
    reloading: bool,
}
impl Default for Readeck {
    fn default() -> Self {
        Self {
            server: None,
            pending_server: None,
            saving_server: false,
            account: account(true),
            address: TextEntry::new().address(),
            draft: String::new(),
            address_error: None,
            search_entry: TextEntry::new(),
            view: View::Loading,
            bookmarks: Vec::new(),
            page: 0,
            search: None,
            list: None,
            article: None,
            mutation: None,
            open: None,
            memory: None,
            book: BookView::new(),
            scale: TextScale::Medium,
            more_open: false,
            server_check: ServerCheck::Unchecked,
            problem: None,
            failure: None,
            reloading: false,
        }
    }
}
impl Readeck {
    fn service(&self) -> Service<'_> {
        let service = Service::new("Readeck").fixed_by(ACCOUNT_FIX);
        match &self.server {
            Some(server) => service.at(server),
            None => service,
        }
    }
    /// Asks for the server address again, as `kobo connect readeck` may
    /// have changed it while the app was away.
    fn reload_server(&mut self, context: &mut Context) {
        self.reloading = true;
        context.store().load(SERVER);
    }
    fn sign_in_again(&mut self) {
        if let Some(server) = self.server.clone() {
            self.account = account(false);
            if self.account.bind_server(&server).is_ok() {
                self.pending_server = Some(server);
                self.account.open();
            }
        }
    }
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context));
    }
    fn current(&self) -> Option<&Bookmark> {
        self.bookmarks
            .iter()
            .find(|entry| Some(&entry.id) == self.open.as_ref())
    }
    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let minutes: Vec<_> = self
            .bookmarks
            .iter()
            .map(|b| minute_label(b.minutes))
            .collect();
        let rows: Vec<_> = self
            .bookmarks
            .iter()
            .zip(&minutes)
            .map(|(b, minutes)| (b.title.as_str(), b.summary.as_str(), minutes.as_str()))
            .collect();
        if let Some(failure) = &self.failure {
            // Under a banner the rows give up their minutes, so they can be
            // measured under the banner as it is drawn, with its mark and its
            // action, rather than as plain words.
            let rows: Vec<_> = rows
                .iter()
                .map(|(title, summary, _)| (*title, *summary))
                .collect();
            let banner = ScreenBuilder::new("readeck-measure")
                .top_bar("Readeck")
                .problem_banner_with(failure, "refresh", Some("sign-in-again"))
                .build();
            return context.paginate_rows_under(&rows, true, Position::AtTheFoot, &banner);
        }
        context.paginate_rows_with_trailing(&rows, true)
    }
    fn screen(&self, context: &Context) -> Screen {
        if self.account.is_open() {
            return self.account.screen("Readeck");
        }
        if let Some(problem) = &self.problem {
            return ScreenBuilder::new("readeck-problem")
                .top_bar("Readeck")
                .banner(BannerLevel::Attention, problem)
                .button("dismiss", "Continue")
                .owns_back(true)
                .build();
        }
        if self.saving_server {
            return ScreenBuilder::new("readeck-saving")
                .top_bar("Readeck")
                .steps(2, 3, STEPS)
                .activity("Saving server address", None)
                .owns_back(true)
                .build();
        }
        if self.pending_server.is_some() && !self.account.is_open() {
            return ScreenBuilder::new("readeck-save-retry")
                .top_bar("Readeck")
                .text("The server address still needs saving.")
                .button("save-server", "Retry saving address")
                .owns_back(true)
                .build();
        }
        match self.view {
            View::Loading => ScreenBuilder::new("readeck-loading")
                .top_bar("Readeck")
                .activity("Opening settings", None)
                .build(),
            View::Server => self.server_screen(context),
            View::Search => ScreenBuilder::new("readeck-search")
                .top_bar("Search library")
                .text_entry_with_hint(
                    &self.search_entry,
                    "Search terms",
                    "Search includes read and archived articles.",
                    "Search",
                )
                .owns_back(true)
                .build(),
            View::Reading => {
                let mut screen = self
                    .book
                    .screen(self.current().map_or("Readeck", |b| &b.title))
                    .unwrap_or_else(|| {
                        ScreenBuilder::new("readeck-empty-reader")
                            .top_bar("Readeck")
                            .build()
                    })
                    .with_own_back(true);
                // Do not offer local marks that could be mistaken for Readeck annotations.
                screen.hold = None;
                screen
            }
            View::Controls => self.controls(),
            View::Delete => ScreenBuilder::new("readeck-delete")
                .top_bar("Readeck")
                .confirm(
                    "Delete this article?",
                    "This removes it from your Readeck library.",
                    ("confirm-delete", "Delete article"),
                    ("keep-article", "Keep article"),
                )
                .owns_back(true)
                .build(),
            View::Inbox => self.inbox_screen(context),
        }
    }
    /// The first step of connecting: where the server is. The runtime draws
    /// the keys; this screen is the computer route, the field, and the way
    /// to keep a token that is already on the reader.
    ///
    /// Laid out as every connect screen is: a banner first, then the
    /// command, then Check again directly under it. Once a check has found
    /// nothing, Check again moves onto the banner that says so, rather than
    /// sitting in the top bar where it is read last.
    fn server_screen(&self, context: &Context) -> Screen {
        if self.address.is_open() {
            return ScreenBuilder::new("readeck-server")
                .top_bar("Connect Readeck")
                .owns_back(true)
                .text_entry(&self.address, "Readeck server address", "Next")
                .build();
        }
        let build = |shape: ServerShape| {
            let mut screen = ScreenBuilder::new("readeck-server").top_bar("Connect Readeck");
            if self.server_check == ServerCheck::FoundNothing {
                screen = screen
                    .banner(BannerLevel::Attention, NO_SERVER_YET)
                    .banner_action("check-again", "Check again");
            }
            if shape < ServerShape::WithoutSteps {
                screen = screen.steps(0, 3, STEPS);
            }
            if shape == ServerShape::Full {
                screen = screen.secondary("Run this on your computer, or type your server below.");
            }
            screen = screen.command("kobo connect readeck");
            if self.server_check == ServerCheck::Unchecked {
                screen = screen.primary_button("check-again", "Check again");
            }
            screen = screen
                .field("server-address", &self.draft, "https://readeck.example")
                .field_label("Server address")
                .field_kind(FieldKind::Address);
            if let Some(error) = &self.address_error {
                screen = screen.field_error(error);
            } else if shape < ServerShape::WithoutHint {
                screen =
                    screen.field_hint("The address you open Readeck at, starting with https://");
            }
            screen
                .button("saved-token", "Use saved token")
                .owns_back(true)
                .build()
        };
        let metrics = context.metrics();
        [
            ServerShape::Full,
            ServerShape::WithoutSentence,
            ServerShape::WithoutHint,
        ]
        .into_iter()
        .map(build)
        .find(|screen| {
            !screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        })
        .unwrap_or_else(|| build(ServerShape::WithoutSteps))
    }
    fn controls(&self) -> Screen {
        if self.mutation.is_some() {
            return ScreenBuilder::new("readeck-updating")
                .top_bar("Readeck")
                .activity("Waiting for Readeck", None)
                .owns_back(true)
                .build();
        }
        let marked = self.current().is_some_and(|b| b.marked);
        ScreenBuilder::new("readeck-controls")
            .top_bar("Article controls")
            .button("mark-read", "Mark read")
            .button("archive", "Archive")
            .button("favorite", if marked { "Unfavorite" } else { "Favorite" })
            .button("delete", "Delete article")
            .action_bar([
                ("reader-smaller", "Smaller text"),
                ("reader-larger", "Larger text"),
            ])
            .action_bar([("reader-dimmer", "Dimmer"), ("reader-brighter", "Brighter")])
            .button("return-article", "Return to article")
            .owns_back(true)
            .build()
    }
    fn inbox_screen(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("readeck").top_bar(
            self.search
                .as_ref()
                .map_or_else(|| "Readeck".into(), |s| format!("Search: {}", s.query)),
        );
        // Search spends the same refused account, so it is left out while the
        // account is the problem; the banner over the inbox carries the fix.
        if !self.failure.as_ref().is_some_and(Problem::needs_account) {
            screen = screen.top_bar_action("search", "Search");
        }
        let mut screen = screen.top_bar_overflow(
            "more",
            self.more_open,
            [
                ("refresh", "Refresh"),
                if self.search.is_some() {
                    ("inbox", "Inbox")
                } else {
                    ("settings", "Account")
                },
            ],
        );
        if self.list.is_some() || self.article.is_some() {
            return screen
                .activity("Loading articles", None)
                .owns_back(true)
                .build();
        }
        if self.bookmarks.is_empty() {
            if let Some(failure) = &self.failure {
                // A failed refresh with nothing kept is the failure, not an
                // empty inbox. Saying "empty" sent owners looking for
                // articles that were there all along.
                screen = screen.problem_state(failure, "refresh");
                if failure.needs_account() {
                    screen = screen.button("sign-in-again", "Sign in again");
                }
                return screen.owns_back(self.search.is_some()).build();
            }
            screen = screen.empty_state(if self.search.is_some() {
                "No readable matches in this batch. Turn back or forward to try another batch."
            } else {
                "Your unread Readeck inbox is empty."
            });
        } else {
            let pages = self.pages(context);
            let page = self.page.min(pages.len().saturating_sub(1));
            let lead = |b: &Bookmark| {
                RowLead::Icon(if b.marked {
                    Glyph::Bookmark
                } else {
                    Glyph::News
                })
            };
            if let Some(failure) = &self.failure {
                screen = screen
                    .problem_banner_with(failure, "refresh", Some("sign-in-again"))
                    .rows(pages.get(page).into_iter().flatten().map(|index| {
                        let b = &self.bookmarks[*index];
                        (
                            format!("article-{index}"),
                            b.title.clone(),
                            b.summary.clone(),
                            lead(b),
                        )
                    }));
            } else {
                screen =
                    screen.rows_with_trailing(pages.get(page).into_iter().flatten().map(|index| {
                        let b = &self.bookmarks[*index];
                        (
                            format!("article-{index}"),
                            b.title.clone(),
                            b.summary.clone(),
                            RowLead::Icon(if b.marked {
                                Glyph::Bookmark
                            } else {
                                Glyph::News
                            }),
                            minute_label(b.minutes),
                        )
                    }));
            }
            if self.search.is_none() {
                screen = screen.page_position(
                    page_number(page),
                    page_number(pages.len().saturating_sub(1)),
                );
            }
        }
        screen
            .page_turns("previous", "next")
            .owns_back(self.search.is_some() || self.more_open)
            .build()
    }
    fn fetch_list(&mut self, context: &mut Context, offset: usize, last_page: bool) {
        let Some(server) = &self.server else { return };
        let path = self
            .search
            .as_ref()
            .map_or_else(|| LIST_PATH.into(), |s| search_path(&s.query, offset));
        self.list = context
            .spawn_retrying(fetch(format!("{server}{path}"), 512 * 1024))
            .map(|task| ListRequest {
                task,
                offset,
                last_page,
            });
        if self.list.is_none() {
            self.problem = Some("The device is busy. Try Refresh again.".into());
        }
    }
    fn cancel_requests(&mut self, context: &mut Context) {
        if let Some(request) = self.list.take() {
            context.cancel(request.task);
        }
        if let Some(task) = self.article.take().and_then(|a| a.task) {
            context.cancel(task);
        }
        self.memory = None;
    }
    fn edit_server(&mut self, context: &mut Context) {
        self.cancel_requests(context);
        self.draft = self.server.clone().unwrap_or_default();
        self.address_error = None;
        self.more_open = false;
        self.server_check = ServerCheck::Unchecked;
        self.view = View::Server;
    }
    fn submit_server(&mut self) {
        self.prepare_server(true);
    }
    fn prepare_server(&mut self, enter_token: bool) {
        let server = self.draft.trim().trim_end_matches('/').to_owned();
        // Validate exactly the origin/base-path shape enforced by runtime storage.
        if !valid_server(&server) {
            self.address_error = Some("Enter an HTTPS server without a query, fragment, credentials or relative path segments.".into());
            return;
        }
        self.account = account(true);
        if let Err(problem) = self.account.bind_server(&server) {
            self.address_error = Some(problem);
            return;
        }
        self.address_error = None;
        self.pending_server = Some(server);
        if enter_token {
            self.account.open();
        }
    }
    fn save_server(&mut self, context: &mut Context) {
        if let Some(server) = &self.pending_server {
            context.store().save(SERVER, server.as_bytes().to_vec());
            self.saving_server = true;
        }
    }
    fn start_search(&mut self, context: &mut Context, query: &str) {
        let query = query.trim().to_owned();
        if query.is_empty() || query.len() > 256 || query.chars().any(char::is_control) {
            self.problem = Some("Enter a search of 1–256 bytes without control characters.".into());
            return;
        }
        self.cancel_requests(context);
        if let Some(search) = &mut self.search {
            search.query = query;
        } else {
            self.search = Some(Search {
                query,
                offset: 0,
                more: false,
                inbox: std::mem::take(&mut self.bookmarks),
                page: self.page,
            });
        }
        self.page = 0;
        self.view = View::Inbox;
        self.fetch_list(context, 0, false);
    }
    /// Back to the inbox from a screen that typed, loading it if nothing is
    /// on it yet.
    fn leave_typing(&mut self, context: &mut Context) {
        self.view = View::Inbox;
        if self.bookmarks.is_empty() {
            self.fetch_list(context, self.search.as_ref().map_or(0, |s| s.offset), false);
        }
    }
    fn leave_search(&mut self, context: &mut Context) {
        self.cancel_requests(context);
        if let Some(search) = self.search.take() {
            self.bookmarks = search.inbox;
            self.page = search.page;
        }
        self.view = View::Inbox;
        self.fetch_list(context, 0, false);
    }
    fn open_article(&mut self, context: &mut Context, index: usize) {
        let Some(entry) = self.bookmarks.get(index) else {
            return;
        };
        let Some(server) = &self.server else { return };
        self.open = Some(entry.id.clone());
        self.article = Some(ArticleRequest {
            id: entry.id.clone(),
            task: None,
        });
        // Fetch only after the place has loaded; a delayed load cannot rewind a turn.
        context.store().load(place_key(server, &entry.id));
    }
    fn remember(&mut self, context: &mut Context) {
        if let Some((id, memory)) = self.open.as_ref().zip(self.book.memory()) {
            if let Some(server) = &self.server {
                context
                    .store()
                    .save(place_key(server, id), encode_place(server, memory));
            }
            self.scale = memory.scale;
            context.store().save(SCALE, vec![self.scale.wire_value()]);
        }
    }
    fn close_article(&mut self, context: &mut Context) {
        self.remember(context);
        self.book.close(context);
        self.open = None;
        self.view = View::Inbox;
    }
    fn mutate(&mut self, context: &mut Context, mutation: Mutation) {
        let Some((server, id)) = self.server.as_ref().zip(self.open.as_ref()) else {
            return;
        };
        let id = id.clone();
        let task = context.spawn(Task::Update {
            method: UpdateMethod::Patch,
            url: format!("{server}/api/bookmarks/{id}"),
            body: mutation.body().into(),
            content_type: "application/json".into(),
            credential: Some(Credential::bearer("readeck")),
            headers: Vec::new(),
            max_bytes: 64 * 1024,
        });
        self.mutation = task.map(|task| (task, id, mutation));
        self.view = View::Controls;
        if self.mutation.is_none() {
            self.problem = Some("The device is busy. Try again.".into());
        }
        self.remember(context);
    }
    fn applied(&mut self, context: &mut Context, id: &str, mutation: Mutation) {
        if let Some(search) = &mut self.search {
            apply_to_list(&mut search.inbox, id, mutation);
        }
        if matches!(mutation, Mutation::Favorite(_))
            || (self.search.is_some() && mutation != Mutation::Delete)
        {
            if let Mutation::Favorite(marked) = mutation {
                if let Some(entry) = self.bookmarks.iter_mut().find(|b| b.id == id) {
                    entry.marked = marked;
                }
            }
            self.view = View::Reading;
        } else {
            self.close_article(context);
            apply_to_list(&mut self.bookmarks, id, mutation);
            self.page = self.page.min(self.pages(context).len().saturating_sub(1));
        }
    }
    fn reader_action(&mut self, context: &mut Context, action: ActionId) {
        if self.mutation.is_some() {
            return;
        }
        if self.view == View::Delete {
            if action == action_id("confirm-delete") {
                self.mutate(context, Mutation::Delete);
            } else if action == action_id("keep-article") || action == ActionId::BACK {
                self.view = View::Controls;
            }
            return;
        }
        if self.view == View::Reading && action == action_id(kobo_read::action::CONTROLS) {
            self.view = View::Controls;
            return;
        }
        if self.view == View::Controls {
            if action == ActionId::BACK || action == action_id("return-article") {
                self.view = View::Reading;
                return;
            }
            if action == action_id("delete") {
                self.view = View::Delete;
                return;
            }
            let mutation = if action == action_id("mark-read") {
                Some(Mutation::Read)
            } else if action == action_id("archive") {
                Some(Mutation::Archive)
            } else if action == action_id("favorite") {
                Some(Mutation::Favorite(
                    !self.current().is_some_and(|b| b.marked),
                ))
            } else {
                None
            };
            if let Some(mutation) = mutation {
                self.mutate(context, mutation);
                return;
            }
        }
        if action == ActionId::BACK {
            self.close_article(context);
            return;
        }
        if let Some(outcome) = self.book.act(context, action) {
            match outcome {
                Outcome::Close => self.close_article(context),
                Outcome::Save => self.remember(context),
                Outcome::Light(level) => context.device().set_frontlight(level),
                Outcome::Elsewhere | Outcome::Repaint => {}
            }
        }
    }
    fn inbox_action(&mut self, context: &mut Context, action: ActionId) {
        if self.more_open {
            self.more_open = false;
            if action == ActionId::BACK || action == action_id("more") {
                return;
            }
        }
        if action == ActionId::BACK || action == action_id("inbox") {
            if self.search.is_some() {
                self.leave_search(context);
            } else {
                self.cancel_requests(context);
            }
        } else if action == action_id("search") {
            self.cancel_requests(context);
            self.search_entry.open_with(
                self.search
                    .as_ref()
                    .map_or_else(String::new, |s| s.query.clone()),
            );
            self.view = View::Search;
        } else if action == action_id("settings") {
            self.edit_server(context);
        } else if action == action_id("sign-in-again") {
            self.sign_in_again();
        } else if self.list.is_some() || self.article.is_some() { /* Ignore repeated taps. */
        } else if action == action_id("more") {
            self.more_open = true;
        } else if action == action_id("refresh") {
            self.fetch_list(context, self.search.as_ref().map_or(0, |s| s.offset), false);
        } else if action == action_id("previous") {
            if self.page > 0 {
                self.page -= 1;
            } else if let Some(s) = self.search.as_ref().filter(|s| s.offset >= LIMIT) {
                self.fetch_list(context, s.offset - LIMIT, true);
            }
        } else if action == action_id("next") {
            if self.page + 1 < self.pages(context).len() {
                self.page += 1;
            } else if let Some(offset) = self
                .search
                .as_ref()
                .filter(|s| s.more)
                .and_then(|s| s.offset.checked_add(LIMIT))
                .filter(|n| u32::try_from(*n).is_ok())
            {
                self.fetch_list(context, offset, false);
            }
        } else if let Some(index) =
            (0..self.bookmarks.len()).find(|i| action == action_id(&format!("article-{i}")))
        {
            self.open_article(context, index);
        }
    }
}
impl KoboApp for Readeck {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SERVER);
        context.store().load(SCALE);
        self.show(context);
    }
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.account.is_open() {
            if self.account.on_action(context, action) == Some(CredentialEvent::Cancelled) {
                self.pending_server = None;
            }
        } else if self.problem.is_some() {
            if action == ActionId::BACK || action == action_id("dismiss") {
                self.problem = None;
            }
        } else if self.saving_server {
            return;
        } else if self.pending_server.is_some() {
            if action == action_id("save-server") {
                self.save_server(context);
            }
        } else if self.view == View::Search {
            match self.search_entry.handle(action) {
                Some(Typing::Submitted(query)) => self.start_search(context, &query),
                // Backed out of the runtime's field, or Back before it opened.
                typing
                    if typing == Some(Typing::Cancelled)
                        || (typing.is_none() && action == ActionId::BACK) =>
                {
                    self.leave_typing(context);
                }
                _ => {}
            }
        } else if self.view == View::Server {
            if let Some(typing) = self.address.handle(action) {
                if let Typing::Submitted(address) = typing {
                    self.draft = address;
                    self.submit_server();
                }
            } else if action == ActionId::BACK {
                if self.server.is_some() {
                    self.leave_typing(context);
                }
            } else if action == action_id("server-address") {
                self.address.open_with(&self.draft);
            } else if action == action_id("check-again") {
                self.server_check = ServerCheck::Unchecked;
                self.reload_server(context);
            } else if action == action_id("saved-token") {
                self.prepare_server(false);
                if self.address_error.is_none() {
                    self.save_server(context);
                }
            }
        } else if matches!(self.view, View::Reading | View::Controls | View::Delete) {
            self.reader_action(context, action);
        } else if self.view == View::Inbox {
            self.inbox_action(context, action);
        }
        self.show(context);
    }
    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        match self.account.on_device_result(&request, &result) {
            Some(CredentialEvent::Saved) => self.save_server(context),
            // The runtime cannot say whether a server-bound account is there;
            // a refresh with it can, and its failure says what is missing.
            Some(CredentialEvent::TryAccount) => {
                if self.pending_server.is_some() {
                    self.save_server(context);
                } else {
                    self.fetch_list(context, 0, false);
                }
            }
            _ => {}
        }
        self.show(context);
    }
    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key.starts_with(kobo_sdk::CACHE_PREFIX) && !matches!(result, StoreResult::Loaded { .. })
        {
            // A kept copy that cannot be read is only a missed shortcut.
            self.on_load(
                context,
                key,
                StoreResult::Loaded {
                    key: key.to_owned(),
                    value: None,
                },
            );
            return;
        }
        let StoreResult::Loaded { value, .. } = result else {
            self.problem = Some("Saved settings or reading position could not be read. Reopen Readeck to retry; nothing was overwritten.".into());
            self.show(context);
            return;
        };
        if key == SERVER {
            let loaded = value
                .as_deref()
                .and_then(|b| std::str::from_utf8(b).ok())
                .filter(|s| valid_server(s))
                .map(str::to_owned);
            if std::mem::take(&mut self.reloading) {
                self.reloaded(context, loaded);
                self.show(context);
                return;
            }
            self.server = loaded;
            if self.server.is_some() {
                if let Some(server) = &self.server {
                    context.store().load_cached(inbox_key(server));
                }
                self.view = View::Inbox;
                self.fetch_list(context, 0, false);
            } else {
                self.view = View::Server;
                if value.is_some() {
                    self.problem = Some(
                        "The saved server address could not be read. Enter it again to reconnect."
                            .into(),
                    );
                }
            }
        } else if key == SCALE {
            self.scale = value
                .as_deref()
                .and_then(|b| b.first())
                .and_then(|b| TextScale::from_wire(*b))
                .unwrap_or(TextScale::Medium);
        } else if self
            .server
            .as_ref()
            .is_some_and(|server| key == kobo_sdk::cache_key(inbox_key(server)))
        {
            // The last inbox seen, so a reader away from Wi-Fi opens on its
            // articles rather than on an error with nothing under it.
            if self.bookmarks.is_empty() && self.search.is_none() {
                if let Some((bookmarks, _)) = value.as_deref().and_then(parse_bookmarks) {
                    self.bookmarks = bookmarks;
                }
            }
        } else if let Some((server, article)) = self.server.clone().zip(self.article.as_ref()) {
            let id = article.id.clone();
            if article.task.is_some() {
            } else if key == place_key(&server, &id) {
                let mut memory = value
                    .as_deref()
                    .and_then(|b| decode_place(&server, b))
                    .unwrap_or_default();
                memory.scale = self.scale;
                self.memory = Some(memory);
                // The kept copy first: an article opened once reads the same
                // with the radio off, and saves a 4 MB fetch with it on.
                context.store().load_cached(kept_key(&server, &id));
            } else if key == kobo_sdk::cache_key(kept_key(&server, &id)) {
                if let Some(html) = value
                    .as_deref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                {
                    self.article = None;
                    let html = html.to_owned();
                    self.open_kept(context, &id, &html);
                } else {
                    let task = context.spawn_retrying(fetch(
                        format!("{server}/api/bookmarks/{id}/article"),
                        4 * 1024 * 1024,
                    ));
                    if task.is_none() {
                        self.article = None;
                        self.problem = Some("The device is busy. Open the article again.".into());
                    } else if let Some(article) = self.article.as_mut() {
                        article.task = task;
                    }
                }
            }
        }
        self.show(context);
    }
    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == SERVER && self.saving_server {
            self.saving_server = false;
            if matches!(result, StoreResult::Saved { .. }) {
                self.server = self.pending_server.take();
                self.bookmarks.clear();
                self.search = None;
                self.page = 0;
                self.open = None;
                self.view = View::Inbox;
                self.fetch_list(context, 0, false);
            } else {
                self.problem =
                    Some("The server address could not be saved. Continue to retry.".into());
            }
        } else if !matches!(result, StoreResult::Saved { .. })
            && !key.starts_with(kobo_sdk::CACHE_PREFIX)
        {
            self.problem = Some("Reading position or text size could not be saved. Turn a page to retry before closing.".into());
        }
        self.show(context);
    }
    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.list.as_ref().is_some_and(|r| r.task == task) {
            let request = self.list.take().expect("matched request");
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if let Some((bookmarks, count)) = parse_bookmarks(&bytes) {
                        self.failure = None;
                        if let Some(server) =
                            self.server.as_deref().filter(|_| self.search.is_none())
                        {
                            if bytes.len() <= KEEP_LIMIT {
                                context.store().cache(inbox_key(server), bytes);
                            }
                        }
                        self.bookmarks = bookmarks;
                        if let Some(search) = &mut self.search {
                            search.offset = request.offset;
                            search.more = count == LIMIT;
                        }
                        self.page = if request.last_page {
                            self.pages(context).len().saturating_sub(1)
                        } else {
                            0
                        };
                    } else {
                        self.problem =
                            Some("Readeck returned an unreadable list. Refresh to retry.".into());
                    }
                }
                TaskOutcome::Failed(error) => {
                    self.failure = Some(self.service().explain(error));
                }
                TaskOutcome::Cancelled => {
                    self.problem = Some("Loading was interrupted. Refresh to retry.".into());
                }
            }
        } else if self.article.as_ref().is_some_and(|r| r.task == Some(task)) {
            let article = self.article.take().expect("matched article");
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    let memory = self.memory.take().unwrap_or_default();
                    let origin = format!(
                        "{}/api/bookmarks/{}/article",
                        self.server.as_deref().unwrap_or_default(),
                        article.id
                    );
                    if std::str::from_utf8(&bytes)
                        .is_ok_and(|html| self.book.open_html(context, html, &origin, memory))
                    {
                        self.view = View::Reading;
                        if let Some(server) = self.server.as_deref() {
                            if bytes.len() <= KEEP_LIMIT {
                                context.store().cache(kept_key(server, &article.id), bytes);
                            }
                        }
                    } else {
                        self.problem = Some("This article could not be opened.".into());
                    }
                }
                TaskOutcome::Failed(error) => {
                    self.problem = Some(format!(
                        "This article is not kept on the reader yet. {}",
                        self.service().explain(error).message
                    ));
                }
                TaskOutcome::Cancelled => {
                    self.problem = Some("Opening was interrupted. Try again.".into());
                }
            }
        } else if self.mutation.as_ref().is_some_and(|r| r.0 == task) {
            let (_, id, mutation) = self.mutation.take().expect("matched mutation");
            match outcome {
                TaskOutcome::Completed(_) => self.applied(context, &id, mutation),
                TaskOutcome::Failed(error) => {
                    self.problem = Some(self.service().explain(error).message);
                }
                TaskOutcome::Cancelled => {
                    self.problem =
                        Some("The change was not confirmed. Check Readeck before retrying.".into());
                }
            }
        } else if self.book.woke(context, task, &outcome) != Step::Repaint {
            return;
        }
        self.show(context);
    }
    fn on_exit(&mut self, context: &mut Context) {
        self.remember(context);
    }
    fn on_foreground(&mut self, context: &mut Context) {
        self.returned(context);
    }
    fn on_resume(&mut self, context: &mut Context) {
        self.returned(context);
    }
}
impl Readeck {
    /// Coming back to the front is when a fix made on the computer arrives:
    /// a server written by `kobo connect readeck`, or a token replaced.
    fn returned(&mut self, context: &mut Context) {
        self.account.recheck(context);
        if !self.saving_server && self.pending_server.is_none() {
            self.reload_server(context);
        }
    }
    /// The server address as it is now, against the one in use.
    fn reloaded(&mut self, context: &mut Context, loaded: Option<String>) {
        match loaded {
            Some(server) if self.server.as_ref() != Some(&server) => {
                self.server_check = ServerCheck::Unchecked;
                self.cancel_requests(context);
                self.account.close();
                self.server = Some(server);
                self.bookmarks.clear();
                self.search = None;
                self.page = 0;
                self.open = None;
                self.failure = None;
                self.view = View::Inbox;
                if let Some(server) = &self.server {
                    context.store().load_cached(inbox_key(server));
                }
                self.fetch_list(context, 0, false);
            }
            Some(_)
                if self.view == View::Inbox && self.failure.is_some() && self.list.is_none() =>
            {
                self.fetch_list(context, 0, false);
            }
            None if self.view == View::Server => self.server_check = ServerCheck::FoundNothing,
            Some(_) | None => {}
        }
    }
    fn open_kept(&mut self, context: &mut Context, id: &str, html: &str) {
        let memory = self.memory.take().unwrap_or_default();
        let origin = format!(
            "{}/api/bookmarks/{id}/article",
            self.server.as_deref().unwrap_or_default()
        );
        if self.book.open_html(context, html, &origin, memory) {
            self.view = View::Reading;
        } else {
            self.problem = Some("This article could not be opened.".into());
        }
    }
}
/// Where the last inbox list from `server` is kept, so the app opens on it
/// without a network.
fn inbox_key(server: &str) -> String {
    let place = place_key(server, "");
    format!("inbox{}", place[1..].trim_end_matches('.'))
}
/// Where an opened article is kept for reading offline. The runtime drops
/// the oldest kept copies on its own when the cache is full, so the reader
/// keeps the most recent articles without the app counting them.
fn kept_key(server: &str, id: &str) -> String {
    let place = place_key(server, id);
    format!("a{}", &place[1..])
}
fn fetch(url: String, max_bytes: u32) -> Task {
    Task::Fetch {
        url,
        offset: 0,
        max_bytes,
        credential: Some(Credential::bearer("readeck")),
        headers: Vec::new(),
    }
}
fn valid_server(server: &str) -> bool {
    // The network parser defaults an invalid explicit port to 443. Do not
    // save an address whose written port differs from the connection target.
    let authority = server
        .strip_prefix("https://")
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("");
    let valid_port = authority.ends_with(']')
        || authority
            .rsplit_once(':')
            .is_none_or(|(_, port)| port.parse::<u16>().is_ok());
    valid_port
        && kobo_protocol::valid_secret_server(server)
        && kobo_net::parse(server).is_ok()
        && !server.contains('%')
        && !server.contains('\\')
        && !server.split('/').any(|s| matches!(s, "." | ".."))
}
fn search_path(query: &str, offset: usize) -> String {
    let mut encoded = String::new();
    for byte in query.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("/api/bookmarks?limit=50&offset={offset}&sort=-created&search={encoded}")
}
fn parse_bookmarks(bytes: &[u8]) -> Option<(Vec<Bookmark>, usize)> {
    let value = kobo_json::parse(std::str::from_utf8(bytes).ok()?).ok()?;
    let list = value.as_array().filter(|list| list.len() <= LIMIT)?;
    Some((list.iter().filter_map(parse_bookmark).collect(), list.len()))
}
fn parse_bookmark(value: &Value) -> Option<Bookmark> {
    let id = value.get("id")?.as_str()?;
    if !(18..=22).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let title = clean(value.get("title")?.as_str()?);
    if title.is_empty()
        || !value
            .get("has_article")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return None;
    }
    let site = value
        .get("site_name")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| value.get("site").and_then(Value::as_str))
        .map(clean)
        .unwrap_or_default();
    let authors = value
        .get("authors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(clean)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    Some(Bookmark {
        id: id.into(),
        title,
        summary: [site, authors]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        minutes: value
            .get("reading_time")
            .and_then(Value::as_i64)
            .and_then(|n| u16::try_from(n).ok())
            .unwrap_or(0),
        marked: value
            .get("is_marked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}
fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn minute_label(minutes: u16) -> String {
    if minutes == 0 {
        String::new()
    } else {
        format!("{minutes} min")
    }
}
fn page_number(index: usize) -> u16 {
    u16::try_from(index.saturating_add(1)).unwrap_or(u16::MAX)
}
fn apply_to_list(list: &mut Vec<Bookmark>, id: &str, mutation: Mutation) {
    if let Mutation::Favorite(marked) = mutation {
        if let Some(b) = list.iter_mut().find(|b| b.id == id) {
            b.marked = marked;
        }
    } else {
        list.retain(|b| b.id != id);
    }
}
// Server-hashed key plus an exact server inside the value: even a hash collision
// cannot restore a position from another instance. IDs remain case-sensitive.
fn place_key(server: &str, id: &str) -> String {
    let hash = server.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    let mut key = format!("p.{hash:016x}.");
    for byte in id.bytes() {
        let _ = write!(key, "{byte:02x}");
    }
    key
}
fn encode_place(server: &str, memory: &Memory) -> Vec<u8> {
    let mut bytes = format!("readeck-place-v1\n{server}\n").into_bytes();
    bytes.extend(memory.encode());
    bytes
}
fn decode_place(server: &str, bytes: &[u8]) -> Option<Memory> {
    bytes
        .strip_prefix(format!("readeck-place-v1\n{server}\n").as_bytes())
        .map(Memory::decode)
}
fn main() -> ExitCode {
    match kobo_sdk::run("readeck", Readeck::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("readeck: {error}");
            ExitCode::FAILURE
        }
    }
}
#[cfg(test)]
mod tests;
