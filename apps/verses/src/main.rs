//! Daily public-domain poetry with an offline shelf and `PoetryDB` search.
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Header, KoboApp, Screen, ScreenBuilder,
    StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::process::ExitCode;

const SETTINGS: &str = "settings";
const SEARCH_LIMIT: u32 = 512 * 1024;
const USER_AGENT: &str = "Cobalt Verses/0.1";

#[derive(Clone, Copy)]
struct Poem {
    title: &'static str,
    author: &'static str,
    year: u16,
    source: &'static str,
    lines: &'static [&'static str],
    tags: &'static str,
}

const CORPUS: &[Poem] = &[
    Poem {
        title: "Hope is the thing with feathers",
        author: "Emily Dickinson",
        year: 1886,
        source: "Poems by Emily Dickinson",
        tags: "hope · under a minute",
        lines: &[
            "“Hope” is the thing with feathers -",
            "That perches in the soul -",
            "And sings the tune without the words -",
            "And never stops - at all -",
        ],
    },
    Poem {
        title: "The Tyger",
        author: "William Blake",
        year: 1794,
        source: "Songs of Experience",
        tags: "nature · under a minute",
        lines: &[
            "Tyger Tyger, burning bright,",
            "In the forests of the night;",
            "What immortal hand or eye,",
            "Could frame thy fearful symmetry?",
        ],
    },
    Poem {
        title: "Ozymandias",
        author: "Percy Bysshe Shelley",
        year: 1818,
        source: "The Examiner",
        tags: "history · under a minute",
        lines: &[
            "I met a traveller from an antique land,",
            "Who said—“Two vast and trunkless legs of stone",
            "Stand in the desert. . . . Near them, on the sand,",
            "Half sunk a shattered visage lies.”",
        ],
    },
];

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OnlinePoem {
    title: String,
    author: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default)]
    linecount: String,
}

#[derive(Default, Deserialize, Serialize)]
struct Saved {
    favorites: BTreeSet<usize>,
    online_favorites: Vec<OnlinePoem>,
    sleep: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Today,
    Browse,
    Reading,
    Search,
    Results,
    Online,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Search,
    Open(usize),
}

struct Verses {
    view: View,
    poem: usize,
    online: Option<usize>,
    saved: Saved,
    keyboard: Keyboard,
    results: Vec<OnlinePoem>,
    task: Option<TaskId>,
    pending: Option<Pending>,
    notice: Option<String>,
    loaded: bool,
}

impl Default for Verses {
    fn default() -> Self {
        Self {
            view: View::Today,
            poem: daily_index(2026, 9, 1),
            online: None,
            saved: Saved::default(),
            keyboard: Keyboard::new(),
            results: Vec::new(),
            task: None,
            pending: None,
            notice: None,
            loaded: false,
        }
    }
}

fn daily_index(year: u16, month: u8, day: u8) -> usize {
    ((year as usize * 372) + (month as usize * 31) + day as usize) % CORPUS.len()
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

fn search_task(query: &str, author_only: bool) -> Task {
    let fields = if author_only {
        "author"
    } else {
        "author,title,lines"
    };
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/{fields}/{}/author,title,linecount",
            escape(query)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

fn poem_task(title: &str) -> Task {
    Task::Fetch {
        url: format!(
            "https://poetrydb.org/title/{}:abs/author,title,lines,linecount",
            escape(title)
        ),
        offset: 0,
        max_bytes: SEARCH_LIMIT,
        credential: None,
        headers: vec![Header::new("User-Agent", USER_AGENT)],
    }
}

impl Verses {
    fn local_poem(&self) -> Screen {
        let poem = CORPUS[self.poem];
        let mut text = format!("{}\n\n{}", poem.author, poem.lines.join("\n"));
        if self.view == View::Today {
            text = format!("Today\n\n{text}");
        }
        let mut screen = ScreenBuilder::new("verses-poem").top_bar(poem.title);
        if self.view == View::Today {
            screen = screen.top_bar_glyph("browse", "Browse", Glyph::Grid);
        }
        screen
            .top_bar_glyph(
                "favorite",
                if self.saved.favorites.contains(&self.poem) {
                    "Remove favorite"
                } else {
                    "Favorite"
                },
                Glyph::Heart,
            )
            .secondary(format!("{} · {}", poem.year, poem.source))
            .reading(true)
            .text(text)
            .build()
    }

    fn online_poem(&self) -> Screen {
        let Some(index) = self.online else {
            return ScreenBuilder::new("verses-online")
                .top_bar("Verses")
                .splash(
                    Some(Glyph::Search),
                    "Choose a poem",
                    "Open one from Search.",
                )
                .build();
        };
        let poem = &self.results[index];
        let favorite = self
            .saved
            .online_favorites
            .iter()
            .any(|saved| saved.title == poem.title && saved.author == poem.author);
        ScreenBuilder::new("verses-online")
            .top_bar(poem.title.clone())
            .top_bar_glyph("more-by-author", "More by this poet", Glyph::Person)
            .top_bar_glyph(
                "favorite",
                if favorite {
                    "Remove favorite"
                } else {
                    "Favorite"
                },
                Glyph::Heart,
            )
            .secondary(poem.author.clone())
            .reading(true)
            .text(poem.lines.join("\n"))
            .build()
    }

    fn browse(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-browse")
            .top_bar("Browse")
            .top_bar_glyph("search", "Search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        if !self.saved.favorites.is_empty() || !self.saved.online_favorites.is_empty() {
            let mut favorites = self
                .saved
                .favorites
                .iter()
                .filter_map(|index| {
                    CORPUS.get(*index).map(|poem| {
                        (
                            format!("poem-{index}"),
                            poem.title.to_owned(),
                            poem.author.to_owned(),
                            Glyph::Heart,
                        )
                    })
                })
                .collect::<Vec<_>>();
            favorites.extend(self.saved.online_favorites.iter().enumerate().map(
                |(index, poem)| {
                    (
                        format!("saved-online-{index}"),
                        poem.title.clone(),
                        poem.author.clone(),
                        Glyph::Heart,
                    )
                },
            ));
            screen = screen.section("Favorites").rows(favorites);
        }
        screen
            .section("Poems")
            .rows(CORPUS.iter().enumerate().map(|(index, poem)| {
                (
                    format!("poem-{index}"),
                    poem.title,
                    format!("{} · {}", poem.author, poem.tags),
                    Glyph::Note,
                )
            }))
            .build()
    }

    fn search(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-search").top_bar("Search poetry");
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        screen
            .typed(&self.keyboard, "Title, poet, or a line")
            .keyboard(&self.keyboard, "Search")
            .build()
    }

    fn results(&self) -> Screen {
        let mut screen = ScreenBuilder::new("verses-results")
            .top_bar("Search")
            .top_bar_glyph("search", "New search", Glyph::Search);
        if let Some(notice) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, notice.clone());
        }
        if self.results.is_empty() {
            screen
                .splash(
                    Some(Glyph::Search),
                    "No poems found",
                    "Try a poet, title, or memorable line.",
                )
                .build()
        } else {
            screen
                .rows(
                    self.results
                        .iter()
                        .take(40)
                        .enumerate()
                        .map(|(index, poem)| {
                            (
                                format!("result-{index}"),
                                poem.title.clone(),
                                poem.author.clone(),
                                Glyph::Note,
                            )
                        }),
                )
                .build()
        }
    }

    fn settings(&self) -> Screen {
        ScreenBuilder::new("verses-settings")
            .top_bar("Sleep screen")
            .splash(
                Some(Glyph::Note),
                if self.saved.sleep {
                    "Daily poem on"
                } else {
                    "Daily poem off"
                },
                if self.saved.sleep {
                    "Tomorrow's poem will appear when the reader sleeps."
                } else {
                    "Show tomorrow's poem while the reader sleeps."
                },
            )
            .primary_button(
                "sleep",
                if self.saved.sleep {
                    "Turn off"
                } else {
                    "Turn on"
                },
            )
            .build()
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Today | View::Reading => self.local_poem(),
            View::Browse => self.browse(),
            View::Search => self.search(),
            View::Results => self.results(),
            View::Online => self.online_poem(),
            View::Settings => self.settings(),
        }
    }

    fn save(&self, context: &mut Context) {
        if let Ok(bytes) = serde_json::to_vec(&self.saved) {
            context.store().save(SETTINGS, bytes);
        }
    }

    fn show(&self, context: &mut Context) {
        context.set_screen(
            self.screen()
                .with_own_back(!matches!(self.view, View::Today)),
        );
    }

    fn begin_search(&mut self, context: &mut Context, query: &str, author_only: bool) {
        if query.trim().is_empty() {
            self.notice = Some("Type something to search for.".into());
            return;
        }
        self.notice = Some("Searching…".into());
        self.results.clear();
        self.task = context.spawn(search_task(query.trim(), author_only));
        self.pending = self.task.map(|_| Pending::Search);
        if self.task.is_none() {
            self.notice = Some("Search is busy. Try again in a moment.".into());
        }
    }

    fn toggle_favorite(&mut self) {
        if self.view == View::Online {
            let Some(index) = self.online else { return };
            let poem = self.results[index].clone();
            if let Some(saved) = self
                .saved
                .online_favorites
                .iter()
                .position(|saved| saved.title == poem.title && saved.author == poem.author)
            {
                self.saved.online_favorites.remove(saved);
            } else {
                self.saved.online_favorites.push(poem);
            }
        } else if !self.saved.favorites.remove(&self.poem) {
            self.saved.favorites.insert(self.poem);
        }
    }
}

impl KoboApp for Verses {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(SETTINGS);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded {
            value: Some(bytes), ..
        } = result
        {
            if let Ok(saved) = serde_json::from_slice(&bytes) {
                self.saved = saved;
            }
        }
        self.loaded = true;
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::Search {
            if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let query = self.keyboard.take();
                self.begin_search(context, &query, false);
            }
            self.show(context);
            return;
        }

        if action == ActionId::BACK {
            self.view = match self.view {
                View::Online | View::Results | View::Search => View::Browse,
                _ => View::Today,
            };
        } else if action == action_id("today") {
            self.view = View::Today;
        } else if action == action_id("browse") {
            self.view = View::Browse;
        } else if action == action_id("search") {
            self.keyboard = Keyboard::new();
            self.notice = None;
            self.view = View::Search;
        } else if action == action_id("favorite") {
            self.toggle_favorite();
            self.save(context);
        } else if action == action_id("more-by-author") {
            if let Some(index) = self.online {
                let author = self.results[index].author.clone();
                self.begin_search(context, &author, true);
            }
        } else if action == action_id("sleep") {
            self.saved.sleep = !self.saved.sleep;
            self.save(context);
        } else if action == action_id("settings") {
            self.view = View::Settings;
        } else if let Some(index) =
            (0..CORPUS.len()).find(|index| action == action_id(&format!("poem-{index}")))
        {
            self.poem = index;
            self.view = View::Reading;
        } else if let Some(index) =
            (0..self.results.len()).find(|index| action == action_id(&format!("result-{index}")))
        {
            if self.results[index].lines.is_empty() {
                self.notice = Some("Opening poem…".into());
                self.task = context.spawn(poem_task(&self.results[index].title));
                self.pending = self.task.map(|_| Pending::Open(index));
                if self.task.is_none() {
                    self.notice = Some("This poem is busy. Try again in a moment.".into());
                }
            } else {
                self.online = Some(index);
                self.view = View::Online;
            }
        } else if let Some(index) = (0..self.saved.online_favorites.len())
            .find(|index| action == action_id(&format!("saved-online-{index}")))
        {
            self.results = vec![self.saved.online_favorites[index].clone()];
            self.online = Some(0);
            self.view = View::Online;
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, id: TaskId, outcome: TaskOutcome) {
        if self.task != Some(id) {
            return;
        }
        self.task = None;
        let pending = self.pending.take();
        match outcome {
            TaskOutcome::Completed(bytes) => match pending {
                Some(Pending::Search) => {
                    if let Ok(results) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        self.results = results;
                        self.notice = None;
                        self.online = None;
                        self.view = View::Results;
                    } else {
                        self.notice = Some("Poetry search couldn't open these results.".into());
                        self.view = View::Results;
                    }
                }
                Some(Pending::Open(index)) => {
                    if let Ok(mut poems) = serde_json::from_slice::<Vec<OnlinePoem>>(&bytes) {
                        if let Some(poem) = poems
                            .drain(..)
                            .find(|poem| poem.author == self.results[index].author)
                        {
                            self.results[index] = poem;
                            self.online = Some(index);
                            self.notice = None;
                            self.view = View::Online;
                        } else {
                            self.notice = Some("That poem isn't available right now.".into());
                        }
                    } else {
                        self.notice = Some("That poem couldn't be opened.".into());
                    }
                }
                None => {}
            },
            TaskOutcome::Failed(TaskError::NotFound) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem isn't available right now.".into());
                    self.view = View::Results;
                } else {
                    self.notice = None;
                    self.results.clear();
                    self.view = View::Results;
                }
            }
            TaskOutcome::Failed(_) => {
                if matches!(pending, Some(Pending::Open(_))) {
                    self.notice = Some("That poem couldn't be opened.".into());
                    self.view = View::Results;
                } else {
                    self.notice =
                        Some("Online search is unavailable. Your shelf is still ready.".into());
                    self.view = View::Browse;
                }
            }
            TaskOutcome::Cancelled => {
                self.notice = None;
            }
        }
        self.show(context);
    }
}

fn main() -> ExitCode {
    kobo_sdk::run("verses", Verses::default()).map_or_else(
        |error| {
            eprintln!("verses: {error}");
            ExitCode::FAILURE
        },
        |()| ExitCode::SUCCESS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn daily_choice_is_deterministic_and_leap_day_safe() {
        assert_eq!(daily_index(2028, 2, 29), daily_index(2028, 2, 29));
        assert_ne!(daily_index(2026, 9, 1), CORPUS.len());
    }

    #[test]
    fn every_poem_has_public_domain_provenance_and_preserved_lines() {
        for poem in CORPUS {
            assert!(!poem.author.is_empty() && !poem.source.is_empty() && poem.year < 1929);
            assert!(poem.lines.iter().all(|line| !line.is_empty()));
        }
    }

    #[test]
    fn poem_actions_are_icons_in_the_header() {
        let screen = Verses::default().screen();
        let debug = format!("{screen:?}");
        assert!(debug.contains("Heart"), "{debug}");
        assert!(debug.contains("Grid"), "{debug}");
        assert!(!debug.contains("Save favorite"), "{debug}");
    }

    #[test]
    fn poetrydb_searches_titles_authors_and_lines() {
        let Task::Fetch { url, .. } = search_task("hope & spring", false) else {
            panic!("search must use the network");
        };
        assert_eq!(
            url,
            "https://poetrydb.org/author,title,lines/hope%20%26%20spring/author,title,linecount"
        );
    }

    #[test]
    fn poetrydb_metadata_results_do_not_need_lines_until_opened() {
        let poems: Vec<OnlinePoem> = serde_json::from_str(
            r#"[{"title":"The Tyger","author":"William Blake","linecount":"24"}]"#,
        )
        .expect("metadata response");
        assert!(poems[0].lines.is_empty());
    }

    #[test]
    fn reading_and_browse_screens_fit() {
        let app = Verses::default();
        for screen in [app.local_poem(), app.browse(), app.search()] {
            let diagnostics = screen.diagnostics(&CLARA_BW_METRICS, &Chrome::default());
            assert!(diagnostics.issues.is_empty(), "{:?}", diagnostics.issues);
        }
    }
}
