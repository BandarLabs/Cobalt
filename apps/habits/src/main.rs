mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Glyph, Header, KoboApp, Screen,
    ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome,
};
use model::{decode, encode, Habit, Schedule};
use std::process::ExitCode;
const HABITS: &str = "habits-v1";
#[derive(Clone, Copy, Eq, PartialEq)]
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
    fn all() -> [(&'static str, &'static str); 5] {
        [
            ("today", "Today"),
            ("streaks", "Streaks"),
            ("manage", "Manage"),
            ("stats", "Stats"),
            ("settings", "Settings"),
        ]
    }
}
struct Habits {
    habits: Vec<Habit>,
    loaded: bool,
    page: Page,
    entry: TextEntry,
    notice: Option<String>,
    syncing: Option<TaskId>,
    habitica: bool,
}
impl Default for Habits {
    fn default() -> Self {
        Self {
            habits: vec![],
            loaded: false,
            page: Page::Today,
            entry: TextEntry::new().opened_by("add"),
            notice: None,
            syncing: None,
            habitica: false,
        }
    }
}
impl Habits {
    fn day() -> u32 {
        (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 86_400) as u32
    }
    fn save(&self, cx: &mut Context) {
        cx.store().save(HABITS, encode(&self.habits));
    }
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen());
    }
    fn screen(&self) -> Screen {
        if self.entry.is_open() {
            return ScreenBuilder::new("hb-add")
                .top_bar("Habits")
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
        .top_bar("Habits")
        .tabs(self.page.index(), Page::all());
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
                    .habits
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
                    s = s
                        .rows(due.iter().map(|(i, h)| {
                            let done = h.done.contains(&day);
                            (
                                format!("done-{i}"),
                                h.name.clone(),
                                if done {
                                    "Done".to_owned()
                                } else {
                                    h.schedule_label()
                                },
                                if done { Glyph::Check } else { Glyph::Circle },
                            )
                        }))
                        .buttons(
                            due.iter()
                                .filter(|(_, h)| !h.done.contains(&day))
                                .map(|(i, h)| (format!("skip-{i}"), format!("Skip {}", h.name)))
                                .take(3),
                        );
                }
            }
            Page::Streaks => {
                if self.habits.is_empty() {
                    s = s.empty_state("Streaks appear after you add a habit.");
                } else {
                    s = s.rows(self.habits.iter().filter(|h| !h.archived).map(|h| {
                        (
                            "none",
                            h.name.clone(),
                            format!(
                                "{} current, {} best",
                                h.current_streak(Self::day()),
                                h.best_streak(Self::day())
                            ),
                            Glyph::Chart,
                        )
                    }));
                }
            }
            Page::Manage => {
                s = s
                    .rows(self.habits.iter().enumerate().map(|(i, h)| {
                        (
                            format!("cycle-{i}"),
                            h.name.clone(),
                            format!(
                                "{}{}",
                                h.schedule_label(),
                                if h.archived { "; archived" } else { "" }
                            ),
                            Glyph::Settings,
                        )
                    }))
                    .button("add", "Add habit");
            }
            Page::Stats => {
                let completed: usize = self.habits.iter().map(|h| h.done.len()).sum();
                s = s
                    .heading(format!("{completed} completions"))
                    .text("Best streaks are measured across scheduled days.");
                if self.habitica {
                    s = s
                        .text("Habitica sync uses the secret named habitica.")
                        .button("sync", "Sync now");
                }
            }
            Page::Settings => {
                s = s
                    .rows([(
                        "mode",
                        if self.habitica {
                            "Habitica account"
                        } else {
                            "Standalone"
                        },
                        if self.habitica {
                            "Scores queue while off the air."
                        } else {
                            "Works without network access."
                        },
                        Glyph::Settings,
                    )])
                    .button("mode", "Change mode");
                if self.habitica {
                    s = s.text(
                        "Set user ID in the account setup, then run kobo secret set habitica.",
                    );
                }
            }
        }
        s.build()
    }
    fn sync(&mut self, cx: &mut Context) {
        self.syncing = cx.spawn(Task::Fetch {
            url: "https://habitica.com/api/v3/tasks/user".into(),
            offset: 0,
            max_bytes: 128 * 1024,
            credential: Some(Credential::in_header("habitica", "x-api-key")),
            headers: vec![Header::new("x-client", "kobo-habits")],
        });
        if self.syncing.is_none() {
            self.notice =
                Some("Habitica sync could not start. Run kobo secret set habitica.".into());
        }
    }
}
impl KoboApp for Habits {
    fn on_start(&mut self, cx: &mut Context) {
        cx.store().load(HABITS);
        self.show(cx)
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == HABITS {
                self.habits = value.map(|v| decode(&v)).unwrap_or_default();
                self.loaded = true;
                self.show(cx)
            }
        }
    }
    fn on_task(&mut self, cx: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.syncing == Some(task) {
            self.syncing = None;
            self.notice = Some(match outcome {
                TaskOutcome::Completed(_) => "Habitica tasks synced.".into(),
                TaskOutcome::Failed(_) => {
                    "Habitica is off the air. Scores will retry on the next sync.".into()
                }
                TaskOutcome::Cancelled => "Habitica sync cancelled.".into(),
            });
            self.show(cx)
        }
    }
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        if let Some(event) = self.entry.handle(a) {
            if let Typing::Submitted(name) = event {
                if !name.trim().is_empty() {
                    self.habits.push(Habit::new(name));
                    self.save(cx)
                }
            }
            self.show(cx);
            return;
        }
        let pages = [
            Page::Today,
            Page::Streaks,
            Page::Manage,
            Page::Stats,
            Page::Settings,
        ];
        if let Some((_, page)) = Page::all()
            .iter()
            .zip(pages)
            .find(|(tab, _)| a == action_id(tab.0))
        {
            self.page = page;
            self.show(cx);
            return;
        }
        if a == action_id("add") {
            self.entry.open();
            self.show(cx);
            return;
        }
        if a == action_id("mode") {
            self.habitica = !self.habitica;
            self.show(cx);
            return;
        }
        if a == action_id("sync") {
            self.sync(cx);
            self.show(cx);
            return;
        }
        let mut changed = false;
        for (i, h) in self.habits.iter_mut().enumerate() {
            if a == action_id(&format!("done-{i}")) {
                changed |= h.complete(Self::day());
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
        self.show(cx)
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
