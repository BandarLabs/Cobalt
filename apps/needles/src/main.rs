//! Needles keeps patterns and row counters usable without Wi-Fi. Ravelry Basic
//! credentials are only passed to the runtime by name and never enter app memory.

use kobo_bookview::{BookView, Step};
use kobo_json::Value;
use kobo_read::{Memory, Outcome};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::{process::ExitCode, time::Duration};

const STATE: &str = "counter-state-v1";
const PATTERN_BLOB: &str = "pattern.md";
const MAX_JSON: u32 = 256 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const MAX_PATTERNS: usize = 60;
const SECTIONS: [&str; 3] = ["Body", "Sleeve", "Finishing"];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Project,
    Library,
    Pattern,
    Reading,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Collection {
    #[default]
    Library,
    Queue,
    Favorites,
}

impl Collection {
    const ALL: [Self; 3] = [Self::Library, Self::Queue, Self::Favorites];

    const fn index(self) -> usize {
        match self {
            Self::Library => 0,
            Self::Queue => 1,
            Self::Favorites => 2,
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Self::Library => "ravelry-library-v1",
            Self::Queue => "ravelry-queue-v1",
            Self::Favorites => "ravelry-favorites-v1",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Queue => "Queue",
            Self::Favorites => "Favorites",
        }
    }

    const fn url(self) -> &'static str {
        match self {
            Self::Library => "https://api.ravelry.com/people/me/library/list.json",
            Self::Queue => "https://api.ravelry.com/people/me/queue/list.json",
            Self::Favorites => "https://api.ravelry.com/people/me/favorites/list.json",
        }
    }

    const fn result_key(self) -> &'static str {
        match self {
            Self::Library => "patterns",
            Self::Queue => "queued_projects",
            Self::Favorites => "favorites",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Counter {
    row: u32,
    repeat: u8,
    repeat_total: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pattern {
    title: String,
    detail: String,
}

struct Needles {
    route: Route,
    section: usize,
    counters: [Counter; 3],
    collection: Collection,
    libraries: [Vec<Pattern>; 3],
    loaded: [bool; 3],
    selected: Option<Pattern>,
    notice: Option<String>,
    task: Option<(TaskId, Collection)>,
    loading: Option<ShelfDownload>,
    book: BookView,
}

impl Needles {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Project));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Project => self.project(),
            Route::Library => self.library(),
            Route::Pattern => self.pattern(),
            Route::Reading => self
                .book
                .screen(
                    self.selected
                        .as_ref()
                        .map_or("Pattern", |pattern| &pattern.title),
                )
                .unwrap_or_else(|| {
                    ScreenBuilder::new("needles-reader")
                        .top_bar("Pattern")
                        .secondary("Opening your synced pattern…")
                        .build()
                }),
        }
    }

    fn counter(&self) -> &Counter {
        &self.counters[self.section]
    }

    fn counter_mut(&mut self) -> &mut Counter {
        &mut self.counters[self.section]
    }

    fn project(&self) -> Screen {
        let mut screen = ScreenBuilder::new("needles-project").top_bar("Needles");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        let counter = self.counter();
        let selected = self
            .selected
            .as_ref()
            .map_or("Row counter", |pattern| pattern.title.as_str());
        screen
            .section(selected)
            .facts([
                ("Section", SECTIONS[self.section].to_owned()),
                ("Row", counter.row.to_string()),
                (
                    "Repeat",
                    if counter.repeat == 0 {
                        format!("ready for 1 of {}", counter.repeat_total)
                    } else {
                        format!("row {} of {}", counter.repeat, counter.repeat_total)
                    },
                ),
                ("Stand", "Screen stays awake while counting".to_owned()),
            ])
            .text("Counters and synced pattern text stay available offline.")
            .primary_button("plus", "+1 row")
            .buttons([("undo", "Undo −1 row"), ("section", "Change section")])
            .buttons([
                ("repeat-total", "Repeat length"),
                ("read", "Read synced pattern"),
            ])
            .button("library", "Library, queue and favorites")
            .build()
    }

    fn library(&self) -> Screen {
        let mut screen = ScreenBuilder::new("needles-library").top_bar("Library");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        let selected = self.collection.index();
        if self.loaded.iter().any(|loaded| *loaded) {
            screen = screen.tabs(
                selected,
                [
                    ("library-tab", "Library"),
                    ("queue-tab", "Queue"),
                    ("favorites-tab", "Favorites"),
                ],
            );
            if self.libraries[selected].is_empty() {
                screen = screen.splash(
                    Some(Glyph::Bookmark),
                    format!("No {} patterns", self.collection.title().to_lowercase()),
                    "Add your Ravelry sign-in during setup, then sync this collection.",
                );
            } else {
                screen = screen.section(self.collection.title()).rows(
                    self.libraries[selected]
                        .iter()
                        .enumerate()
                        .map(|(index, pattern)| {
                            (
                                format!("pattern-{index}"),
                                pattern.title.clone(),
                                pattern.detail.clone(),
                                Glyph::Bookmark,
                            )
                        }),
                );
            }
        } else {
            screen = screen.skeleton(5);
        }
        screen
            .primary_button("sync", format!("Sync {}", self.collection.title()))
            .build()
    }

    fn pattern(&self) -> Screen {
        let title = self
            .selected
            .as_ref()
            .map_or("Pattern", |pattern| pattern.title.as_str());
        let mut screen = ScreenBuilder::new("needles-pattern")
            .top_bar("Pattern")
            .heading(title)
            .text("Use `kobo needles push` on your computer to prepare and transfer a PDF you own. Text pages reflow here.")
            .primary_button("follow", "Follow this pattern")
            .button("read", "Open synced pattern");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen.build()
    }

    fn save(&self, context: &mut Context) {
        let counters = self
            .counters
            .iter()
            .map(|counter| {
                format!(
                    "{},{},{}",
                    counter.row, counter.repeat, counter.repeat_total
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let selected = self
            .selected
            .as_ref()
            .map_or_else(String::new, |pattern| hex(&pattern.title));
        context.store().save(
            STATE,
            format!("1\n{}\n{}\n{}", self.section, selected, counters),
        );
    }

    fn increment(&mut self, context: &mut Context) {
        let counter = self.counter_mut();
        counter.row = counter.row.saturating_add(1);
        counter.repeat = if counter.repeat >= counter.repeat_total {
            1
        } else {
            counter.repeat.saturating_add(1)
        };
        self.notice = None;
        context.device().keep_awake(Duration::from_secs(14_400));
        self.save(context);
    }

    fn undo(&mut self, context: &mut Context) {
        let counter = self.counter_mut();
        if counter.row > 0 {
            counter.row -= 1;
            counter.repeat = if counter.row == 0 {
                0
            } else if counter.repeat <= 1 {
                counter.repeat_total
            } else {
                counter.repeat - 1
            };
            self.notice = None;
            self.save(context);
        } else {
            self.notice = Some("Already at row 0; nothing was undone.".to_owned());
        }
    }

    fn cycle_repeat_total(&mut self, context: &mut Context) {
        let counter = self.counter_mut();
        counter.repeat_total = match counter.repeat_total {
            4 => 8,
            8 => 12,
            12 => 16,
            _ => 4,
        };
        if counter.repeat > counter.repeat_total {
            counter.repeat = counter.repeat_total;
        }
        self.save(context);
    }

    fn sync(&mut self, context: &mut Context) {
        let collection = self.collection;
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: collection.url().to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential: Some(Credential::basic("ravelry")),
            headers: Vec::new(),
        }) {
            self.task = Some((task, collection));
            self.notice = Some(format!(
                "Reading your Ravelry {}.",
                collection.title().to_lowercase()
            ));
        }
    }

    fn read_library(&mut self, bytes: &[u8], collection: Collection) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(patterns) = value.get(collection.result_key()).and_then(Value::as_array) else {
            return false;
        };
        self.libraries[collection.index()] = patterns
            .iter()
            .filter_map(pattern_from)
            .take(MAX_PATTERNS)
            .collect();
        self.loaded[collection.index()] = true;
        true
    }

    fn open_pattern(&mut self, context: &mut Context) {
        if self.loading.is_some() {
            return;
        }
        let mut loading = ShelfDownload::new(PATTERN_BLOB).at_most(MAX_PATTERN);
        loading.start(context);
        self.loading = Some(loading);
        self.notice = Some("Opening the pattern transferred from your computer.".to_owned());
    }

    fn restore(&mut self, bytes: &[u8]) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        let mut fields = text.lines();
        let (Some("1"), Some(section), Some(selected), Some(counters)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return;
        };
        let Ok(section) = section.parse::<usize>() else {
            return;
        };
        if section >= SECTIONS.len() {
            return;
        }
        let parsed = counters
            .split('|')
            .map(|counter| {
                let mut parts = counter.split(',');
                let (Some(row), Some(repeat), Some(total)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return None;
                };
                let (Ok(row), Ok(repeat), Ok(repeat_total)) =
                    (row.parse(), repeat.parse(), total.parse())
                else {
                    return None;
                };
                (repeat_total > 0 && repeat <= repeat_total).then_some(Counter {
                    row,
                    repeat,
                    repeat_total,
                })
            })
            .collect::<Option<Vec<_>>>();
        let Some(parsed) = parsed else {
            return;
        };
        let Ok(counters) = <[Counter; 3]>::try_from(parsed) else {
            return;
        };
        self.section = section;
        self.counters = counters;
        self.selected = unhex(selected)
            .filter(|title| !title.is_empty())
            .map(|title| Pattern {
                title,
                detail: "Selected pattern".to_owned(),
            });
    }
}

impl Default for Needles {
    fn default() -> Self {
        Self {
            route: Route::Project,
            section: 0,
            counters: std::array::from_fn(|_| Counter {
                repeat_total: 12,
                ..Counter::default()
            }),
            collection: Collection::Library,
            libraries: std::array::from_fn(|_| Vec::new()),
            loaded: [false; 3],
            selected: None,
            notice: None,
            task: None,
            loading: None,
            book: BookView::new(),
        }
    }
}

fn pattern_from(value: &Value) -> Option<Pattern> {
    let nested = value.get("pattern");
    let title = value
        .get("name")
        .or_else(|| value.get("pattern_name"))
        .or_else(|| nested.and_then(|pattern| pattern.get("name")))
        .and_then(Value::as_str)?
        .trim();
    if title.is_empty() {
        return None;
    }
    let detail = value
        .get("designer")
        .or_else(|| value.get("pattern_author"))
        .or_else(|| nested.and_then(|pattern| pattern.get("designer")))
        .and_then(Value::as_str)
        .map_or_else(|| "Ravelry pattern".to_owned(), str::to_owned);
    Some(Pattern {
        title: title.to_owned(),
        detail,
    })
}

fn hex(text: &str) -> String {
    use std::fmt::Write as _;
    text.bytes().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn unhex(text: &str) -> Option<String> {
    if text.len() % 2 != 0 {
        return None;
    }
    let bytes = text
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from(high * 16 + low).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

impl KoboApp for Needles {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        for collection in Collection::ALL {
            context.store().load(collection.key());
        }
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = &result {
            if key == STATE {
                if let Some(value) = value {
                    self.restore(value);
                }
            } else if let Some(collection) = Collection::ALL
                .into_iter()
                .find(|collection| key == collection.key())
            {
                if let Some(value) = value {
                    let _ignored = self.read_library(value, collection);
                }
                self.loaded[collection.index()] = true;
            }
        }

        let loading_progress = self
            .loading
            .as_mut()
            .map(|loading| loading.advance(context, &result));
        if let Some(progress) = loading_progress {
            match progress {
                ShelfProgress::Done => {
                    let Some(loading) = self.loading.take() else {
                        self.show(context);
                        return;
                    };
                    match self.book.open_bytes(
                        context,
                        PATTERN_BLOB,
                        &loading.take(),
                        Memory::default(),
                    ) {
                        Ok(()) => {
                            self.route = Route::Reading;
                            self.notice = None;
                        }
                        Err(_) => {
                            self.notice = Some(
                                "The transferred pattern is not readable Markdown or text. Run `kobo needles push` again."
                                    .to_owned(),
                            );
                        }
                    }
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.notice = Some(
                        "No readable pattern is on this Kobo yet. Run `kobo needles push PATTERN.pdf --device <address>` on your computer."
                            .to_owned(),
                    );
                }
                ShelfProgress::Elsewhere | ShelfProgress::Moving { .. } => {}
            }
        }
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.route == Route::Reading {
            if let Some(outcome) = self.book.act(context, action) {
                if matches!(outcome, Outcome::Close) {
                    self.book.close(context);
                    self.route = Route::Project;
                    context.device().allow_sleep();
                }
                self.show(context);
                return;
            }
        }
        if action == ActionId::BACK {
            self.route = Route::Project;
        } else if action == action_id("plus") {
            self.increment(context);
        } else if action == action_id("undo") {
            self.undo(context);
        } else if action == action_id("section") {
            self.section = (self.section + 1) % SECTIONS.len();
            self.notice = None;
            self.save(context);
        } else if action == action_id("repeat-total") {
            self.cycle_repeat_total(context);
        } else if action == action_id("library") {
            self.route = Route::Library;
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("library-tab") {
            self.collection = Collection::Library;
        } else if action == action_id("queue-tab") {
            self.collection = Collection::Queue;
        } else if action == action_id("favorites-tab") {
            self.collection = Collection::Favorites;
        } else if action == action_id("follow") {
            self.route = Route::Project;
            context.device().keep_awake(Duration::from_secs(14_400));
            self.save(context);
        } else if action == action_id("read") {
            self.open_pattern(context);
        } else if let Some(index) = (0..self.libraries[self.collection.index()].len())
            .find(|index| action == action_id(&format!("pattern-{index}")))
        {
            self.selected = Some(self.libraries[self.collection.index()][index].clone());
            self.route = Route::Pattern;
            self.notice = None;
            self.save(context);
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.book.woke(context, task, &outcome) != Step::Elsewhere {
            self.show(context);
            return;
        }
        let Some((_, collection)) = self.task.take_if(|(known, _)| *known == task) else {
            return;
        };
        self.notice = match outcome {
            TaskOutcome::Completed(bytes) if self.read_library(&bytes, collection) => {
                context.store().save(collection.key(), bytes);
                None
            }
            TaskOutcome::Completed(_) => Some(format!(
                "Ravelry returned a {} this version cannot read.",
                collection.title().to_lowercase()
            )),
            TaskOutcome::Failed(TaskError::NoCredential) => Some(
                "Install your credential with `kobo secret set ravelry --device <address>`."
                    .to_owned(),
            ),
            TaskOutcome::Failed(TaskError::Unauthorized) => {
                Some("Ravelry did not accept the named Basic credential.".to_owned())
            }
            TaskOutcome::Failed(error) => Some(Failure::of(error).naming("ravelry")),
            TaskOutcome::Cancelled => Some("The library sync was cancelled.".to_owned()),
        };
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("needles", Needles::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("needles: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{hex, pattern_from, Collection, Counter, Needles, Route, SECTIONS};
    use kobo_sdk::{action_id, Context, KoboApp, StoreResult};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn counter_repeats_undoes_and_keeps_section_totals() {
        let mut app = Needles {
            counters: [
                Counter {
                    row: 11,
                    repeat: 11,
                    repeat_total: 12,
                },
                Counter {
                    repeat_total: 8,
                    ..Counter::default()
                },
                Counter {
                    repeat_total: 4,
                    ..Counter::default()
                },
            ],
            ..Needles::default()
        };
        let mut context = Context::default();
        app.increment(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (12, 12));
        app.increment(&mut context);
        assert_eq!(app.counter().repeat, 1);
        app.undo(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (12, 12));
        app.section = 1;
        app.increment(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (1, 1));
        assert_eq!(app.counters[0].row, 12);
    }

    #[test]
    fn state_round_trips_selected_pattern_and_all_sections() {
        let mut app = Needles {
            section: 1,
            ..Needles::default()
        };
        app.counters[0].row = 42;
        app.counters[1] = Counter {
            row: 7,
            repeat: 7,
            repeat_total: 8,
        };
        app.selected = Some(super::Pattern {
            title: "Warm sweater".to_owned(),
            detail: "Ravelry pattern".to_owned(),
        });
        let mut context = Context::default();
        app.save(&mut context);
        let saved = context
            .commands()
            .iter()
            .find_map(|command| match command {
                kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                    if key == super::STATE =>
                {
                    Some(value.clone())
                }
                _ => None,
            })
            .expect("saved state");
        let mut restored = Needles::default();
        restored.on_store(
            &mut Context::default(),
            StoreResult::Loaded {
                key: super::STATE.to_owned(),
                value: Some(saved),
            },
        );
        assert_eq!(restored.section, 1);
        assert_eq!(restored.counters[0].row, 42);
        assert_eq!(restored.counters[1].repeat_total, 8);
        assert_eq!(
            restored
                .selected
                .as_ref()
                .map(|pattern| pattern.title.as_str()),
            Some("Warm sweater")
        );
    }

    #[test]
    fn ravelry_collections_are_bounded_and_parse_nested_queue_patterns() {
        let response = kobo_json::parse(
            r#"{"queued_projects":[{"pattern":{"name":"Clouds"}},{"pattern":{"name":"Moss"}}]}"#,
        )
        .expect("json");
        let patterns = response
            .get(Collection::Queue.result_key())
            .and_then(kobo_json::Value::as_array)
            .expect("queue");
        assert_eq!(patterns.iter().filter_map(pattern_from).count(), 2);
        assert_eq!(hex("Warm sweater"), "5761726d2073776561746572");
        assert_eq!(SECTIONS, ["Body", "Sleeve", "Finishing"]);
    }

    #[test]
    fn counter_target_is_large_on_clara() {
        let app = Needles::default();
        let layout = app
            .project()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let plus = layout
            .rect_of_action(action_id("plus"))
            .expect("counter target");
        assert!(plus.height >= CLARA_BW_METRICS.touch_target_minimum());
        let mut app = app;
        let mut context = Context::default();
        app.on_action(&mut context, action_id("library"));
        assert_eq!(app.route, Route::Library);
        assert!(app
            .project()
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
