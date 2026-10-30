//! Needles keeps patterns and row counters usable without Wi-Fi. Ravelry Basic
//! credentials are only passed to the runtime by name and never enter app memory.

use kobo_bookview::positions::{Progress, KEY as POSITIONS};
use kobo_bookview::{BookView, Step};
use kobo_json::Value;
use kobo_read::Outcome;
use kobo_sdk::failure::{Cause, Problem, Service};
use kobo_sdk::{
    action_id, is_valid_key, ActionId, BandAlign, BannerLevel, Context, Credential, Glyph, KoboApp,
    RowLead, Screen, ScreenBuilder, ShelfDownload, ShelfProgress, SlotWidth, StoreResult, Task,
    TaskId, TaskOutcome,
};
use std::{cell::Cell, collections::VecDeque, process::ExitCode, time::Duration};

const STATE: &str = "counter-state-v1";
/// Which way up the owner asked for, kept apart from the counts so turning
/// the reader never rewrites them: `landscape` or `portrait`.
const TURN: &str = "turn-v1";
/// The one pattern a push wrote before patterns had names of their own.
const PATTERN_BLOB: &str = "pattern.md";
/// Patterns pushed since, one shelf file each: `pattern-<name>.md`.
const PATTERN_PREFIX: &str = "pattern-";
/// The titles of the pushed patterns, one `file<TAB>title` line each, written
/// by `kobo needles push` so the list shows a pattern's real name before it
/// has ever been opened.
const PATTERN_INDEX: &str = "patterns.tsv";
const MAX_INDEX: usize = 64 * 1024;
const MAX_JSON: u32 = 256 * 1024;
const MAX_PATTERN: usize = 4 * 1024 * 1024;
const MAX_PATTERNS: usize = 60;
/// More named sections than a pattern reasonably has; past this the heading
/// list is being read as something it is not.
const MAX_SECTIONS: usize = 12;
const SECTIONS: [&str; 3] = ["Body", "Sleeve", "Finishing"];
/// The repeat lengths offered, in rows. They are a choice rather than a
/// cycle, so a knitter on row 10 of a 12-row repeat reaches 16 in one tap
/// instead of passing through 4 and losing the place on the way.
const REPEATS: [u8; 4] = [4, 8, 12, 16];
/// Ravelry, as every failed sync is told. The key is installed from a
/// computer, so the fix is the command that installs it there and the
/// banner offers no sign-in on the reader.
const RAVELRY: Service<'static> =
    Service::new("Ravelry").fixed_by("Run kobo secret set ravelry on your computer.");
/// What the counter says when a pattern it was asked to open has gone.
const PATTERN_GONE: &str =
    "That pattern is no longer on this Kobo. Send it again with kobo needles push.";
/// What the counter says when a pattern arrived but cannot be read.
const PATTERN_UNREADABLE: &str =
    "This pattern could not be read. Send it again with kobo needles push.";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Project,
    /// Everything being counted, for switching.
    Projects,
    Library,
    Pattern,
    /// The patterns sent from the computer, to choose one to read.
    Patterns,
    Reading,
    /// The project's sections, to pick the one being knitted.
    Sections,
    /// How many rows the repeat of the section being knitted has.
    RepeatLength,
}

/// How the counter is laid out on this panel at this text size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CounterShape {
    /// The count and its controls side by side, for a panel held sideways,
    /// where the one stacked above the other is taller than the panel at
    /// every text size.
    beside: bool,
    /// The shortcut to a pattern sent from the computer.
    shortcut: bool,
    /// The bar drawing where the row sits in the repeat.
    bar: bool,
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

/// A pattern file on this Kobo, sent from the computer.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Shelved {
    file: String,
    title: String,
}

/// One thing on the needles: a name, the section being worked, and a counter
/// for each named section.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Project {
    name: String,
    /// The sections this project counts through: an imported pattern's own
    /// once one is read, the everyday three until then.
    sections: Vec<String>,
    section: usize,
    counters: Vec<Counter>,
}

impl Project {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sections: SECTIONS
                .iter()
                .map(|section| (*section).to_owned())
                .collect(),
            section: 0,
            counters: SECTIONS
                .iter()
                .map(|_| Counter {
                    repeat_total: 12,
                    ..Counter::default()
                })
                .collect(),
        }
    }

    /// The imported pattern's sections become this project's. A section
    /// already being counted keeps its count; the rest start fresh.
    fn adopt_sections(&mut self, sections: Vec<String>) {
        if sections.is_empty() {
            return;
        }
        let working = self.sections.get(self.section).cloned();
        let counters = sections
            .iter()
            .map(|name| {
                self.sections
                    .iter()
                    .position(|known| known == name)
                    .map_or_else(
                        || Counter {
                            repeat_total: 12,
                            ..Counter::default()
                        },
                        |index| self.counters[index].clone(),
                    )
            })
            .collect();
        self.section = working
            .and_then(|name| sections.iter().position(|section| *section == name))
            .unwrap_or(0);
        self.sections = sections;
        self.counters = counters;
    }
}

struct Needles {
    route: Route,
    /// The projects on the go. Never empty: the first is the plain row
    /// counter, there before anything is followed.
    projects: Vec<Project>,
    /// Which project the counter screen is counting.
    current: usize,
    collection: Collection,
    project_page: usize,
    collection_pages: [usize; 3],
    libraries: [Vec<Pattern>; 3],
    loaded: [bool; 3],
    selected: Option<Pattern>,
    /// What the last thing done has to say, and how much it matters: news
    /// and nudges are Info, a failure the owner has to act on is an Error.
    notice: Option<(BannerLevel, String)>,
    /// The last Ravelry sync that failed: the collection, and why.
    sync_problem: Option<(Collection, Problem)>,
    /// The page of the section list on show.
    sections_page: u16,
    task: Option<(TaskId, Collection)>,
    loading: Option<ShelfDownload>,
    /// The charts the open pattern refers to, waiting their turn to be read
    /// off the shelf.
    charts: VecDeque<String>,
    /// The chart being read off the shelf, and its transfer.
    chart: Option<(String, ShelfDownload)>,
    book: BookView,
    /// The patterns on this Kobo, in the order they were sent.
    shelved: Vec<Shelved>,
    /// Titles from the push index, by file.
    titles: Vec<(String, String)>,
    /// The pattern files on the shelf, as last listed.
    listed: Vec<String>,
    index_load: Option<ShelfDownload>,
    patterns_page: usize,
    /// The pattern file open in the reader, or loading into it.
    reading: Option<String>,
    /// Where each pattern was left, so it reopens on the same page.
    positions: Progress,
    /// The count a "Start section again" cleared, so Undo can bring it back:
    /// the project, the section and the counter as it was.
    restarted: Option<(usize, usize, Counter)>,
    /// Whether the counter's More menu is open.
    menu_open: bool,
    /// Whether the owner asked for the counter held sideways. The runtime
    /// keeps an app upright until it asks, so the counter turns only on
    /// Use landscape, and stays turned across restarts.
    landscape: bool,
    /// The way up last asked of the runtime, so it is asked once a change.
    sent_orientation: Cell<Option<kobo_sdk::Orientation>>,
}

impl Needles {
    /// Which way up this screen is drawn.
    ///
    /// Only the counter and its two choices turn. The lists and the pattern
    /// reader are paged by the runtime's measure of the panel, which is
    /// always the upright one, so turned they would be paged for a taller
    /// panel than they are drawn on and lose their last rows.
    fn orientation(&self) -> kobo_sdk::Orientation {
        if self.landscape
            && matches!(
                self.route,
                Route::Project | Route::Sections | Route::RepeatLength
            )
        {
            kobo_sdk::Orientation::Landscape
        } else {
            kobo_sdk::Orientation::Portrait
        }
    }

    /// The panel this screen is drawn on: the runtime's upright one, turned
    /// when this screen asked to be.
    fn panel(&self, context: &Context) -> kobo_sdk::DisplayMetrics {
        let metrics = context.metrics();
        if self.orientation() == kobo_sdk::Orientation::Landscape && metrics.width < metrics.height
        {
            metrics.oriented(kobo_sdk::Orientation::Landscape)
        } else {
            metrics
        }
    }

    fn show(&self, context: &mut Context) {
        let orientation = self.orientation();
        if self.sent_orientation.get() != Some(orientation) {
            context.set_orientation(orientation);
            self.sent_orientation.set(Some(orientation));
        }
        // An open menu claims Back, so the tap that puts it away does not
        // also close the app.
        context.set_screen(
            self.screen(context)
                .with_own_back(self.route != Route::Project || self.menu_open),
        );
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.route {
            Route::Project => self.project(context),
            Route::Projects => self.projects_screen(context).0,
            Route::Library => self.library(context).0,
            Route::Pattern => self.pattern(context),
            Route::Patterns => self.patterns_screen(context).0,
            Route::Sections => self.sections_screen(),
            Route::RepeatLength => self.repeat_screen(),
            Route::Reading => self.book.screen(self.reading_title()).unwrap_or_else(|| {
                ScreenBuilder::new("needles-reader")
                    .top_bar("Pattern")
                    .secondary("Opening your synced pattern…")
                    .build()
            }),
        }
    }

    fn counter(&self) -> &Counter {
        let project = &self.projects[self.current];
        &project.counters[project.section]
    }

    fn counter_mut(&mut self) -> &mut Counter {
        let project = &mut self.projects[self.current];
        &mut project.counters[project.section]
    }

    /// The project a followed pattern counts against, made on first follow.
    fn project_for(&mut self, name: &str) -> usize {
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| project.name == name)
        {
            return index;
        }
        self.projects.push(Project::new(name));
        self.projects.len() - 1
    }

    /// The counter, with the shortcut to a sent pattern when there is room.
    ///
    /// A notice over the counter, such as the one saying Undo brings a
    /// restarted section back, pushed Undo and +1 row off the panel at the
    /// larger text sizes. The shortcut is the one thing here with another
    /// way in (Library, then a pattern, then Read a pattern), so it gives
    /// way first; then the bar under the repeat, which draws what the
    /// Repeat line already says. The count and its controls never go.
    ///
    /// Held sideways the panel is wide and short, and the count stacked
    /// over its controls is taller than a Clara at every text size, so
    /// there the count stands beside its controls, as the audio player's
    /// book stands beside its transport.
    fn project(&self, context: &Context) -> Screen {
        let metrics = self.panel(context);
        let beside = metrics.width > metrics.height;
        let fits = |screen: &Screen| {
            !screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors()
        };
        [(true, true), (false, true)]
            .into_iter()
            .map(|(shortcut, bar)| {
                self.project_with(CounterShape {
                    beside,
                    shortcut,
                    bar,
                })
            })
            .find(fits)
            .unwrap_or_else(|| {
                self.project_with(CounterShape {
                    beside,
                    shortcut: false,
                    bar: false,
                })
            })
    }

    fn project_with(&self, shape: CounterShape) -> Screen {
        // Projects is the place a knitter goes most, so it is a mark in the
        // bar rather than another box in a wall of controls. The Ravelry
        // library and the way up go under More, the bar's second and last
        // control.
        let mut screen = ScreenBuilder::new("needles-project")
            .top_bar("Needles")
            .top_bar_glyph("projects", "Projects", Glyph::Folder)
            .top_bar_overflow(
                "more",
                self.menu_open,
                [
                    ("library", "Ravelry library"),
                    (
                        "turn",
                        if self.landscape {
                            "Use portrait"
                        } else {
                            "Use landscape"
                        },
                    ),
                ],
            );
        if let Some((level, note)) = &self.notice {
            screen = screen.banner(*level, note);
        }
        let count = move |screen: ScreenBuilder| self.count(screen, shape);
        if shape.beside {
            return screen
                .band(
                    BandAlign::Top,
                    [
                        (
                            SlotWidth::Fill,
                            Box::new(count) as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder + '_>,
                        ),
                        (SlotWidth::Fill, Box::new(Self::controls)),
                    ],
                )
                .build();
        }
        // The count is what this screen is for, so the controls sit at the
        // foot where a thumb rests between stitches.
        Self::controls(count(screen).fill()).build()
    }

    /// The project, its row count, where the row sits in the repeat, and
    /// the shortcut to a pattern when the shape has room for it.
    fn count(&self, screen: ScreenBuilder, shape: CounterShape) -> ScreenBuilder {
        let counter = self.counter();
        let project = &self.projects[self.current];
        let screen = screen
            .section(project.name.as_str())
            // The row count is the thing a knitter glances at between
            // stitches, so it is the heading rather than one fact among four.
            .heading(format!("Row {}", counter.row))
            .facts([
                (
                    "Section",
                    project
                        .sections
                        .get(project.section)
                        .cloned()
                        .unwrap_or_default(),
                ),
                (
                    "Repeat",
                    if counter.repeat == 0 {
                        format!("ready for 1 of {}", counter.repeat_total)
                    } else {
                        format!("row {} of {}", counter.repeat, counter.repeat_total)
                    },
                ),
            ]);
        // Where in the repeat the row sits, drawn rather than only said.
        let screen = if shape.bar {
            screen.progress(if counter.repeat == 0 {
                0
            } else {
                #[allow(clippy::integer_division)]
                (u32::from(counter.repeat) * 100 / u32::from(counter.repeat_total))
                    .min(100)
                    .try_into()
                    .unwrap_or(100)
            })
        } else {
            screen
        };
        if !shape.shortcut {
            return screen;
        }
        screen.rows([(
            "read",
            "Read a pattern",
            match self.shelved.len() {
                0 => "Send one from your computer".to_owned(),
                1 => self.shelved[0].title.clone(),
                count => format!("{count} patterns from your computer"),
            },
            Glyph::Note,
        )])
    }

    /// The counter's controls. +1 row is the one filled control: it is
    /// pressed a hundred times for every press of anything else, and it was
    /// the same small box as six things done once a project.
    fn controls(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .buttons([
                ("section", "Change section"),
                ("repeat-total", "Repeat length"),
            ])
            // The second sleeve is the same rows again: a fresh count for
            // the section, and Undo beside +1 brings the old one back.
            .button("restart-section", "Start section again")
            // Undo still sits beside the increment it reverses: a miscount is
            // fixed with a tap next to the tap that made it.
            .band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(|slot: ScreenBuilder| slot.button("undo", "Undo"))
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(|slot: ScreenBuilder| slot.primary_button("plus", "+1 row")),
                    ),
                ],
            )
    }

    /// The project's sections as one answer from a list, the one being
    /// knitted marked and each with its own row, so any section is one tap
    /// away. Changing section used to step through them one full-panel
    /// refresh at a time, and an overshoot went all the way round again.
    fn sections_screen(&self) -> Screen {
        let project = &self.projects[self.current];
        ScreenBuilder::new("needles-sections")
            .top_bar("Sections")
            .option_list(
                "Which section are you knitting?",
                self.sections_page,
                "sections-previous",
                "sections-next",
                project
                    .sections
                    .iter()
                    .zip(&project.counters)
                    .enumerate()
                    .map(|(index, (name, counter))| {
                        (
                            format!("section-{index}"),
                            name.clone(),
                            format!("Row {}", counter.row),
                        )
                    }),
            )
            .chosen(project.section)
            .build()
    }

    /// The repeat lengths, the current one marked. A tap sets it and goes
    /// back to the count.
    fn repeat_screen(&self) -> Screen {
        let total = self.counter().repeat_total;
        let screen = ScreenBuilder::new("needles-repeat")
            .top_bar("Repeat length")
            .choose(
                "Rows in the repeat",
                REPEATS
                    .iter()
                    .map(|rows| (format!("repeat-{rows}"), format!("{rows} rows"))),
            );
        match REPEATS.iter().position(|rows| *rows == total) {
            Some(index) => screen.chosen(index),
            None => screen,
        }
        .build()
    }

    /// The page of the section list holding the section being knitted,
    /// counted the way the runtime pages it.
    fn open_sections(&mut self, context: &Context) {
        self.route = Route::Sections;
        self.notice = None;
        let per_page = kobo_sdk::options_per_page(&self.panel(context), true).max(1);
        let section = self.projects[self.current].section;
        self.sections_page = u16::try_from(section / per_page).unwrap_or(0);
    }

    fn turn_sections(&mut self, context: &Context, forward: bool) {
        let last = kobo_sdk::options_pages(
            self.projects[self.current].sections.len(),
            &self.panel(context),
            true,
        )
        .saturating_sub(1);
        self.sections_page = if forward {
            self.sections_page
                .saturating_add(1)
                .min(u16::try_from(last).unwrap_or(0))
        } else {
            self.sections_page.saturating_sub(1)
        };
    }

    /// Everything being counted, each with where it stands, so switching
    /// projects is picking up the right needle rather than starting over.
    fn paged_rows(
        context: &Context,
        screen: ScreenBuilder,
        rows: &[(String, String, String, RowLead)],
        page: usize,
        pinned_action: bool,
    ) -> (ScreenBuilder, usize) {
        let labels = rows
            .iter()
            .map(|(_, title, detail, _)| (title.as_str(), detail.as_str()))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows_under(
            &labels,
            pinned_action,
            kobo_sdk::Position::AtTheFoot,
            &screen.clone().build(),
        );
        let count = pages.len().max(1);
        let page = page.min(count - 1);
        let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen.rows(visible.iter().map(|&index| rows[index].clone()));
        if count > 1 {
            screen = screen
                .page_turns("list-previous", "list-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(count).unwrap_or(u16::MAX),
                );
        }
        (screen, count)
    }

    fn projects_screen(&self, context: &Context) -> (Screen, usize) {
        let rows = self
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| {
                (
                    format!("project-{index}"),
                    context.clamped_row(&project.name, 2, false),
                    format!(
                        "{} - row {}",
                        project
                            .sections
                            .get(project.section)
                            .map_or("", String::as_str),
                        project.counters[project.section].row
                    ),
                    RowLead::Number(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                )
            })
            .collect::<Vec<_>>();
        let (screen, pages) = Self::paged_rows(
            context,
            ScreenBuilder::new("needles-projects").top_bar("Projects"),
            &rows,
            self.project_page,
            false,
        );
        (screen.build(), pages)
    }

    /// The synced collections, a page at a time under the tabs.
    ///
    /// Held sideways at the larger text sizes, a refused key's banner and
    /// the tabs leave less room than one pattern of two lines and its
    /// designer, so the titles are cut to one line, and then the designers
    /// go, before a pattern is pushed off the panel. The banner keeps its
    /// whole sentence, since it is what says how to fix the sync.
    fn library(&self, context: &Context) -> (Screen, usize) {
        let metrics = context.metrics();
        let mut shaped = None;
        for (lines, designers) in [(2, true), (1, true), (1, false)] {
            let (screen, pages) = self.library_with(context, lines, designers);
            let fits = !screen
                .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                .has_errors();
            shaped = Some((screen, pages));
            if fits {
                break;
            }
        }
        shaped.unwrap_or_else(|| self.library_with(context, 1, false))
    }

    fn library_with(&self, context: &Context, lines: usize, designers: bool) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("needles-library").top_bar("Library");
        let problem = self
            .sync_problem
            .as_ref()
            .filter(|(collection, _)| *collection == self.collection)
            .map(|(_, problem)| problem);
        if let Some(problem) = problem {
            // No sign-in on the banner: the key is installed from a computer,
            // and the problem's own words name the command.
            screen = screen.problem_banner_with(problem, "sync", None);
        } else if let Some((level, note)) = &self.notice {
            screen = screen.banner(*level, note);
        }
        // Where the banner carries Try again, Sync at the foot would be a
        // second control for the same job, so it gives way to the banner.
        let retry_on_banner = problem.is_some_and(offers_try_again);
        let selected = self.collection.index();
        let mut pages = 1;
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
                let rows = self.libraries[selected]
                    .iter()
                    .enumerate()
                    .map(|(index, pattern)| {
                        (
                            format!("pattern-{index}"),
                            context.clamped_row(&pattern.title, lines, true),
                            if designers {
                                pattern.detail.clone()
                            } else {
                                String::new()
                            },
                            RowLead::Icon(Glyph::Bookmark),
                        )
                    })
                    .collect::<Vec<_>>();
                (screen, pages) = Self::paged_rows(
                    context,
                    screen,
                    &rows,
                    self.collection_pages[selected],
                    !retry_on_banner,
                );
            }
        } else {
            screen = screen.skeleton(5);
        }
        let sync = self.task.as_ref().map_or_else(
            || format!("Sync {}", self.collection.title()),
            |(_, collection)| format!("Syncing {}…", collection.title()),
        );
        if retry_on_banner {
            return (screen.build(), pages);
        }
        (screen.bottom_action("sync", sync).build(), pages)
    }

    /// The patterns sent from the computer, newest last, each saying
    /// whether it has been opened, since that is where it reopens.
    fn patterns_screen(&self, context: &Context) -> (Screen, usize) {
        let mut screen = ScreenBuilder::new("needles-patterns").top_bar("Patterns");
        if let Some((level, note)) = &self.notice {
            screen = screen.banner(*level, note);
        }
        if self.shelved.is_empty() {
            return (Self::no_patterns(context, &screen), 1);
        }
        let rows = self
            .shelved
            .iter()
            .enumerate()
            .map(|(index, pattern)| {
                let opened = self
                    .positions
                    .has_read(&position_id(&pattern.file))
                    .unwrap_or(false);
                (
                    format!("shelved-{index}"),
                    context.clamped_row(&pattern.title, 2, false),
                    if opened {
                        "Opens where you left it".to_owned()
                    } else {
                        "Not opened yet".to_owned()
                    },
                    RowLead::Icon(Glyph::Note),
                )
            })
            .collect::<Vec<_>>();
        let (screen, pages) = Self::paged_rows(context, screen, &rows, self.patterns_page, false);
        (screen.build(), pages)
    }

    /// No pattern on this Kobo yet: what to run on the computer, and Check
    /// for patterns directly under the command, the shape Vault and Music
    /// Stand use. Where a notice leaves no room, the mark goes first and
    /// then the sentence; the heading, the command and the check stay.
    fn no_patterns(context: &Context, screen: &ScreenBuilder) -> Screen {
        let with = |mark: Option<Glyph>, sentence: &str| {
            screen
                .clone()
                .splash(mark, "No patterns yet", sentence)
                .section("On your computer")
                .command("kobo needles push PATTERN.pdf")
                .button("check-patterns", "Check for patterns")
                .build()
        };
        let sentence = "Send a pattern you own from a computer.";
        let metrics = context.metrics();
        [(Some(Glyph::Note), sentence), (None, sentence), (None, "")]
            .into_iter()
            .map(|(mark, sentence)| with(mark, sentence))
            .find(|candidate| {
                !candidate
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
            })
            .unwrap_or_else(|| with(None, ""))
    }

    fn reading_title(&self) -> &str {
        self.reading
            .as_ref()
            .and_then(|file| self.shelved.iter().find(|pattern| &pattern.file == file))
            .map_or("Pattern", |pattern| pattern.title.as_str())
    }

    /// Reads the shelf again: what is on it, and what the push index calls
    /// it. Done on start and on every wake, so a push shows up without a
    /// restart.
    fn refresh_shelf(&mut self, context: &mut Context) {
        context.shelf().list();
        if self.index_load.is_none() {
            let mut load = ShelfDownload::new(PATTERN_INDEX).at_most(MAX_INDEX);
            load.start(context);
            self.index_load = Some(load);
        }
    }

    fn shelf_listed(&mut self, items: &[(String, u32)]) {
        self.listed = items
            .iter()
            .map(|(name, _)| name.clone())
            .filter(|name| is_pattern_file(name))
            .collect();
        self.arrange();
    }

    /// The listed patterns in the push index's order with its titles, then
    /// any it does not name. Either answer can arrive first.
    fn arrange(&mut self) {
        let mut files = self.listed.clone();
        // The order the push index gives, then anything it does not know.
        files.sort_by_key(|file| {
            (
                self.titles
                    .iter()
                    .position(|(known, _)| known == file)
                    .unwrap_or(usize::MAX),
                file.clone(),
            )
        });
        self.shelved = files
            .into_iter()
            .map(|file| Shelved {
                title: self
                    .titles
                    .iter()
                    .find(|(known, _)| *known == file)
                    .map_or_else(|| title_from_file(&file), |(_, title)| title.clone()),
                file,
            })
            .collect();
    }

    fn read_index(&mut self, bytes: &[u8]) {
        self.titles = std::str::from_utf8(bytes)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| {
                let (file, title) = line.split_once('\t')?;
                let title = title.trim();
                (is_pattern_file(file) && !title.is_empty())
                    .then(|| (file.to_owned(), title.chars().take(80).collect()))
            })
            .collect();
        self.arrange();
    }

    /// Opens the one pattern there is, or the list when there are several.
    fn choose_pattern(&mut self, context: &mut Context) {
        if let [only] = self.shelved.as_slice() {
            let file = only.file.clone();
            self.open_pattern(context, file);
        } else {
            self.patterns_page = 0;
            self.route = Route::Patterns;
            self.notice = None;
        }
    }

    /// Leaves the pattern being read, keeping its page for next time.
    fn close_pattern(&mut self, context: &mut Context) {
        self.keep_position(context);
        self.book.close(context);
        self.reading = None;
        self.charts.clear();
        self.chart = None;
        context.device().allow_sleep();
    }

    /// Keeps the open pattern's page, so it reopens there.
    fn keep_position(&mut self, context: &mut Context) {
        if let (Some(file), Some(memory)) = (&self.reading, self.book.memory()) {
            self.positions
                .keep(context, position_id(file), memory.clone());
        }
    }

    /// Clears the count of the section being worked, keeping it for Undo.
    fn restart_section(&mut self, context: &mut Context) {
        let current = self.current;
        let project = &mut self.projects[current];
        let section = project.section;
        let old = project.counters[section].clone();
        if old.row == 0 {
            self.notice = Some((
                BannerLevel::Info,
                "This section is already at row 0.".to_owned(),
            ));
            return;
        }
        project.counters[section] = Counter {
            repeat_total: old.repeat_total,
            ..Counter::default()
        };
        self.notice = Some((
            BannerLevel::Info,
            format!(
                "{} starts again at row 0. Undo brings back row {}.",
                project
                    .sections
                    .get(section)
                    .map_or("This section", String::as_str),
                old.row
            ),
        ));
        self.restarted = Some((current, section, old));
        self.save(context);
    }

    fn turn_list(&mut self, context: &Context, forward: bool) {
        let count = match self.route {
            Route::Projects => self.projects_screen(context).1,
            Route::Library => self.library(context).1,
            Route::Patterns => self.patterns_screen(context).1,
            _ => return,
        };
        let page = match self.route {
            Route::Projects => &mut self.project_page,
            Route::Library => &mut self.collection_pages[self.collection.index()],
            Route::Patterns => &mut self.patterns_page,
            _ => return,
        };
        let last = count.saturating_sub(1);
        *page = if forward {
            page.saturating_add(1).min(last)
        } else {
            (*page).min(last).saturating_sub(1)
        };
    }

    /// A Ravelry pattern, to follow or to read.
    ///
    /// Held sideways at the larger text sizes, a banner over the pattern
    /// leaves no room for everything, so the sentence on sending a PDF goes
    /// first (Read a pattern leads to the command), and then the two
    /// buttons stand side by side. Both buttons stay at every size.
    fn pattern(&self, context: &Context) -> Screen {
        let metrics = context.metrics();
        [(true, false), (false, false)]
            .into_iter()
            .map(|(sentence, beside)| self.pattern_with(sentence, beside))
            .find(|screen| {
                !screen
                    .diagnostics(&metrics, &kobo_sdk::Chrome::measuring(true))
                    .has_errors()
            })
            .unwrap_or_else(|| self.pattern_with(false, true))
    }

    fn pattern_with(&self, sentence: bool, beside: bool) -> Screen {
        let title = self
            .selected
            .as_ref()
            .map_or("Pattern", |pattern| pattern.title.as_str());
        // The banner leads, as on every other screen: added after the
        // buttons, a failure landed under them.
        let mut screen = ScreenBuilder::new("needles-pattern").top_bar("Pattern");
        if let Some((level, note)) = &self.notice {
            screen = screen.banner(*level, note);
        }
        screen = screen.heading(title);
        if sentence {
            screen = screen.text(
                "To read it here, send a PDF you own from your computer with kobo needles push.",
            );
        }
        if !beside {
            return screen
                .primary_button("follow", "Follow this pattern")
                .button("read", "Read a pattern")
                .build();
        }
        screen
            .band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(|slot: ScreenBuilder| {
                            slot.primary_button("follow", "Follow this pattern")
                        })
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(|slot: ScreenBuilder| slot.button("read", "Read a pattern")),
                    ),
                ],
            )
            .build()
    }

    fn save(&self, context: &mut Context) {
        let mut out = format!("3\n{}", self.current);
        for project in &self.projects {
            let sections = project
                .sections
                .iter()
                .map(|section| hex(section))
                .collect::<Vec<_>>()
                .join("|");
            let counters = project
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
            out.push_str(&format!(
                "\n{}\t{}\t{}\t{}",
                hex(&project.name),
                project.section,
                sections,
                counters
            ));
        }
        context.store().save(STATE, out);
    }

    fn increment(&mut self, context: &mut Context) {
        self.restarted = None;
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
        if let Some((project, section, counter)) = self.restarted.take() {
            if project == self.current && section == self.projects[project].section {
                self.projects[project].counters[section] = counter;
                self.notice = None;
                self.save(context);
                return;
            }
        }
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
            self.notice = Some((
                BannerLevel::Info,
                "Already at row 0; nothing was undone.".to_owned(),
            ));
        }
    }

    /// The counter's choices: which section is being knitted, how long its
    /// repeat is, and which way up the counter is held. Says whether the
    /// action was one of them.
    fn counter_choice(&mut self, context: &mut Context, action: ActionId) -> bool {
        let picked_section = (0..self.projects[self.current].sections.len())
            .find(|index| action == action_id(&format!("section-{index}")))
            .filter(|_| self.route == Route::Sections);
        let picked_total = REPEATS
            .into_iter()
            .find(|rows| action == action_id(&format!("repeat-{rows}")))
            .filter(|_| self.route == Route::RepeatLength);
        if action == action_id("turn") {
            self.landscape = !self.landscape;
            context.store().save(
                TURN,
                if self.landscape {
                    "landscape"
                } else {
                    "portrait"
                },
            );
        } else if action == action_id("section") {
            self.open_sections(context);
        } else if action == action_id("sections-next") {
            self.turn_sections(context, true);
        } else if action == action_id("sections-previous") {
            self.turn_sections(context, false);
        } else if let Some(index) = picked_section {
            self.projects[self.current].section = index;
            self.restarted = None;
            self.notice = None;
            self.route = Route::Project;
            self.save(context);
        } else if action == action_id("repeat-total") {
            self.route = Route::RepeatLength;
            self.notice = None;
        } else if let Some(total) = picked_total {
            self.set_repeat_total(context, total);
            self.route = Route::Project;
        } else {
            return false;
        }
        true
    }

    /// Sets the repeat of the section being knitted. The place in the
    /// repeat stays where it is, unless the new repeat is shorter than the
    /// row reached, when it is the new repeat's last row.
    fn set_repeat_total(&mut self, context: &mut Context, total: u8) {
        let counter = self.counter_mut();
        counter.repeat_total = total;
        counter.repeat = counter.repeat.min(total);
        self.save(context);
    }

    fn sync(&mut self, context: &mut Context) {
        if self.task.is_some() {
            return;
        }
        let collection = self.collection;
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: collection.url().to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential: Some(Credential::basic("ravelry")),
            headers: Vec::new(),
        }) {
            self.task = Some((task, collection));
            self.sync_problem = None;
            self.notice = Some((
                BannerLevel::Info,
                format!(
                    "Reading your Ravelry {}.",
                    collection.title().to_lowercase()
                ),
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
        self.collection_pages[collection.index()] = 0;
        true
    }

    fn open_pattern(&mut self, context: &mut Context, file: String) {
        if self.loading.is_some() {
            return;
        }
        self.charts.clear();
        self.chart = None;
        let mut loading = ShelfDownload::new(file.clone()).at_most(MAX_PATTERN);
        loading.start(context);
        self.loading = Some(loading);
        self.reading = Some(file);
        self.notice = Some((
            BannerLevel::Info,
            "Opening the pattern sent from your computer.".to_owned(),
        ));
    }

    /// Reads the pattern's charts off the shelf one at a time. A chart only
    /// has bytes to read when the transfer brought them across under the same
    /// name, so anything that is not a shelf name -- a link out to the web --
    /// keeps its caption and no frame.
    fn load_next_chart(&mut self, context: &mut Context) {
        while let Some(name) = self.charts.pop_front() {
            if !is_valid_key(&name) {
                continue;
            }
            let mut download =
                ShelfDownload::new(&name).at_most(kobo_bookview::MAX_PICTURE_BYTES as usize);
            download.start(context);
            self.chart = Some((name, download));
            return;
        }
        self.chart = None;
        self.book.settle_pictures(context);
    }

    fn restore(&mut self, bytes: &[u8]) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        let mut fields = text.lines();
        match fields.next() {
            Some("3") => self.restore_projects(fields),
            Some("2") => self.restore_projects_v2(fields),
            // The first version kept one set of counters and the followed
            // pattern's name; that is one project with a little history.
            Some("1") => self.restore_alone(fields),
            _ => {}
        }
    }

    fn restore_projects<'a>(&mut self, mut fields: impl Iterator<Item = &'a str>) {
        let Some(Ok(current)) = fields.next().map(str::parse::<usize>) else {
            return;
        };
        let mut projects = Vec::new();
        for line in fields {
            let mut parts = line.split('\t');
            let (Some(name), Some(section), Some(sections), Some(counters)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                return;
            };
            let sections = sections
                .split('|')
                .map(unhex)
                .collect::<Option<Vec<_>>>()
                .filter(|sections| !sections.is_empty() && sections.len() <= MAX_SECTIONS);
            let (Some(name), Ok(section), Some(sections), Some(counters)) = (
                unhex(name),
                section.parse::<usize>(),
                sections,
                parse_counters(counters),
            ) else {
                return;
            };
            if name.is_empty() || section >= sections.len() || counters.len() != sections.len() {
                return;
            }
            projects.push(Project {
                name,
                sections,
                section,
                counters,
            });
        }
        if projects.is_empty() || current >= projects.len() {
            return;
        }
        self.projects = projects;
        self.current = current;
    }

    /// Version two kept the same three everyday sections on every project.
    fn restore_projects_v2<'a>(&mut self, mut fields: impl Iterator<Item = &'a str>) {
        let Some(Ok(current)) = fields.next().map(str::parse::<usize>) else {
            return;
        };
        let mut projects = Vec::new();
        for line in fields {
            let mut parts = line.split('\t');
            let (Some(name), Some(section), Some(counters)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return;
            };
            let (Some(name), Ok(section), Some(counters)) = (
                unhex(name),
                section.parse::<usize>(),
                parse_counters(counters),
            ) else {
                return;
            };
            if name.is_empty() || section >= SECTIONS.len() || counters.len() != SECTIONS.len() {
                return;
            }
            let mut project = Project::new(name);
            project.section = section;
            project.counters = counters;
            projects.push(project);
        }
        if projects.is_empty() || current >= projects.len() {
            return;
        }
        self.projects = projects;
        self.current = current;
    }

    fn restore_alone<'a>(&mut self, mut fields: impl Iterator<Item = &'a str>) {
        let (Some(section), Some(selected), Some(counters)) =
            (fields.next(), fields.next(), fields.next())
        else {
            return;
        };
        let Ok(section) = section.parse::<usize>() else {
            return;
        };
        if section >= SECTIONS.len() {
            return;
        }
        let Some(counters) =
            parse_counters(counters).filter(|counters| counters.len() == SECTIONS.len())
        else {
            return;
        };
        let name = unhex(selected)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "Row counter".to_owned());
        let mut project = Project::new(name);
        project.section = section;
        project.counters = counters;
        self.projects = vec![project];
        self.current = 0;
    }
}

impl Default for Needles {
    fn default() -> Self {
        Self {
            route: Route::Project,
            projects: vec![Project::new("Row counter")],
            current: 0,
            collection: Collection::Library,
            project_page: 0,
            collection_pages: [0; 3],
            libraries: std::array::from_fn(|_| Vec::new()),
            loaded: [false; 3],
            selected: None,
            notice: None,
            sync_problem: None,
            sections_page: 0,
            task: None,
            loading: None,
            charts: VecDeque::new(),
            chart: None,
            book: BookView::new(),
            shelved: Vec::new(),
            titles: Vec::new(),
            listed: Vec::new(),
            index_load: None,
            patterns_page: 0,
            reading: None,
            positions: Progress::default(),
            restarted: None,
            menu_open: false,
            landscape: false,
            sent_orientation: Cell::new(None),
        }
    }
}

/// Whether a Ravelry problem's banner carries Try again, as
/// `problem_banner_with` draws it with no sign-in: a reader off the Wi-Fi is
/// offered Join Wi-Fi instead, and a refused or missing key nothing, since
/// asking again would fail the same way until the key is fixed.
fn offers_try_again(problem: &Problem) -> bool {
    problem.retryable && problem.cause != Cause::Offline
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

/// A transferred pattern's own outline: its title heading and the sections
/// its counting should follow. Second-level headings are the sections when
/// the pattern has that much structure; a flat pattern counts by its
/// top-level headings past the title. A title alone is not a section list.
fn parse_pattern(markdown: &[u8]) -> (Option<String>, Vec<String>) {
    let Ok(text) = std::str::from_utf8(markdown) else {
        return (None, Vec::new());
    };
    let headings = text
        .lines()
        .filter_map(|line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            if hashes == 0 || hashes > 6 {
                return None;
            }
            let heading = line[hashes..].trim().trim_end_matches('#').trim();
            (!heading.is_empty()).then(|| (hashes, heading.chars().take(60).collect::<String>()))
        })
        .collect::<Vec<_>>();
    let title = headings
        .iter()
        .find(|(level, _)| *level == 1)
        .map(|(_, heading)| heading.clone());
    let at_level = |level: usize| {
        headings
            .iter()
            .filter(|(here, _)| *here == level)
            .map(|(_, heading)| heading.clone())
            .collect::<Vec<_>>()
    };
    let mut sections = if headings.iter().any(|(level, _)| *level == 2) {
        at_level(2)
    } else {
        let mut flat = at_level(1);
        if let Some(title) = &title {
            flat.retain(|heading| heading != title);
        }
        if flat.len() > 1 {
            flat
        } else {
            Vec::new()
        }
    };
    let mut seen = Vec::<String>::new();
    sections.retain(|heading| {
        let fresh = !seen.contains(heading);
        seen.push(heading.clone());
        fresh
    });
    sections.truncate(MAX_SECTIONS);
    (title, sections)
}

/// The counters as saved, or nothing when any one is off.
fn parse_counters(text: &str) -> Option<Vec<Counter>> {
    text.split('|')
        .map(|counter| {
            let mut parts = counter.split(',');
            let (Some(row), Some(repeat), Some(total)) = (parts.next(), parts.next(), parts.next())
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
        .collect::<Option<Vec<_>>>()
}

fn is_pattern_file(name: &str) -> bool {
    name == PATTERN_BLOB
        || name
            .strip_prefix(PATTERN_PREFIX)
            .and_then(|rest| rest.strip_suffix(".md"))
            .is_some_and(|stem| !stem.is_empty())
}

/// A readable name for a pattern the push index does not list, from its
/// file name: `pattern-winter-socks.md` reads "Winter socks".
fn title_from_file(file: &str) -> String {
    if file == PATTERN_BLOB {
        return "Pattern".to_owned();
    }
    let words = file
        .trim_start_matches(PATTERN_PREFIX)
        .trim_end_matches(".md")
        .replace(['-', '_'], " ");
    let mut letters = words.trim().chars();
    letters.next().map_or_else(
        || "Pattern".to_owned(),
        |first| first.to_uppercase().chain(letters).collect(),
    )
}

/// The key a pattern's reading position is kept under.
fn position_id(file: &str) -> String {
    kobo_bookview::positions::identity(&format!("needles:{file}"))
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
        context.store().load(POSITIONS);
        context.store().load(TURN);
        self.refresh_shelf(context);
        self.show(context);
    }

    /// A pattern pushed while the reader slept is on the shelf when it
    /// wakes, so the list is read again.
    fn on_resume(&mut self, context: &mut Context) {
        self.refresh_shelf(context);
    }

    fn on_foreground(&mut self, context: &mut Context) {
        self.refresh_shelf(context);
        self.show(context);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.keep_position(context);
    }

    /// Shelf answers carry the file they are for, so the index read is
    /// routed by name and a refusal for it cannot fail a pattern opening.
    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if name == PATTERN_INDEX {
            let Some(load) = &mut self.index_load else {
                return;
            };
            match load.advance(context, &result) {
                ShelfProgress::Done => {
                    let bytes = self.index_load.take().expect("index read").take();
                    self.read_index(&bytes);
                    self.show(context);
                }
                ShelfProgress::Failed(_) => self.index_load = None,
                ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => {}
            }
            return;
        }
        self.on_store(context, result);
    }

    #[allow(clippy::too_many_lines)]
    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Shelf(items) = &result {
            self.shelf_listed(items);
            self.show(context);
            return;
        }
        if matches!(&result, StoreResult::Loaded { key, .. } if key == POSITIONS) {
            self.positions.load(result);
            self.show(context);
            return;
        }
        if matches!(&result, StoreResult::Saved { key } if key == POSITIONS) {
            self.positions.saved(context, result);
            return;
        }
        if let StoreResult::Loaded { key, value } = &result {
            if key == STATE {
                if let Some(value) = value {
                    self.restore(value);
                }
            } else if key == TURN {
                self.landscape = value.as_deref() == Some(b"landscape".as_slice());
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
                    let file = loading.name().to_owned();
                    let bytes = loading.take();
                    let memory = self.positions.memory(&position_id(&file));
                    if self.book.open_bytes(context, &file, &bytes, memory).is_ok() {
                        // The pattern being read is the work being
                        // counted: its own sections take over the project
                        // it belongs to, named after its title heading.
                        let (title, sections) = parse_pattern(&bytes);
                        let counting = title.map_or(self.current, |title| self.project_for(&title));
                        self.current = counting;
                        if !sections.is_empty() {
                            self.projects[counting].adopt_sections(sections);
                        }
                        self.save(context);
                        self.charts = self.book.missing_pictures().into_iter().collect();
                        self.load_next_chart(context);
                        self.route = Route::Reading;
                        self.notice = None;
                    } else {
                        self.reading = None;
                        self.notice = Some((BannerLevel::Error, PATTERN_UNREADABLE.to_owned()));
                    }
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.reading = None;
                    self.notice = Some((BannerLevel::Error, PATTERN_GONE.to_owned()));
                    context.shelf().list();
                }
                ShelfProgress::Elsewhere | ShelfProgress::Moving { .. } => {}
            }
        }

        if let Some((_, download)) = &mut self.chart {
            match download.advance(context, &result) {
                ShelfProgress::Done => {
                    let Some((name, download)) = self.chart.take() else {
                        self.show(context);
                        return;
                    };
                    self.book.provide_picture(&name, download.take());
                    self.load_next_chart(context);
                }
                // A chart that never made it across keeps its caption rather
                // than holding the page up.
                ShelfProgress::Failed(_) => {
                    self.chart = None;
                    self.load_next_chart(context);
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
                    self.close_pattern(context);
                    self.route = Route::Project;
                }
                self.show(context);
                return;
            }
        }
        // Any tap with the menu open puts it away; Back, the tap beside it,
        // does nothing else.
        let menu_was_open = std::mem::take(&mut self.menu_open);
        if action == ActionId::BACK && menu_was_open {
            self.show(context);
            return;
        }
        if action == action_id("more") {
            self.menu_open = !menu_was_open;
        } else if self.counter_choice(context, action) {
            // The section or the repeat length, asked and answered.
        } else if action == ActionId::BACK {
            if self.route == Route::Reading {
                self.close_pattern(context);
            }
            self.route = if self.route == Route::Pattern {
                Route::Library
            } else {
                Route::Project
            };
            self.notice = None;
        } else if action == action_id("list-next") {
            self.turn_list(context, true);
        } else if action == action_id("list-previous") {
            self.turn_list(context, false);
        } else if action == action_id("plus") {
            self.increment(context);
        } else if action == action_id("undo") {
            self.undo(context);
        } else if action == action_id("projects") {
            self.route = Route::Projects;
        } else if action == action_id("check-patterns") {
            self.refresh_shelf(context);
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
            if let Some(pattern) = self.selected.clone() {
                self.current = self.project_for(&pattern.title);
            }
            self.route = Route::Project;
            context.device().keep_awake(Duration::from_secs(14_400));
            self.save(context);
        } else if action == action_id("read") {
            self.choose_pattern(context);
        } else if action == action_id("restart-section") {
            self.restart_section(context);
        } else if let Some(index) =
            (0..self.shelved.len()).find(|index| action == action_id(&format!("shelved-{index}")))
        {
            let file = self.shelved[index].file.clone();
            self.open_pattern(context, file);
        } else if let Some(index) =
            (0..self.projects.len()).find(|index| action == action_id(&format!("project-{index}")))
        {
            self.current = index;
            self.route = Route::Project;
            self.notice = None;
            self.save(context);
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

    fn on_page_turn(&mut self, context: &mut Context, forward: bool) {
        if self.route == Route::Reading {
            let action = if forward {
                kobo_read::action::FORWARD
            } else {
                kobo_read::action::BACK
            };
            let _ = self.book.act(context, action_id(action));
            self.show(context);
            return;
        }
        if matches!(
            self.route,
            Route::Projects | Route::Library | Route::Patterns
        ) {
            self.turn_list(context, forward);
            self.show(context);
        } else if self.route == Route::Sections {
            self.turn_sections(context, forward);
            self.show(context);
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.book.woke(context, task, &outcome) != Step::Elsewhere {
            self.show(context);
            return;
        }
        let Some((_, collection)) = self.task.take_if(|(known, _)| *known == task) else {
            return;
        };
        // A failed sync is told in Ravelry's terms: a refused or missing key
        // is an error whose fix is the computer command, and a server that
        // did not answer offers Try again on the banner that says so.
        self.sync_problem = RAVELRY
            .problem(&outcome)
            .map(|problem| (collection, problem));
        self.notice = match outcome {
            _ if self.sync_problem.is_some() => None,
            TaskOutcome::Completed(bytes) if self.read_library(&bytes, collection) => {
                context.store().save(collection.key(), bytes);
                None
            }
            TaskOutcome::Completed(_) => Some((
                BannerLevel::Error,
                format!(
                    "Ravelry returned a {} this version cannot read.",
                    collection.title().to_lowercase()
                ),
            )),
            TaskOutcome::Cancelled => Some((
                BannerLevel::Info,
                "The library sync was cancelled.".to_owned(),
            )),
            TaskOutcome::Failed(_) => None,
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
    use super::{
        hex, parse_pattern, pattern_from, Collection, Counter, Needles, Route, PATTERN_GONE,
        PATTERN_UNREADABLE, SECTIONS,
    };
    use kobo_sdk::{action_id, ActionId, AppRunner, BannerLevel, Context, KoboApp, StoreResult};
    use kobo_ui::{Chrome, DisplayMetrics, Orientation, TextScale, CLARA_BW_METRICS};

    /// A 300 ppi Clara at `text_scale`, held `orientation`.
    fn clara(text_scale: TextScale, orientation: Orientation) -> DisplayMetrics {
        DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        }
        .oriented(orientation)
    }

    const HELD: [Orientation; 2] = [Orientation::Portrait, Orientation::Landscape];

    #[test]
    fn counter_repeats_undoes_and_keeps_section_totals() {
        let mut app = Needles {
            projects: vec![super::Project {
                name: "Row counter".to_owned(),
                sections: SECTIONS
                    .iter()
                    .map(|section| (*section).to_owned())
                    .collect(),
                section: 0,
                counters: vec![
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
            }],
            ..Needles::default()
        };
        let mut context = Context::default();
        app.increment(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (12, 12));
        app.increment(&mut context);
        assert_eq!(app.counter().repeat, 1);
        app.undo(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (12, 12));
        app.projects[0].section = 1;
        app.increment(&mut context);
        assert_eq!((app.counter().row, app.counter().repeat), (1, 1));
        assert_eq!(app.projects[0].counters[0].row, 12);
    }

    #[test]
    fn state_round_trips_projects_and_all_sections() {
        let mut app = Needles::default();
        app.projects[0].counters[0].row = 42;
        app.projects[0].section = 1;
        app.projects.push(super::Project::new("Warm sweater"));
        app.projects[1].counters[2] = Counter {
            row: 7,
            repeat: 7,
            repeat_total: 8,
        };
        app.current = 1;
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
        assert_eq!(restored.current, 1);
        assert_eq!(restored.projects.len(), 2);
        assert_eq!(restored.projects[0].section, 1);
        assert_eq!(restored.projects[0].counters[0].row, 42);
        assert_eq!(restored.projects[1].name, "Warm sweater");
        assert_eq!(restored.projects[1].counters[2].repeat_total, 8);
    }

    #[test]
    fn state_round_trips_imported_sections() {
        let mut app = Needles::default();
        app.projects[0].adopt_sections(
            ["Cuff", "Leg", "Heel", "Foot"]
                .iter()
                .map(|section| (*section).to_owned())
                .collect(),
        );
        app.projects[0].counters[1].row = 30;
        app.projects[0].section = 1;
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
        assert_eq!(
            restored.projects[0].sections,
            ["Cuff", "Leg", "Heel", "Foot"]
        );
        assert_eq!(restored.projects[0].section, 1);
        assert_eq!(restored.projects[0].counters[1].row, 30);
    }

    #[test]
    fn the_projects_version_of_the_state_keeps_the_everyday_sections() {
        // As saved before imported sections: hex of the name, the section,
        // then its three counters.
        let legacy = format!("2\n0\n{}\t1\t42,0,12|7,7,8|0,0,4", hex("Warm sweater"));
        let mut restored = Needles::default();
        restored.on_store(
            &mut Context::default(),
            StoreResult::Loaded {
                key: super::STATE.to_owned(),
                value: Some(legacy.into()),
            },
        );
        assert_eq!(restored.projects.len(), 1);
        assert_eq!(restored.projects[0].name, "Warm sweater");
        assert_eq!(restored.projects[0].section, 1);
        assert_eq!(restored.projects[0].sections, SECTIONS);
        assert_eq!(restored.projects[0].counters[0].row, 42);
    }

    #[test]
    fn an_imported_patterns_sections_become_the_projects() {
        let markdown = b"# Winter socks\n\nCast on.\n\n## Cuff\n\nWork 12 rows.\n\n## Leg\n\n## Heel\n\n## Foot\n";
        let (title, sections) = parse_pattern(markdown);
        assert_eq!(title.as_deref(), Some("Winter socks"));
        assert_eq!(sections, ["Cuff", "Leg", "Heel", "Foot"]);

        let mut project = super::Project::new("Winter socks");
        project.counters[0].row = 14;
        project.adopt_sections(sections.clone());
        // None of the everyday names survive, so every section starts fresh.
        assert_eq!(project.sections, ["Cuff", "Leg", "Heel", "Foot"]);
        assert_eq!(project.counters[0].row, 0);
        // Reading the same pattern again keeps the counts it already has.
        project.counters[1].row = 30;
        project.adopt_sections(sections);
        assert_eq!(project.counters[1].row, 30);
    }

    #[test]
    fn a_flat_pattern_counts_by_its_headings_past_the_title() {
        let (title, sections) = parse_pattern(b"# Dishcloth\n\n# Body\n\n# Edging\n");
        assert_eq!(title.as_deref(), Some("Dishcloth"));
        assert_eq!(sections, ["Body", "Edging"]);
        // A title alone is not a section list.
        assert_eq!(
            parse_pattern(b"# Just a title\n\nPlain rows.\n").1,
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_patterns_charts_come_off_the_shelf_by_name() {
        let mut app = Needles::default();
        let mut context = Context::default();
        let document = kobo_doc::markdown::parse(
            "# Winter socks\n\n![Main chart](chart-main.png)\n\n![Web chart](https://shared.example/chart.png)\n",
        );
        app.book
            .open(&mut context, document, kobo_read::Memory::default());
        app.charts = app.book.missing_pictures().into_iter().collect();
        app.load_next_chart(&mut context);
        // The shelf-named chart starts its transfer; the web address waits
        // its turn.
        let Some((name, _)) = &app.chart else {
            panic!("the named chart is being read");
        };
        assert_eq!(name, "chart-main.png");
        // When it lands, the web address has no bytes on the shelf: it is
        // passed over and keeps its caption, and the queue drains.
        let bytes = b"not really a png".to_vec();
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        app.on_store(
            &mut context,
            StoreResult::ShelfRead {
                name: "chart-main.png".to_owned(),
                offset: 0,
                bytes,
                size,
            },
        );
        assert!(app.chart.is_none());
        assert!(app.charts.is_empty());
    }

    #[test]
    fn every_pushed_pattern_is_listed_by_its_title_in_push_order() {
        let mut app = Needles::default();
        let mut context = Context::default();
        app.read_index(b"pattern-winter-socks.md\tWinter Socks\npattern-lace.md\tLace Shawl\n");
        app.on_store(
            &mut context,
            StoreResult::Shelf(vec![
                ("chart-main.png".to_owned(), 3),
                ("pattern-lace.md".to_owned(), 10),
                ("pattern-new-hat.md".to_owned(), 9),
                ("pattern-winter-socks.md".to_owned(), 10),
                ("pattern.md".to_owned(), 5),
            ]),
        );
        let titles = app
            .shelved
            .iter()
            .map(|pattern| pattern.title.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Winter Socks", "Lace Shawl", "New hat", "Pattern"]);
        app.on_action(&mut context, action_id("read"));
        assert_eq!(app.route, Route::Patterns);
    }

    fn deliver(app: &mut Needles, context: &mut Context, file: &str, bytes: &[u8]) {
        app.open_pattern(context, file.to_owned());
        let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        app.on_store(
            context,
            StoreResult::ShelfRead {
                name: file.to_owned(),
                offset: 0,
                bytes: bytes.to_vec(),
                size,
            },
        );
    }

    #[test]
    fn a_pattern_reopens_at_the_page_it_was_left_on() {
        let mut app = Needles::default();
        let mut context = Context::default();
        app.positions.load(StoreResult::Loaded {
            key: super::POSITIONS.to_owned(),
            value: None,
        });
        let mut long = String::from("# Long cardigan\n\n");
        for row in 1..=400 {
            long.push_str(&format!(
                "Row {row}: knit to the marker, slip it, purl two together.\n\n"
            ));
        }
        deliver(
            &mut app,
            &mut context,
            "pattern-long-cardigan.md",
            long.as_bytes(),
        );
        assert_eq!(app.route, Route::Reading);
        let start = app.book.memory().cloned();
        for _ in 0..3 {
            app.on_page_turn(&mut context, true);
        }
        let left = app.book.memory().cloned();
        assert_ne!(left, start);
        app.keep_position(&mut context);
        app.book.close(&mut context);
        app.route = Route::Project;
        deliver(
            &mut app,
            &mut context,
            "pattern-long-cardigan.md",
            long.as_bytes(),
        );
        assert_eq!(app.book.memory().cloned(), left);
    }

    #[test]
    fn starting_a_section_again_clears_its_count_and_undo_brings_it_back() {
        let mut app = Needles::default();
        let mut context = Context::default();
        for _ in 0..42 {
            app.increment(&mut context);
        }
        app.on_action(&mut context, action_id("restart-section"));
        assert_eq!(app.counter().row, 0);
        assert!(app
            .notice
            .as_ref()
            .is_some_and(|(level, notice)| *level == BannerLevel::Info
                && notice.contains("Undo brings back row 42")));
        app.on_action(&mut context, action_id("undo"));
        assert_eq!(app.counter().row, 42);
        app.on_action(&mut context, action_id("restart-section"));
        app.on_action(&mut context, action_id("plus"));
        app.on_action(&mut context, action_id("undo"));
        assert_eq!(app.counter().row, 0);
    }

    #[test]
    fn the_first_version_of_the_state_becomes_one_project() {
        // As saved before projects: section, hex of the followed pattern,
        // then the three counters.
        let legacy = format!("1\n1\n{}\n42,0,12|7,7,8|0,0,4", hex("Warm sweater"));
        let mut restored = Needles::default();
        restored.on_store(
            &mut Context::default(),
            StoreResult::Loaded {
                key: super::STATE.to_owned(),
                value: Some(legacy.into()),
            },
        );
        assert_eq!(restored.projects.len(), 1);
        assert_eq!(restored.projects[0].name, "Warm sweater");
        assert_eq!(restored.projects[0].section, 1);
        assert_eq!(restored.projects[0].counters[0].row, 42);
        assert_eq!(restored.projects[0].counters[1].repeat_total, 8);
    }

    #[test]
    fn following_a_pattern_twice_keeps_one_project_and_its_count() {
        let mut app = Needles::default();
        let mut context = Context::default();
        app.selected = Some(super::Pattern {
            title: "Warm sweater".to_owned(),
            detail: "Ravelry pattern".to_owned(),
        });
        app.on_action(&mut context, action_id("follow"));
        assert_eq!(app.current, 1);
        for _ in 0..5 {
            app.on_action(&mut context, action_id("plus"));
        }
        // Following it again from the library rejoins the same project.
        app.on_action(&mut context, action_id("follow"));
        assert_eq!(app.projects.len(), 2);
        assert_eq!(app.counter().row, 5);
        // And the plain counter is still where it was.
        app.on_action(&mut context, action_id("projects"));
        app.on_action(&mut context, action_id("project-0"));
        assert_eq!(app.current, 0);
        assert_eq!(app.counter().row, 0);
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

    /// A notice over the counter used to push Undo and +1 row off the panel
    /// at the larger text sizes: restarting a section, the one place Undo
    /// matters most, was refused by the reader at the largest size. Held
    /// sideways the stacked counter was taller than a Clara at every size,
    /// the default included, until the count stood beside its controls.
    /// The pattern failures are errors, whose mark narrows the words.
    #[test]
    fn the_counter_keeps_undo_and_the_increment_under_any_notice_at_every_text_size_held_either_way(
    ) {
        let notices = [
            None,
            Some((
                BannerLevel::Info,
                "Leg starts again at row 0. Undo brings back row 24.",
            )),
            Some((BannerLevel::Error, PATTERN_GONE)),
            Some((BannerLevel::Error, PATTERN_UNREADABLE)),
        ];
        for text_scale in TextScale::STEPS {
            for orientation in HELD {
                // The runtime always reports the upright panel; the counter
                // turns because the owner asked it to.
                let metrics = clara(text_scale, orientation);
                let runner = AppRunner::with_metrics(
                    Needles::default(),
                    clara(text_scale, Orientation::Portrait),
                );
                for notice in notices {
                    let at = format!("{text_scale:?} {orientation:?} {notice:?}");
                    let app = Needles {
                        notice: notice.map(|(level, words)| (level, words.to_owned())),
                        landscape: orientation == Orientation::Landscape,
                        ..Needles::default()
                    };
                    let screen = app.screen(&runner.context());
                    let chrome = Chrome::measuring(true);
                    let issues = screen.diagnostics(&metrics, &chrome).issues;
                    assert!(issues.is_empty(), "{at}: {issues:?}");
                    let layout = screen.layout_with(&metrics, &chrome);
                    for action in ["undo", "plus", "section", "repeat-total", "restart-section"] {
                        assert!(
                            layout.rect_of_action(action_id(action)).is_some(),
                            "{at}: {action}"
                        );
                    }
                    let drawn = format!("{screen:?}");
                    for said in ["Row 0", "Body", "ready for 1 of 12"] {
                        assert!(drawn.contains(said), "{at}: {said}");
                    }
                    if let Some((_, words)) = notice {
                        assert!(drawn.contains(words), "{at}");
                    }
                    if notice.is_none() {
                        assert!(
                            layout.rect_of_action(action_id("read")).is_some(),
                            "{at}: the shortcut to a pattern went with no notice"
                        );
                    }
                }
            }
        }
    }

    /// Changing section used to step to the next one, so reaching the
    /// eleventh of twelve was ten full-panel refreshes, and an overshoot
    /// went all the way round. It opens the sections instead, the one being
    /// knitted marked, and one tap picks any of them with its count kept.
    #[test]
    fn change_section_opens_the_sections_with_the_one_being_knitted_marked() {
        let mut app = Needles::default();
        app.projects[0].adopt_sections((1..=12).map(|n| format!("Part {n}")).collect());
        app.projects[0].counters[10].row = 30;
        let mut runner = AppRunner::new(app);
        runner.action(action_id("section"));
        assert_eq!(runner.app().route, Route::Sections);
        assert_eq!(runner.app().projects[0].section, 0, "opening changed it");
        let screen = runner.app().screen(&runner.context());
        let (count, selected) = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Options {
                    options, selected, ..
                } => Some((options.len(), *selected)),
                _ => None,
            })
            .expect("the sections as an option list");
        assert_eq!((count, selected), (12, Some(0)));
        assert!(format!("{screen:?}").contains("Row 30"));
        runner.action(action_id("section-10"));
        assert_eq!(runner.app().route, Route::Project);
        assert_eq!(runner.app().projects[0].section, 10);
        assert_eq!(runner.app().counter().row, 30);
        runner.action(action_id("section"));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().route, Route::Project);
        assert_eq!(runner.app().projects[0].section, 10, "Back changed it");
    }

    /// Repeat length used to cycle 4, 8, 12, 16: from row 10 of a 12-row
    /// repeat two taps went to 16 and then 4, and the place was lost with
    /// no Undo for it. Now the lengths are a choice and one tap sets one.
    #[test]
    fn a_repeat_length_is_chosen_in_one_tap_and_the_place_in_the_repeat_is_kept() {
        let mut app = Needles::default();
        app.projects[0].counters[0] = Counter {
            row: 22,
            repeat: 10,
            repeat_total: 12,
        };
        let mut runner = AppRunner::new(app);
        runner.action(action_id("repeat-total"));
        assert_eq!(runner.app().route, Route::RepeatLength);
        assert_eq!(
            runner.app().counter().repeat_total,
            12,
            "opening changed it"
        );
        let screen = runner.app().screen(&runner.context());
        let (count, selected) = screen
            .nodes
            .iter()
            .find_map(|node| match node {
                kobo_sdk::Node::Choice {
                    options, selected, ..
                } => Some((options.len(), *selected)),
                _ => None,
            })
            .expect("the lengths as a choice");
        assert_eq!((count, selected), (4, Some(2)));
        runner.action(action_id("repeat-16"));
        assert_eq!(runner.app().route, Route::Project);
        assert_eq!(
            *runner.app().counter(),
            Counter {
                row: 22,
                repeat: 10,
                repeat_total: 16,
            }
        );
        // Shorter than the row reached: the new repeat's last row.
        runner.action(action_id("repeat-total"));
        runner.action(action_id("repeat-8"));
        assert_eq!(runner.app().counter().repeat, 8);
        assert_eq!(runner.app().counter().row, 22);
    }

    /// Twelve sections, the most a pattern brings, are all reachable from
    /// the page that opens on the section being knitted, and the four
    /// lengths are on one screen, at every size held either way.
    #[test]
    fn every_section_and_repeat_length_is_reachable_at_every_text_size_held_either_way() {
        for text_scale in TextScale::STEPS {
            for orientation in HELD {
                let metrics = clara(text_scale, orientation);
                let at = format!("{text_scale:?} {orientation:?}");
                let mut app = Needles::default();
                app.projects[0].adopt_sections(
                    (1..=12)
                        .map(|n| format!("Section {n}: the yoke and its short rows"))
                        .collect(),
                );
                app.projects[0].section = 11;
                app.landscape = orientation == Orientation::Landscape;
                let mut runner =
                    AppRunner::with_metrics(app, clara(text_scale, Orientation::Portrait));
                runner.action(action_id("section"));
                let mut reached = std::collections::BTreeSet::new();
                for turn in 0..12 {
                    let screen = runner.app().screen(&runner.context());
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{at}: {:?}",
                        diagnostics.issues
                    );
                    if turn == 0 {
                        assert!(
                            diagnostics
                                .layout
                                .rect_of_action(action_id("section-11"))
                                .is_some(),
                            "{at}: the page opened without the section being knitted"
                        );
                    }
                    for index in 0..12 {
                        if diagnostics
                            .layout
                            .rect_of_action(action_id(&format!("section-{index}")))
                            .is_some()
                        {
                            reached.insert(index);
                        }
                    }
                    if runner.app().sections_page == 0 {
                        break;
                    }
                    runner.action(action_id("sections-previous"));
                }
                assert_eq!(reached, (0..12).collect(), "{at}");
                runner.action(ActionId::BACK);
                runner.action(action_id("repeat-total"));
                let screen = runner.app().screen(&runner.context());
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{at}: {:?}",
                    diagnostics.issues
                );
                for rows in super::REPEATS {
                    assert!(
                        diagnostics
                            .layout
                            .rect_of_action(action_id(&format!("repeat-{rows}")))
                            .is_some(),
                        "{at}: {rows}"
                    );
                }
            }
        }
    }

    /// The pattern screen's banner was added after its buttons, so a
    /// failure landed under them. It leads, as on every other screen, and
    /// the screen keeps both buttons at every size held either way.
    #[test]
    fn the_pattern_screen_leads_with_its_banner_at_every_text_size_held_either_way() {
        for text_scale in TextScale::STEPS {
            for orientation in HELD {
                let metrics = clara(text_scale, orientation);
                let at = format!("{text_scale:?} {orientation:?}");
                let runner = AppRunner::with_metrics(Needles::default(), metrics);
                let app = Needles {
                    route: Route::Pattern,
                    selected: Some(super::Pattern {
                        title: "Warm sweater with a folded collar".to_owned(),
                        detail: "Ravelry pattern".to_owned(),
                    }),
                    notice: Some((BannerLevel::Error, PATTERN_GONE.to_owned())),
                    ..Needles::default()
                };
                let screen = app.screen(&runner.context());
                let first = |wanted: fn(&kobo_sdk::Node) -> bool| {
                    screen.nodes.iter().position(wanted).expect("drawn")
                };
                let banner = first(|node| matches!(node, kobo_sdk::Node::Banner { .. }));
                let heading = first(|node| matches!(node, kobo_sdk::Node::Heading { .. }));
                assert!(banner < heading, "{at}: the banner is under the pattern");
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{at}: {:?}",
                    diagnostics.issues
                );
                for action in ["follow", "read"] {
                    assert!(
                        diagnostics
                            .layout
                            .rect_of_action(action_id(action))
                            .is_some(),
                        "{at}: {action}"
                    );
                }
            }
        }
    }

    /// With no pattern on the reader, Patterns says what to run on the
    /// computer under "On your computer" and offers Check for patterns
    /// directly under it, as Vault and Music Stand do, at every size held
    /// either way and under a notice. The check reads the shelf again.
    #[test]
    fn no_patterns_shows_the_command_on_your_computer_with_check_under_it_held_either_way() {
        for text_scale in TextScale::STEPS {
            for orientation in HELD {
                let metrics = clara(text_scale, orientation);
                for notice in [None, Some((BannerLevel::Error, PATTERN_GONE.to_owned()))] {
                    let at = format!("{text_scale:?} {orientation:?} {notice:?}");
                    let mut runner = AppRunner::with_metrics(
                        Needles {
                            route: Route::Patterns,
                            notice,
                            ..Needles::default()
                        },
                        metrics,
                    );
                    let screen = runner.app().screen(&runner.context());
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    assert!(
                        diagnostics.issues.is_empty(),
                        "{at}: {:?}",
                        diagnostics.issues
                    );
                    let drawn = format!("{screen:?}");
                    for said in [
                        "No patterns yet",
                        "On your computer",
                        "kobo needles push PATTERN.pdf",
                    ] {
                        assert!(drawn.contains(said), "{at}: {said}");
                    }
                    let command = screen
                        .nodes
                        .iter()
                        .position(|node| {
                            matches!(node, kobo_sdk::Node::Quote { text, .. }
                                if text.contains("kobo needles push"))
                        })
                        .expect("the command");
                    assert!(
                        matches!(
                            screen.nodes.get(command + 1),
                            Some(kobo_sdk::Node::Button { label, .. }) if label == "Check for patterns"
                        ),
                        "{at}: Check for patterns is not under the command"
                    );
                    assert!(
                        diagnostics
                            .layout
                            .rect_of_action(action_id("check-patterns"))
                            .is_some(),
                        "{at}"
                    );
                    let commands = runner.action(action_id("check-patterns"));
                    assert!(
                        commands.iter().any(|command| matches!(
                            command,
                            kobo_sdk::Command::Store(kobo_sdk::StoreRequest::ShelfList)
                        )),
                        "{at}: the shelf was not read again"
                    );
                }
            }
        }
    }

    /// The runtime keeps an app upright until it asks, so the counter's
    /// sideways shape is reached from More: Use landscape turns the counter,
    /// the choice is kept for the next start, and the lists the runtime
    /// pages upright are drawn upright. The menu fits over the counter
    /// either way up at every size.
    #[test]
    fn use_landscape_turns_the_counter_and_is_kept_for_the_next_start() {
        let turned = |commands: &[kobo_sdk::Command]| {
            commands.iter().rev().find_map(|command| match command {
                kobo_sdk::Command::SetOrientation(orientation) => Some(*orientation),
                _ => None,
            })
        };
        let menu = |runner: &AppRunner<Needles>| {
            runner
                .app()
                .screen(&runner.context())
                .overlay
                .map(|overlay| {
                    overlay
                        .nodes
                        .iter()
                        .filter_map(|node| match node {
                            kobo_sdk::Node::Button { label, .. } => Some(label.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let mut runner = AppRunner::new(Needles::default());
        assert_eq!(turned(&runner.start()), Some(Orientation::Portrait));
        runner.action(action_id("more"));
        assert_eq!(menu(&runner), ["Ravelry library", "Use landscape"]);
        let commands = runner.action(action_id("turn"));
        assert_eq!(turned(&commands), Some(Orientation::Landscape));
        assert!(commands.iter().any(|command| matches!(
            command,
            kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, value })
                if key == super::TURN && value.as_slice() == b"landscape"
        )));
        assert!(!runner.app().menu_open, "a choice from More closes it");
        runner.action(action_id("more"));
        assert_eq!(menu(&runner), ["Ravelry library", "Use portrait"]);
        let commands = runner.action(ActionId::BACK);
        assert!(!runner.app().menu_open, "a tap beside More closes it");
        assert_eq!(runner.app().route, Route::Project, "and nothing else");
        assert_eq!(turned(&commands), None);
        // A list is paged upright, so it is drawn upright, and the counter
        // turns again on the way back.
        assert_eq!(
            turned(&runner.action(action_id("projects"))),
            Some(Orientation::Portrait)
        );
        assert_eq!(
            turned(&runner.action(ActionId::BACK)),
            Some(Orientation::Landscape)
        );
        // The next start asks for the way up the owner chose.
        let mut next = AppRunner::new(Needles::default());
        next.start();
        let commands = next.store_result(StoreResult::Loaded {
            key: super::TURN.to_owned(),
            value: Some(b"landscape".to_vec()),
        });
        assert_eq!(turned(&commands), Some(Orientation::Landscape));
        for text_scale in TextScale::STEPS {
            for orientation in HELD {
                let at = format!("{text_scale:?} {orientation:?}");
                let app = Needles {
                    menu_open: true,
                    landscape: orientation == Orientation::Landscape,
                    ..Needles::default()
                };
                let runner = AppRunner::with_metrics(
                    Needles::default(),
                    clara(text_scale, Orientation::Portrait),
                );
                let screen = app.screen(&runner.context());
                // The scrim under an open menu is a fifth tone, a warning
                // every app with a menu shares; nothing may be cut or lost.
                let errors = screen
                    .diagnostics(&clara(text_scale, orientation), &Chrome::measuring(true))
                    .issues
                    .into_iter()
                    .filter(|issue| issue.severity == kobo_sdk::DiagnosticSeverity::Error)
                    .collect::<Vec<_>>();
                assert!(errors.is_empty(), "{at}: {errors:?}");
            }
        }
    }

    /// Progress and nudges are news, and a pattern that cannot be opened is
    /// a failure the owner has to act on, so they no longer look alike.
    #[test]
    fn progress_and_nudges_are_info_and_a_pattern_that_cannot_open_is_an_error() {
        let mut app = Needles::default();
        let mut context = Context::default();
        app.undo(&mut context);
        assert_eq!(
            app.notice.as_ref().map(|(level, _)| *level),
            Some(BannerLevel::Info)
        );
        app.restart_section(&mut context);
        assert_eq!(
            app.notice.as_ref().map(|(level, _)| *level),
            Some(BannerLevel::Info)
        );
        app.open_pattern(&mut context, "pattern-gone.md".to_owned());
        assert_eq!(
            app.notice.as_ref().map(|(level, _)| *level),
            Some(BannerLevel::Info)
        );
        app.on_store(
            &mut context,
            StoreResult::Denied(kobo_sdk::StoreError::Missing),
        );
        assert_eq!(
            app.notice,
            Some((BannerLevel::Error, PATTERN_GONE.to_owned()))
        );
    }

    #[test]
    fn undo_sits_beside_the_increment_it_reverses() {
        let app = Needles::default();
        let layout = app
            .project(&Context::default())
            .layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let plus = layout
            .rect_of_action(action_id("plus"))
            .expect("increment target");
        let undo = layout
            .rect_of_action(action_id("undo"))
            .expect("undo target");
        assert_eq!(
            plus.y, undo.y,
            "undo is not beside the increment: {plus:?} vs {undo:?}"
        );
    }

    #[test]
    fn the_row_count_is_the_biggest_thing_on_the_screen() {
        let mut app = Needles::default();
        let mut context = Context::default();
        for _ in 0..7 {
            app.on_action(&mut context, action_id("plus"));
        }
        let drawn = format!("{:?}", app.project(&Context::default()));
        assert!(
            drawn.contains("Heading") && drawn.contains("Row 7"),
            "the row count is not the heading: {drawn}"
        );
    }

    #[test]
    fn the_repeat_progress_is_drawn_not_only_said() {
        let mut app = Needles::default();
        let mut context = Context::default();
        // Six rows into a twelve-row repeat is half way.
        for _ in 0..6 {
            app.on_action(&mut context, action_id("plus"));
        }
        let drawn = format!("{:?}", app.project(&Context::default()));
        assert!(
            drawn.contains("Progress") && drawn.contains("50"),
            "the repeat progress is not drawn: {drawn}"
        );
    }

    #[test]
    fn counter_target_is_large_on_clara() {
        let app = Needles::default();
        let layout = app
            .project(&Context::default())
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
            .project(&Context::default())
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}

#[cfg(test)]
mod collection_tests;
