//! Wikipedia, on a panel with no scrollbar and no browser.
//!
//! Three screens: search, a list of what it found, and one article read as
//! plain prose. A fourth verb, Random, skips straight to an article nobody
//! chose, which on a device this quiet is the whole reason to pick it up
//! between the things somebody meant to look up.

mod api;

use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Failure, Glyph, KoboApp, QuoteRole, Screen,
    ScreenBuilder, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

/// How much of a search answer to accept. Twenty titles and snippets is a
/// few kilobytes; this is generous headroom over it.
const SEARCH_BYTES: u32 = 32 * 1024;

/// How much of a random-article answer to accept. One title.
const RANDOM_BYTES: u32 = 4 * 1024;

/// How much of an article's plain-text body to accept.
///
/// A long article (a country, a war, a century) runs past a hundred
/// kilobytes of prose alone. Cut short by the runtime's own ceiling rather
/// than refused, which the reading screen says plainly when it happens.
const EXTRACT_BYTES: u32 = 512 * 1024;

/// Which screen is in front of the reader.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Home,
    Search,
    Results,
    Reading,
}

/// What the one outstanding request is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Search,
    Random,
    Extract,
}

#[derive(Default)]
struct Wiki {
    view: View,
    /// Where Back from the reading screen goes: a search a reader typed has
    /// results worth returning to, a random article does not.
    read_from: View,
    keyboard: Keyboard,
    /// What was typed, kept to caption the results screen while it loads.
    query: String,
    hits: Vec<api::Hit>,
    list_page: usize,
    /// The open article's title, once one has arrived.
    title: String,
    /// The open article's body, as section titles and paragraphs, in order.
    blocks: Vec<api::Block>,
    /// The article, cut into pages that fit the panel. Each paragraph carries
    /// its role, so a section title still reads larger than the paragraphs
    /// under it once the article has been split at the panel's edges.
    pages: Vec<Vec<(u32, u8, QuoteRole, String)>>,
    page: usize,
    /// Whether the last extract arrived at its byte ceiling, so the reading
    /// screen can say the article was cut rather than pretend it is whole.
    cut_short: bool,
    task: Option<(TaskId, Awaiting)>,
    problem: Option<String>,
    /// The last task failure, kept as the SDK read it rather than as a
    /// sentence: an empty screen wants the whole-screen version of the same
    /// thing, and a screen with something on it wants the banner.
    trouble: Option<Failure>,
}

impl Wiki {
    fn awaiting(&self, what: Awaiting) -> bool {
        matches!(self.task, Some((_, outstanding)) if outstanding == what)
    }

    fn ask(&mut self, context: &mut Context, url: String, what: Awaiting) {
        self.problem = None;
        self.trouble = None;
        match context.spawn_retrying(Task::Fetch {
            url,
            offset: 0,
            max_bytes: match what {
                Awaiting::Search => SEARCH_BYTES,
                Awaiting::Random => RANDOM_BYTES,
                Awaiting::Extract => EXTRACT_BYTES,
            },
            credential: None,
            headers: Vec::new(),
        }) {
            Some(task) => self.task = Some((task, what)),
            None => self.problem = Some("The device is busy. Try that again.".to_owned()),
        }
    }

    fn ask_search(&mut self, context: &mut Context, query: &str) {
        self.hits.clear();
        self.ask(context, api::search_url(query), Awaiting::Search);
    }

    fn ask_random(&mut self, context: &mut Context) {
        self.ask(context, api::random_url(), Awaiting::Random);
    }

    fn ask_extract(&mut self, context: &mut Context, title: &str) {
        self.pages.clear();
        self.ask(context, api::extract_url(title), Awaiting::Extract);
    }

    /// Cuts the open article into pages that fit the panel.
    fn lay_out(&mut self, context: &Context) {
        let paragraphs = self.article_paragraphs();
        let borrowed: Vec<_> = paragraphs
            .iter()
            .map(|(tag, depth, role, text)| (*tag, *depth, *role, text.as_str()))
            .collect();
        // No bar: a reading page carries nothing at its foot but the place
        // it is at. Reserving one leaves a hand's width of white above the
        // position and takes lines off every page.
        self.pages = context.paginate_tagged_reading(&borrowed, false);
        self.page = 0;
    }

    /// The article as prose, the title and every section title marked apart
    /// from the paragraphs under them.
    fn article_paragraphs(&self) -> Vec<(u32, u8, QuoteRole, String)> {
        let mut paragraphs = vec![(0, 0, QuoteRole::Heading, self.title.clone())];
        for block in &self.blocks {
            let role = if block.heading {
                QuoteRole::Heading
            } else {
                QuoteRole::Body
            };
            paragraphs.push((0, 0, role, block.text.clone()));
        }
        paragraphs
    }

    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(),
            View::Search => self.search(),
            View::Results => self.results(context),
            View::Reading => self.reading(),
        };
        // Every view but Home was reached from another one, so Back unwinds
        // this application first and leaves it only from Home.
        context.set_screen(screen.with_own_back(self.view != View::Home));
    }

    fn home(&self) -> Screen {
        let mut screen = ScreenBuilder::new("wiki-home");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(Awaiting::Random) {
            return screen
                .activity("Finding something", None)
                .skeleton(3)
                .build();
        }
        screen
            .splash(
                Some(Glyph::Globe),
                "Wikipedia",
                "Look something up, or let it choose.",
            )
            .primary_button("search", "Search")
            .buttons([("random", "Random article")])
            .build()
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("wiki-search").top_bar("Search Wikipedia");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        screen
            .typed(&self.keyboard, "A person, a place, anything")
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    fn results(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("wiki-results").top_bar("Results");
        if let Some(problem) = &self.problem {
            screen = screen.banner(BannerLevel::Attention, problem.clone());
        }
        if self.awaiting(Awaiting::Search) {
            return screen
                .divider()
                .activity(format!("Searching for {}", self.query), None)
                .skeleton(6)
                .build();
        }
        if self.hits.is_empty() {
            if let Some(failure) = self.trouble {
                return screen.failure_state(failure, "retry").build();
            }
            return screen
                .empty_state(format!("Nothing found for {}.", self.query))
                .primary_button("retry", "Try another search")
                .build();
        }
        let rows: Vec<(String, String)> = self
            .hits
            .iter()
            .map(|hit| {
                (
                    context.one_line_row(&hit.title, true),
                    context.clamped_row(&hit.snippet, 2, true),
                )
            })
            .collect();
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(title, snippet)| (title.as_str(), snippet.as_str()))
            .collect();
        let pages = context.paginate_rows(&borrowed, false);
        let pages = if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        };
        let page = self.list_page.min(pages.len().saturating_sub(1));
        let shown = pages.get(page).cloned().unwrap_or_default();
        screen = screen.rows(shown.iter().map(|index| {
            (
                format!("hit-{index}"),
                rows[*index].0.clone(),
                rows[*index].1.clone(),
                Glyph::Book,
            )
        }));
        if pages.len() <= 1 {
            return screen.build();
        }
        screen
            .page_turns("list-back", "list-next")
            .page_position(page_number(page), page_total(pages.len()))
            .build()
    }

    fn reading(&self) -> Screen {
        let mut screen = ScreenBuilder::new("wiki-reading")
            .top_bar(self.title.clone())
            .reading(true);
        if self.awaiting(Awaiting::Extract) {
            return screen
                .activity("Fetching the article", None)
                .skeleton(6)
                .build();
        }
        if self.pages.is_empty() {
            if let Some(failure) = self.trouble {
                return screen.failure_state(failure, "reload").build();
            }
            return screen.empty_state("This article arrived empty.").build();
        }
        if self.cut_short {
            screen = screen.banner(
                BannerLevel::Attention,
                "This article is longer than this can read in full; \
                 showing the beginning of it."
                    .to_owned(),
            );
        }
        let page = self.page.min(self.pages.len() - 1);
        for (_, depth, role, text) in &self.pages[page] {
            screen = screen.quote_as(*depth, *role, text.clone());
        }
        screen
            .page_turns("page-back", "page-next")
            .page_position(page_number(page), page_total(self.pages.len()))
            .build()
    }
}

/// A page number the position band can carry, one based and clamped.
fn page_number(page: usize) -> u16 {
    u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX)
}

/// How many pages there are, clamped.
fn page_total(pages: usize) -> u16 {
    u16::try_from(pages).unwrap_or(u16::MAX)
}

/// The index in a `prefix-N` action name, if that is what this is.
fn indexed(action: ActionId, prefix: &str, count: usize) -> Option<usize> {
    (0..count).find(|index| action_id(&format!("{prefix}-{index}")) == action)
}

impl KoboApp for Wiki {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        // The keyboard first: while the search screen is up, it owns the
        // panel.
        if self.view == View::Search {
            match self.keyboard.press(action) {
                Some(Pressed::Submitted) => {
                    let typed = self.keyboard.take().trim().to_owned();
                    if typed.is_empty() {
                        return;
                    }
                    self.query.clone_from(&typed);
                    self.view = View::Results;
                    self.list_page = 0;
                    self.ask_search(context, &typed);
                    self.show(context);
                    return;
                }
                Some(Pressed::Edited | Pressed::Shifted) => {
                    self.show(context);
                    return;
                }
                None => {}
            }
        }

        if action == ActionId::BACK {
            self.problem = None;
            self.trouble = None;
            match self.view {
                View::Home => {}
                View::Search => self.view = View::Home,
                View::Results => self.view = View::Search,
                View::Reading => self.view = self.read_from,
            }
            self.show(context);
            return;
        }

        if action == action_id("search") {
            self.keyboard.clear();
            self.problem = None;
            self.trouble = None;
            self.view = View::Search;
            self.show(context);
            return;
        }

        if action == action_id("random") {
            self.problem = None;
            self.trouble = None;
            self.read_from = View::Home;
            self.ask_random(context);
            self.show(context);
            return;
        }

        if action == action_id("retry") {
            self.view = View::Search;
            self.show(context);
            return;
        }

        if action == action_id("reload") {
            self.ask_extract(context, &self.title.clone());
            self.show(context);
            return;
        }

        if action == action_id("list-back") {
            self.list_page = self.list_page.saturating_sub(1);
            self.show(context);
            return;
        }

        if action == action_id("list-next") {
            self.list_page += 1;
            self.show(context);
            return;
        }

        if action == action_id("page-back") {
            self.page = self.page.saturating_sub(1);
            self.show(context);
            return;
        }

        if action == action_id("page-next") {
            if self.page + 1 < self.pages.len() {
                self.page += 1;
            }
            self.show(context);
            return;
        }

        if self.view == View::Results {
            if let Some(index) = indexed(action, "hit", self.hits.len()) {
                let Some(hit) = self.hits.get(index).cloned() else {
                    return;
                };
                self.read_from = View::Results;
                self.title.clone_from(&hit.title);
                self.blocks.clear();
                self.view = View::Reading;
                self.ask_extract(context, &hit.title);
                self.show(context);
            }
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let Some((outstanding, awaiting)) = self.task else {
            return;
        };
        if outstanding != task {
            return;
        }
        self.task = None;
        match outcome {
            TaskOutcome::Completed(bytes) => match awaiting {
                Awaiting::Search => {
                    self.hits = api::search_results(&bytes);
                }
                Awaiting::Random => match api::random_title(&bytes) {
                    Some(title) => {
                        self.title.clone_from(&title);
                        self.blocks.clear();
                        self.view = View::Reading;
                        self.ask_extract(context, &title);
                        self.show(context);
                        return;
                    }
                    None => {
                        self.problem = Some("Wikipedia's answer could not be read.".to_owned());
                    }
                },
                Awaiting::Extract => {
                    self.cut_short = bytes.len() >= EXTRACT_BYTES as usize;
                    match api::extract(&bytes) {
                        Some((title, blocks)) => {
                            self.title = title;
                            self.blocks = blocks;
                            self.lay_out(context);
                        }
                        None => {
                            self.problem = Some(if self.cut_short {
                                "This article is larger than this can read.".to_owned()
                            } else {
                                "That article could not be read.".to_owned()
                            });
                        }
                    }
                }
            },
            TaskOutcome::Failed(error) => {
                let failure = Failure::of(error);
                self.trouble = Some(failure);
                self.problem = Some(failure.advice.to_owned());
            }
            TaskOutcome::Cancelled => self.problem = Some("Cancelled.".to_owned()),
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("wiki", Wiki::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wiki: {error}");
            ExitCode::FAILURE
        }
    }
}
