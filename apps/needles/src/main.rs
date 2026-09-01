//! Needles keeps the counter usable without Wi-Fi. Ravelry Basic credentials
//! are only passed to the runtime by name and never enter application memory.

use kobo_json::Value;
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, Screen,
    ScreenBuilder, StoreResult, Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

const STATE: &str = "counter-state";
const LIBRARY_URL: &str = "https://api.ravelry.com/people/me/library/list.json";
const MAX_JSON: u32 = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Project,
    Library,
    Pattern,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Counter {
    section: String,
    row: u32,
    repeat: u8,
    repeat_total: u8,
}

#[derive(Default)]
struct Needles {
    route: Route,
    counter: Counter,
    library: Vec<String>,
    notice: Option<String>,
    task: Option<TaskId>,
    loaded: bool,
}

impl Needles {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Project));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Project => self.project(),
            Route::Library => self.library(),
            Route::Pattern => Self::pattern(),
        }
    }

    fn project(&self) -> Screen {
        let mut screen = ScreenBuilder::new("needles-project").top_bar("Needles");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
            .heading(if self.counter.section.is_empty() {
                "No pattern selected"
            } else {
                &self.counter.section
            })
            .facts([
                ("Row", self.counter.row.to_string()),
                (
                    "Repeat",
                    format!(
                        "row {} of {}",
                        self.counter.repeat, self.counter.repeat_total
                    ),
                ),
                ("Power", "Stand mode: ~7 days".to_owned()),
            ])
            .text("Pattern text and counters stay available after sync.")
            .primary_button("plus", "+1 row")
            .buttons([("minus", "−1 row"), ("section", "Change section")])
            .button("library", "Library and queue")
            .build()
    }

    fn library(&self) -> Screen {
        let mut screen = ScreenBuilder::new("needles-library").top_bar("Library");
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        if !self.loaded {
            screen = screen.skeleton(5);
        } else if self.library.is_empty() {
            screen = screen.empty_state(
                "Your Ravelry library appears here after a sync. Install a key with kobo secret set ravelry.",
            );
        } else {
            screen = screen
                .section("Patterns")
                .rows(self.library.iter().enumerate().map(|(index, title)| {
                    (
                        format!("pattern-{index}"),
                        title.clone(),
                        "Tap to follow",
                        Glyph::Bookmark,
                    )
                }));
        }
        screen.primary_button("sync", "Sync library").build()
    }

    fn pattern() -> Screen {
        ScreenBuilder::new("needles-pattern")
            .top_bar("Pattern")
            .heading("Pattern ready offline")
            .text("Native-text pages reflow here. Charts and scans remain image pages.")
            .primary_button("follow", "Follow this pattern")
            .build()
    }

    fn save(&self, context: &mut Context) {
        context.store().save(
            STATE,
            format!(
                "{}\n{}\n{}\n{}",
                self.counter.section,
                self.counter.row,
                self.counter.repeat,
                self.counter.repeat_total
            ),
        );
    }

    fn increment(&mut self, context: &mut Context, direction: i8) {
        if direction > 0 {
            self.counter.row = self.counter.row.saturating_add(1);
            self.counter.repeat = self.counter.repeat.saturating_add(1);
            if self.counter.repeat > self.counter.repeat_total {
                self.counter.repeat = 1;
            }
        } else {
            self.counter.row = self.counter.row.saturating_sub(1);
            self.counter.repeat = self.counter.repeat.saturating_sub(1).max(1);
        }
        self.notice = None;
        self.save(context);
    }

    fn sync(&mut self, context: &mut Context) {
        if let Some(task) = context.spawn_retrying(Task::Fetch {
            url: LIBRARY_URL.to_owned(),
            offset: 0,
            max_bytes: MAX_JSON,
            credential: Some(Credential::basic("ravelry")),
            headers: Vec::new(),
        }) {
            self.task = Some(task);
            self.notice = Some("Reading your Ravelry library.".to_owned());
        }
    }

    fn read_library(&mut self, bytes: &[u8]) -> bool {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return false;
        };
        let Ok(value) = kobo_json::parse(text) else {
            return false;
        };
        let Some(patterns) = value.get("patterns").and_then(Value::as_array) else {
            return false;
        };
        self.library = patterns
            .iter()
            .filter_map(|pattern| pattern.get("name").and_then(Value::as_str))
            .map(str::to_owned)
            .take(60)
            .collect();
        self.loaded = true;
        true
    }

    fn restore(&mut self, bytes: &[u8]) {
        let Ok(text) = std::str::from_utf8(bytes) else {
            return;
        };
        let mut fields = text.lines();
        let (Some(section), Some(row), Some(repeat), Some(total)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            return;
        };
        if let (Ok(row), Ok(repeat), Ok(total)) = (row.parse(), repeat.parse(), total.parse()) {
            self.counter = Counter {
                section: section.to_owned(),
                row,
                repeat,
                repeat_total: total,
            };
        }
    }
}

impl KoboApp for Needles {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(STATE);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == STATE {
                if let Some(value) = value {
                    self.restore(&value);
                }
                self.show(context);
            }
        }
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == ActionId::BACK {
            self.route = Route::Project;
        } else if action == action_id("plus") {
            self.increment(context, 1);
        } else if action == action_id("minus") {
            self.increment(context, -1);
        } else if action == action_id("section") {
            let section = if self.counter.section == "Body" {
                "Sleeve"
            } else {
                "Body"
            };
            section.clone_into(&mut self.counter.section);
            self.counter.repeat = 1;
            self.counter.repeat_total = 12;
            self.save(context);
        } else if action == action_id("library") {
            self.route = Route::Library;
        } else if action == action_id("sync") {
            self.sync(context);
        } else if action == action_id("follow") {
            self.route = Route::Project;
            "Body".clone_into(&mut self.counter.section);
            self.counter.repeat = 1;
            self.counter.repeat_total = 12;
            self.save(context);
        } else if (0..self.library.len())
            .any(|index| action == action_id(&format!("pattern-{index}")))
        {
            self.route = Route::Pattern;
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        self.notice = match outcome {
            TaskOutcome::Completed(bytes) if self.read_library(&bytes) => None,
            TaskOutcome::Completed(_) => {
                Some("Ravelry returned a library this version cannot read.".to_owned())
            }
            TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential) => {
                Some("Install your Ravelry Basic key with kobo secret set ravelry.".to_owned())
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
    use super::{Counter, Needles, Route};
    use kobo_sdk::{action_id, Context, KoboApp};
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn counter_repeats_and_never_underflows() {
        let mut app = Needles {
            counter: Counter {
                section: "Body".to_owned(),
                row: 11,
                repeat: 11,
                repeat_total: 12,
            },
            ..Needles::default()
        };
        let mut context = Context::default();
        app.increment(&mut context, 1);
        assert_eq!((app.counter.row, app.counter.repeat), (12, 12));
        app.increment(&mut context, 1);
        assert_eq!(app.counter.repeat, 1);
        app.increment(&mut context, -1);
        assert_eq!(app.counter.repeat, 1);
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
    }
}
