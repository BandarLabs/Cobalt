mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult,
};
#[cfg(test)]
use model::decode;
use model::{
    canonical_name, decode_with_blank_names, encode, Habit, Schedule, MAX_HABIT_NAME_CHARS,
};
use std::process::ExitCode;
const HABITS: &str = "habits-v1";
const ROWS_PER_PAGE: usize = 3;
const ACTION_NAME_CHARS: usize = 24;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Page {
    Today,
    Streaks,
    Manage,
    Stats,
    Settings,
}
impl Page {
    fn index(self) -> usize {
        match self {
            Self::Today => 0,
            Self::Streaks => 1,
            Self::Manage => 2,
            Self::Stats => 3,
            Self::Settings => 4,
        }
    }
    fn all() -> [(&'static str, &'static str); 4] {
        [
            ("today", "Today"),
            ("streaks", "Streaks"),
            ("manage", "Manage"),
            ("stats", "Stats"),
        ]
    }
}
struct Habits {
    items: Vec<Habit>,
    loaded: bool,
    page: Page,
    today_page: usize,
    manage_page: usize,
    streaks_page: usize,
    entry: TextEntry,
    notice: Option<String>,
    loading: bool,
    save_in_flight: bool,
    queued_save: Option<Vec<u8>>,
}
impl Default for Habits {
    fn default() -> Self {
        Self {
            items: vec![],
            loaded: false,
            page: Page::Today,
            today_page: 0,
            manage_page: 0,
            streaks_page: 0,
            entry: TextEntry::new().opened_by("add"),
            notice: None,
            loading: false,
            save_in_flight: false,
            queued_save: None,
        }
    }
}
impl Habits {
    fn day() -> u32 {
        u32::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                / 86_400,
        )
        .unwrap_or(u32::MAX)
    }
    fn save(&mut self, cx: &mut Context) {
        self.queued_save = Some(encode(&self.items));
        self.start_save(cx);
    }
    fn start_save(&mut self, cx: &mut Context) {
        if self.save_in_flight {
            return;
        }
        if let Some(value) = self.queued_save.take() {
            self.save_in_flight = true;
            cx.store().save(HABITS, value);
        }
    }
    fn page_bounds(page: usize, total: usize) -> (usize, usize, usize, usize) {
        let pages = total.div_ceil(ROWS_PER_PAGE);
        let page = Self::clamp_page(page, total);
        let start = page * ROWS_PER_PAGE;
        (start, (start + ROWS_PER_PAGE).min(total), page, pages)
    }
    fn clamp_page(page: usize, total: usize) -> usize {
        if total == 0 {
            0
        } else {
            page.min(total.div_ceil(ROWS_PER_PAGE) - 1)
        }
    }
    fn normalize_pages(&mut self) {
        let day = Self::day();
        let due = self
            .items
            .iter()
            .filter(|habit| !habit.archived && habit.due(day))
            .count();
        let streaks = self.items.iter().filter(|habit| !habit.archived).count();
        self.today_page = Self::clamp_page(self.today_page, due);
        self.manage_page = Self::clamp_page(self.manage_page, self.items.len());
        self.streaks_page = Self::clamp_page(self.streaks_page, streaks);
    }
    fn display_name(name: &str) -> String {
        Self::shortened_name(name, MAX_HABIT_NAME_CHARS)
    }
    fn action_name(name: &str) -> String {
        Self::shortened_name(name, ACTION_NAME_CHARS)
    }
    fn shortened_name(name: &str, maximum: usize) -> String {
        let mut display: String = name.chars().take(maximum).collect();
        if name.chars().nth(maximum).is_some() {
            display.pop();
            display.push('…');
        }
        display
    }
    fn paged(
        mut screen: ScreenBuilder,
        page: usize,
        pages: usize,
        previous: &str,
        next: &str,
    ) -> ScreenBuilder {
        if pages > 1 {
            let mut actions = Vec::new();
            if page > 0 {
                actions.push((previous, "Previous"));
            }
            if page + 1 < pages {
                actions.push((next, "More"));
            }
            screen = screen
                .secondary(format!("Page {} of {pages}", page + 1))
                .buttons(actions);
        }
        screen
    }
    fn back_target(&self) -> Option<Page> {
        match self.page {
            Page::Today => None,
            Page::Settings => Some(Page::Stats),
            Page::Streaks | Page::Manage | Page::Stats => Some(Page::Today),
        }
    }
    fn owns_back(&self) -> bool {
        self.entry.is_open() || self.back_target().is_some()
    }
    fn go_back(&mut self) {
        if self.entry.is_open() {
            self.entry.close();
        } else if let Some(page) = self.back_target() {
            self.page = page;
        }
    }
    fn show(&mut self, cx: &mut Context) {
        self.normalize_pages();
        cx.set_screen(self.screen().with_own_back(self.owns_back()));
    }
    #[allow(clippy::too_many_lines)]
    fn screen(&self) -> Screen {
        if self.entry.is_open() {
            return ScreenBuilder::new("hb-add")
                .top_bar("Habits")
                .secondary(format!("Use {MAX_HABIT_NAME_CHARS} characters or fewer."))
                .text_entry(&self.entry, "Habit name", "Add")
                .build();
        }
        let mut s = ScreenBuilder::new(match self.page {
            Page::Today => "hb-today",
            Page::Streaks => "hb-streaks",
            Page::Manage => "hb-manage",
            Page::Stats => "hb-stats",
            Page::Settings => "hb-settings",
        })
        .top_bar("Habits");
        if self.page != Page::Settings {
            s = s.tabs(self.page.index(), Page::all());
        }
        if !self.loaded {
            return s.skeleton(4).build();
        }
        if let Some(note) = &self.notice {
            s = s.banner(BannerLevel::Attention, note);
        }
        match self.page {
            Page::Today => {
                let day = Self::day();
                let due: Vec<_> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, h)| !h.archived && h.due(day))
                    .collect();
                if due.is_empty() {
                    s = s.splash(
                        Some(Glyph::Check),
                        "Nothing due",
                        "Add a habit, or return when one is due.",
                    );
                } else {
                    let (start, end, page, pages) = Self::page_bounds(self.today_page, due.len());
                    let visible = &due[start..end];
                    s = s
                        .checklist(visible.iter().map(|(i, h)| {
                            let done = h.done.contains(&day);
                            let skipped = h.skipped.contains(&day);
                            (
                                format!("done-{i}"),
                                Self::display_name(&h.name),
                                if done {
                                    "Done".to_owned()
                                } else if skipped {
                                    "Skipped".to_owned()
                                } else {
                                    h.schedule_label()
                                },
                                done,
                            )
                        }))
                        .buttons(
                            visible
                                .iter()
                                .filter(|(_, h)| {
                                    !h.done.contains(&day) && !h.skipped.contains(&day)
                                })
                                .map(|(i, h)| {
                                    (
                                        format!("skip-{i}"),
                                        format!("Skip {}", Self::action_name(&h.name)),
                                    )
                                })
                                .take(3),
                        );
                    if pages > 1 {
                        s = Self::paged(s, page, pages, "due-prev", "due-next");
                    }
                }
            }
            Page::Streaks => {
                let habits: Vec<_> = self.items.iter().filter(|h| !h.archived).collect();
                if habits.is_empty() {
                    s = s.splash(
                        Some(Glyph::Clock),
                        "No streaks yet",
                        "Add a habit to begin.",
                    );
                } else {
                    let (start, end, page, pages) =
                        Self::page_bounds(self.streaks_page, habits.len());
                    s = s.rows(habits[start..end].iter().enumerate().map(|(i, h)| {
                        (
                            format!("streak-{}", start + i),
                            Self::display_name(&h.name),
                            format!(
                                "{} current, {} best",
                                h.current_streak(Self::day()),
                                h.best_streak(Self::day())
                            ),
                            Glyph::Chart,
                        )
                    }));
                    s = Self::paged(s, page, pages, "streaks-prev", "streaks-next");
                }
            }
            Page::Manage => {
                let (start, end, page, pages) =
                    Self::page_bounds(self.manage_page, self.items.len().max(1));
                s = s.top_bar_action("add", "Add").rows(
                    self.items[start..end.min(self.items.len())]
                        .iter()
                        .enumerate()
                        .map(|(i, h)| {
                            (
                                format!("cycle-{}", start + i),
                                Self::display_name(&h.name),
                                format!(
                                    "{}{}",
                                    h.schedule_label(),
                                    if h.archived { "; archived" } else { "" }
                                ),
                                Glyph::Settings,
                            )
                        }),
                );
                s = Self::paged(s, page, pages, "manage-prev", "manage-next");
            }
            Page::Stats => {
                let completed: usize = self.items.iter().map(|h| h.done.len()).sum();
                s = s
                    .heading(format!("{completed} completions"))
                    .text("Best streaks are measured across scheduled days.")
                    .button("settings", "Settings");
            }
            Page::Settings => {
                s = s
                    .rows([(
                        "local",
                        "Stored on this reader",
                        "Works without network access.",
                        Glyph::Settings,
                    )])
                    .text("Habits never connect, upload, or back up your completions.");
            }
        }
        s.build()
    }
}
impl KoboApp for Habits {
    fn on_start(&mut self, cx: &mut Context) {
        self.loading = true;
        cx.store().load(HABITS);
        self.show(cx);
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        match result {
            StoreResult::Loaded { key, value } if key == HABITS => {
                let (items, ignored_blank_names) = value
                    .map(|value| decode_with_blank_names(&value))
                    .unwrap_or_default();
                self.items = items;
                self.loaded = true;
                self.loading = false;
                if ignored_blank_names > 0 {
                    self.notice = Some(format!(
                        "{ignored_blank_names} blank saved habit name(s) were ignored. Other habits remain editable."
                    ));
                }
            }
            StoreResult::Saved { key } if key == HABITS && self.save_in_flight => {
                self.save_in_flight = false;
                self.start_save(cx);
                if !self.save_in_flight {
                    self.notice = None;
                }
            }
            StoreResult::Denied(error) if self.save_in_flight => {
                self.save_in_flight = false;
                self.notice = Some(format!(
                    "A local change could not be saved: {error}. It will be lost if Habits closes."
                ));
                self.start_save(cx);
            }
            StoreResult::Denied(error) if self.loading => {
                self.loading = false;
                self.loaded = true;
                self.notice = Some(format!(
                    "Could not open local habits: {error}. You can use an empty session, but changes may not persist."
                ));
            }
            _ => return,
        }
        self.show(cx);
    }
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        self.normalize_pages();
        if a == ActionId::BACK {
            self.go_back();
            self.show(cx);
            return;
        }
        if let Some(event) = self.entry.handle(a) {
            if let Typing::Submitted(name) = event {
                if let Some(name) = canonical_name(&name) {
                    self.items.push(Habit::new(name));
                    self.save(cx);
                } else {
                    self.notice = Some(format!(
                        "Habit names must be 1 to {MAX_HABIT_NAME_CHARS} characters. Nothing was added."
                    ));
                }
            }
            self.show(cx);
            return;
        }
        let pages = [Page::Today, Page::Streaks, Page::Manage, Page::Stats];
        if let Some((_, page)) = Page::all()
            .iter()
            .zip(pages)
            .find(|(tab, _)| a == action_id(tab.0))
        {
            self.page = page;
            if page == Page::Today {
                self.today_page = 0;
            }
            self.show(cx);
            return;
        }
        if a == action_id("add") {
            self.entry.open();
            self.show(cx);
            return;
        }
        if a == action_id("settings") {
            self.page = Page::Settings;
            self.show(cx);
            return;
        }
        if a == action_id("due-prev") {
            self.today_page = self.today_page.saturating_sub(1);
            self.show(cx);
            return;
        }
        if a == action_id("due-next") {
            self.today_page = self.today_page.saturating_add(1);
            self.show(cx);
            return;
        }
        if a == action_id("manage-prev") {
            self.manage_page = self.manage_page.saturating_sub(1);
            self.show(cx);
            return;
        }
        if a == action_id("manage-next") {
            self.manage_page = self.manage_page.saturating_add(1);
            self.show(cx);
            return;
        }
        if a == action_id("streaks-prev") {
            self.streaks_page = self.streaks_page.saturating_sub(1);
            self.show(cx);
            return;
        }
        if a == action_id("streaks-next") {
            self.streaks_page = self.streaks_page.saturating_add(1);
            self.show(cx);
            return;
        }
        let mut changed = false;
        for (i, h) in self.items.iter_mut().enumerate() {
            if a == action_id(&format!("done-{i}")) {
                changed |= h.toggle_complete(Self::day());
            }
            if a == action_id(&format!("skip-{i}")) {
                changed |= h.skip(Self::day());
            }
            if a == action_id(&format!("cycle-{i}")) {
                h.schedule = match h.schedule {
                    Schedule::Daily => Schedule::Weekdays,
                    Schedule::Weekdays => Schedule::Every(2),
                    Schedule::Every(_) => Schedule::Daily,
                };
                changed = true;
            }
        }
        if changed {
            self.save(cx);
        }
        self.show(cx);
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("habits", Habits::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("habits: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command, StoreError, StoreRequest};

    fn wrapped_name(number: usize) -> String {
        let name: String = format!("Habit {number} {}", "long ".repeat(12))
            .chars()
            .take(MAX_HABIT_NAME_CHARS)
            .collect();
        assert_eq!(name.chars().count(), MAX_HABIT_NAME_CHARS);
        name
    }

    fn has_row_action(screen: &Screen, name: &str) -> bool {
        use kobo_sdk::Node;

        screen.nodes.iter().any(|node| match node {
            Node::Rows { rows, .. } => rows.iter().any(|row| row.action == action_id(name)),
            _ => false,
        })
    }

    #[test]
    fn back_returns_each_page_to_its_parent() {
        let mut app = Habits {
            loaded: true,
            ..Habits::default()
        };
        assert!(!app.owns_back());
        app.page = Page::Manage;
        assert!(app.owns_back());
        app.go_back();
        assert_eq!(app.page, Page::Today);
        app.page = Page::Settings;
        assert_eq!(app.back_target(), Some(Page::Stats));
        app.go_back();
        assert_eq!(app.page, Page::Stats);
        app.entry.open();
        assert!(app.owns_back());
        app.go_back();
        assert!(!app.entry.is_open());
    }

    #[test]
    fn sdk_back_from_settings_returns_to_stats() {
        let app = Habits {
            loaded: true,
            page: Page::Settings,
            ..Habits::default()
        };
        let mut runner = AppRunner::new(app);
        runner.start();
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().page, Page::Stats);
    }

    #[test]
    fn manage_screen_has_no_layout_errors() {
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let app = Habits {
            items: vec![Habit::new("Read".into())],
            loaded: true,
            page: Page::Manage,
            ..Habits::default()
        };
        assert!(app
            .screen()
            .with_own_back(app.owns_back())
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }

    #[test]
    fn settings_are_reachable_without_a_fifth_tab() {
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let app = Habits {
            loaded: true,
            page: Page::Stats,
            ..Habits::default()
        };
        assert!(app
            .screen()
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .rect_of_action(action_id("settings"))
            .is_some());
    }

    #[test]
    fn today_checklist_is_stateful_and_layout_clean() {
        use kobo_sdk::{Node, RowState};
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let app = Habits {
            items: vec![Habit::new("Read".into())],
            loaded: true,
            ..Habits::default()
        };
        let screen = app.screen();
        let state = screen.nodes.iter().find_map(|node| match node {
            Node::Rows { rows, .. } => rows
                .iter()
                .find(|row| row.action == action_id("done-0"))
                .map(|row| row.state),
            _ => None,
        });
        assert_eq!(state, Some(RowState::Open));
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }

    #[test]
    fn skipped_habits_are_labelled_and_cannot_be_skipped_twice() {
        use kobo_sdk::Node;
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let day = Habits::day();
        let app = Habits {
            items: vec![Habit::new("Read".into())],
            loaded: true,
            ..Habits::default()
        };
        let mut runner = AppRunner::new(app);
        runner.start();
        let commands = runner.action(action_id("skip-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        assert_eq!(
            decode(saved.expect("skip must be saved"))[0].skipped,
            vec![day]
        );
        runner.store_result(StoreResult::Saved { key: HABITS.into() });

        let screen = runner.app().screen();
        let summary = screen.nodes.iter().find_map(|node| match node {
            Node::Rows { rows, .. } => rows
                .iter()
                .find(|row| row.action == action_id("done-0"))
                .map(|row| row.summary.as_str()),
            _ => None,
        });
        assert_eq!(summary, Some("Skipped"));
        assert!(!screen.nodes.iter().any(|node| matches!(
            node,
            Node::Button { action, .. } if *action == action_id("skip-0")
        )));
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }

    #[test]
    fn every_due_habit_has_a_skip_control_on_a_clean_page() {
        use kobo_sdk::Node;
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let mut app = Habits {
            items: (1..=4)
                .map(|number| Habit::new(wrapped_name(number)))
                .collect(),
            loaded: true,
            ..Habits::default()
        };
        for (page, expected_rows, expected_skip, pager) in [
            (
                0,
                ["done-0", "done-1", "done-2"].as_slice(),
                ["skip-0", "skip-1", "skip-2"].as_slice(),
                "due-next",
            ),
            (1, ["done-3"].as_slice(), ["skip-3"].as_slice(), "due-prev"),
        ] {
            app.today_page = page;
            let screen = app.screen();
            let row_actions = screen
                .nodes
                .iter()
                .flat_map(|node| match node {
                    Node::Rows { rows, .. } => rows.iter().map(|row| row.action).collect(),
                    _ => Vec::new(),
                })
                .collect::<Vec<_>>();
            for action in expected_rows {
                assert!(
                    row_actions.contains(&action_id(action)),
                    "{action} is reachable"
                );
            }
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            for action in expected_skip.iter().chain([pager].iter()) {
                assert!(
                    layout.rect_of_action(action_id(action)).is_some(),
                    "{action} is reachable"
                );
            }
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }

    #[test]
    fn due_page_controls_reach_later_habits() {
        let app = Habits {
            items: (1..=4)
                .map(|number| Habit::new(wrapped_name(number)))
                .collect(),
            loaded: true,
            ..Habits::default()
        };
        let mut runner = AppRunner::new(app);
        runner.action(action_id("due-next"));
        assert_eq!(runner.app().today_page, 1);
        runner.action(action_id("due-prev"));
        assert_eq!(runner.app().today_page, 0);
    }

    #[test]
    fn stale_pager_state_normalizes_before_previous_on_every_route() {
        let mut app = Habits {
            items: (1..=7)
                .map(|number| Habit::new(wrapped_name(number)))
                .collect(),
            loaded: true,
            today_page: 2,
            manage_page: 2,
            streaks_page: 2,
            ..Habits::default()
        };
        app.items.truncate(4);
        let mut runner = AppRunner::new(app);

        assert!(has_row_action(&runner.app().screen(), "done-3"));
        runner.action(action_id("due-prev"));
        assert_eq!(runner.app().today_page, 0);
        assert!(has_row_action(&runner.app().screen(), "done-0"));

        runner.app_mut().page = Page::Manage;
        assert!(has_row_action(&runner.app().screen(), "cycle-3"));
        runner.action(action_id("manage-prev"));
        assert_eq!(runner.app().manage_page, 0);
        assert!(has_row_action(&runner.app().screen(), "cycle-0"));

        runner.app_mut().page = Page::Streaks;
        assert!(has_row_action(&runner.app().screen(), "streak-3"));
        runner.action(action_id("streaks-prev"));
        assert_eq!(runner.app().streaks_page, 0);
        assert!(has_row_action(&runner.app().screen(), "streak-0"));
    }

    #[test]
    fn manage_and_streak_pages_keep_ten_wrapped_habits_reachable() {
        use kobo_sdk::Node;
        use kobo_ui::{Chrome, CLARA_BW_METRICS};

        let mut app = Habits {
            items: (1..=10)
                .map(|number| Habit::new(wrapped_name(number)))
                .collect(),
            loaded: true,
            ..Habits::default()
        };
        for (page, expected, pager) in [
            (
                0,
                ["cycle-0", "cycle-1", "cycle-2"].as_slice(),
                "manage-next",
            ),
            (
                1,
                ["cycle-3", "cycle-4", "cycle-5"].as_slice(),
                "manage-next",
            ),
            (
                2,
                ["cycle-6", "cycle-7", "cycle-8"].as_slice(),
                "manage-next",
            ),
            (3, ["cycle-9"].as_slice(), "manage-prev"),
        ] {
            app.page = Page::Manage;
            app.manage_page = page;
            let screen = app.screen();
            let actions = screen
                .nodes
                .iter()
                .flat_map(|node| match node {
                    Node::Rows { rows, .. } => rows.iter().map(|row| row.action).collect(),
                    _ => Vec::new(),
                })
                .collect::<Vec<_>>();
            for action in expected {
                assert!(actions.contains(&action_id(action)));
            }
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            assert!(layout.rect_of_action(action_id("add")).is_some());
            assert!(layout.rect_of_action(action_id(pager)).is_some());
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }

        for (page, expected, pager) in [
            (
                0,
                ["streak-0", "streak-1", "streak-2"].as_slice(),
                "streaks-next",
            ),
            (
                1,
                ["streak-3", "streak-4", "streak-5"].as_slice(),
                "streaks-next",
            ),
            (
                2,
                ["streak-6", "streak-7", "streak-8"].as_slice(),
                "streaks-next",
            ),
            (3, ["streak-9"].as_slice(), "streaks-prev"),
        ] {
            app.page = Page::Streaks;
            app.streaks_page = page;
            let screen = app.screen();
            let actions = screen
                .nodes
                .iter()
                .flat_map(|node| match node {
                    Node::Rows { rows, .. } => rows.iter().map(|row| row.action).collect(),
                    _ => Vec::new(),
                })
                .collect::<Vec<_>>();
            for action in expected {
                assert!(actions.contains(&action_id(action)));
            }
            let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
            assert!(layout.rect_of_action(action_id(pager)).is_some());
            assert!(screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                .issues
                .is_empty());
        }
    }

    #[test]
    fn initial_store_failure_leaves_an_actionable_empty_session() {
        use kobo_sdk::Node;

        let mut runner = AppRunner::new(Habits::default());
        runner.start();
        runner.store_result(StoreResult::Denied(StoreError::Unwritable));

        assert!(runner.app().loaded);
        assert!(runner.app().items.is_empty());
        assert!(runner
            .app()
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("empty session")));
        assert!(!runner
            .app()
            .screen()
            .nodes
            .iter()
            .any(|node| matches!(node, Node::Skeleton { .. })));
    }

    #[test]
    fn legacy_names_remain_editable_and_round_trip_without_truncation() {
        let long = "x".repeat(513);
        let spaced = "  Read before bed  ";
        let mut runner = AppRunner::new(Habits::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: HABITS.into(),
            value: Some(
                format!("0\td\t{long}\t\t\n0\tw\t{spaced}\t\t\n0\td\t  \t\t\n").into_bytes(),
            ),
        });

        assert_eq!(runner.app().items.len(), 2);
        assert_eq!(runner.app().items[0].name, long);
        assert_eq!(runner.app().items[1].name, spaced);
        assert!(runner
            .app()
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("blank saved")));

        let today = runner.app().screen();
        assert!(has_row_action(&today, "done-0"));
        let title = today.nodes.iter().find_map(|node| match node {
            kobo_sdk::Node::Rows { rows, .. } => rows
                .iter()
                .find(|row| row.action == action_id("done-0"))
                .map(|row| row.title.as_str()),
            _ => None,
        });
        assert_eq!(
            title.map(|value| value.chars().count()),
            Some(MAX_HABIT_NAME_CHARS)
        );
        assert!(title.is_some_and(|title| title.ends_with('…')));
        assert!(today
            .layout_with(&kobo_ui::CLARA_BW_METRICS, &kobo_ui::Chrome::default())
            .rect_of_action(action_id("skip-0"))
            .is_some());
        let diagnostics =
            today.diagnostics(&kobo_ui::CLARA_BW_METRICS, &kobo_ui::Chrome::default());
        assert!(diagnostics.issues.is_empty(), "{:#?}", diagnostics.issues);

        let day = Habits::day();
        let commands = runner.action(action_id("done-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        let saved = decode(saved.expect("legacy completion must be saved"));
        assert_eq!(saved[0].name, long);
        assert_eq!(saved[1].name, spaced);
        assert_eq!(saved[0].done, vec![day]);
        runner.store_result(StoreResult::Saved { key: HABITS.into() });

        let commands = runner.action(action_id("done-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        assert!(
            decode(saved.expect("legacy completion must be correctable"))[0]
                .done
                .is_empty()
        );
        runner.store_result(StoreResult::Saved { key: HABITS.into() });

        let commands = runner.action(action_id("skip-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        assert_eq!(
            decode(saved.expect("legacy skip must be saved"))[0].skipped,
            vec![day]
        );
        runner.store_result(StoreResult::Saved { key: HABITS.into() });

        runner.app_mut().page = Page::Manage;
        let manage = runner.app().screen();
        assert!(has_row_action(&manage, "cycle-0"));
        assert!(manage
            .diagnostics(&kobo_ui::CLARA_BW_METRICS, &kobo_ui::Chrome::default())
            .issues
            .is_empty());
        let commands = runner.action(action_id("cycle-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        let saved = decode(saved.expect("legacy schedule change must be saved"));
        assert_eq!(saved[0].name, long);
        assert_eq!(saved[0].schedule, Schedule::Weekdays);
        assert_eq!(saved[0].skipped, vec![day]);
    }

    #[test]
    fn corrective_save_clears_an_earlier_failure_without_stale_override() {
        let day = Habits::day();
        let app = Habits {
            items: vec![Habit::new("Read".into())],
            loaded: true,
            ..Habits::default()
        };
        let mut runner = AppRunner::new(app);

        let commands = runner.action(action_id("done-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        let saved = saved.expect("completion must be saved");
        assert_eq!(decode(saved)[0].done, vec![day]);

        runner.store_result(StoreResult::Denied(StoreError::Unwritable));
        assert_eq!(runner.app().items[0].done, vec![day]);
        assert!(runner
            .app()
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("could not be saved")));

        let commands = runner.action(action_id("done-0"));
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        assert!(
            decode(saved.expect("corrected completion must be saved"))[0]
                .done
                .is_empty()
        );

        runner.store_result(StoreResult::Saved { key: HABITS.into() });
        assert!(runner.app().notice.is_none());

        runner.store_result(StoreResult::Denied(StoreError::Unwritable));
        assert!(runner.app().notice.is_none(), "stale failure is ignored");
    }

    #[test]
    fn later_full_state_waits_for_the_previous_save_acknowledgement() {
        let app = Habits {
            items: vec![Habit::new("Read".into())],
            loaded: true,
            ..Habits::default()
        };
        let mut runner = AppRunner::new(app);

        assert!(runner
            .action(action_id("done-0"))
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::Save { .. }))));
        assert!(!runner
            .action(action_id("done-0"))
            .iter()
            .any(|command| matches!(command, Command::Store(StoreRequest::Save { .. }))));

        let commands = runner.store_result(StoreResult::Saved { key: HABITS.into() });
        let saved = commands.iter().find_map(|command| match command {
            Command::Store(StoreRequest::Save { key, value }) if key == HABITS => Some(value),
            _ => None,
        });
        assert!(
            decode(saved.expect("latest state must follow the acknowledgement"))[0]
                .done
                .is_empty()
        );
    }
}
