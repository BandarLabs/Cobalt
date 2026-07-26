//! Every UI primitive, on one device, so each can be checked by eye.
//!
//! This is a test instrument as much as a demonstration. If a primitive looks
//! wrong here it looks wrong everywhere, and the layout tests only prove that
//! sizes are right, not that the result is worth reading.

use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, KoboApp, LogLevel, ScreenBuilder, Space,
    Task, TaskId, TaskOutcome,
};
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Text,
    Tiles,
    Ask,
    Work,
}

impl Tab {
    const ALL: [(Self, &'static str, &'static str); 4] = [
        (Self::Text, "tab-text", "Text"),
        (Self::Tiles, "tab-tiles", "Tiles"),
        (Self::Ask, "tab-ask", "Ask"),
        (Self::Work, "tab-work", "Work"),
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|(tab, _, _)| *tab == self)
            .unwrap_or(0)
    }
}

#[derive(Default)]
struct Gallery {
    tab: Tab,
    answer: Option<String>,
    loading: bool,
    task: Option<TaskId>,
    outcome: Option<String>,
}

impl Gallery {
    fn show(&self, context: &mut Context) {
        let screen = ScreenBuilder::new("gallery").top_bar(match self.tab {
            Tab::Text => "Type and tone",
            Tab::Tiles => "Tiles and icons",
            Tab::Ask => "Asking a question",
            Tab::Work => "Work in flight",
        });

        let screen = match self.tab {
            Tab::Text => screen
                .heading("Heading")
                .text(
                    "Body copy wraps at a measure chosen from the panel's physical width, \
                     not its pixel count, so a line holds a similar number of words on \
                     every supported device.",
                )
                .spacer(Space::Small)
                .banner(BannerLevel::Info, "An informational banner.")
                .spacer(Space::Small)
                .banner(
                    BannerLevel::Attention,
                    "An attention banner, drawn inverted. This is what replaces flashing \
                     the frontlight.",
                )
                .spacer(Space::Medium)
                .divider()
                .text("Below the rule: a progress bar and placeholder lines.")
                .progress(65)
                .spacer(Space::Small)
                .skeleton(3),

            Tab::Tiles => screen
                .text(
                    "Columns are derived from physical width, so this is two across on a \
                       six inch panel and three on a ten inch one.",
                )
                .spacer(Space::Small)
                .tiles([
                    ("open-reader", "Reader", Glyph::Reader),
                    ("open-books", "Library", Glyph::Book),
                    ("open-notes", "Notes", Glyph::Note),
                    ("open-clock", "Clock", Glyph::Clock),
                    ("open-search", "Search", Glyph::Search),
                    ("open-chart", "Stats", Glyph::Chart),
                    ("open-wifi", "Network", Glyph::Wifi),
                    ("open-battery", "Battery", Glyph::Battery),
                    ("open-folder", "Files", Glyph::Folder),
                    ("open-settings", "Settings", Glyph::Settings),
                    ("open-power", "Power", Glyph::Power),
                    ("open-app", "Generic", Glyph::App),
                ]),

            Tab::Ask => {
                let screen = screen
                    .text(match &self.answer {
                        None => "Nothing chosen yet.".to_owned(),
                        Some(answer) => format!("You chose: {answer}"),
                    })
                    .spacer(Space::Small)
                    .choose(
                        "How should this note be filed?",
                        [
                            ("file-keep", "Keep for later"),
                            ("file-share", "Share it"),
                            ("file-archive", "Archive"),
                            ("file-discard", "Discard"),
                        ],
                    )
                    .or_type("file-other", "Something else...");
                screen
            }

            Tab::Work => {
                let screen = screen.text(
                    "A request is work handed to the runtime. The screen stays live \
                     throughout, and there is no spinner because every frame would be a \
                     panel refresh.",
                );
                let screen = if self.loading {
                    screen
                        .spacer(Space::Small)
                        .activity("Fetching the catalogue", None)
                        .cancellable("cancel-fetch", "Cancel")
                } else {
                    screen
                        .spacer(Space::Small)
                        .text(self.outcome.as_deref().unwrap_or("Idle."))
                        .spacer(Space::Small)
                        .button("start-fetch", "Start a request")
                };
                screen
            }
        };

        let screen = screen
            .nav_bar(
                self.tab.index(),
                Tab::ALL.map(|(_, name, label)| (name, label)),
            )
            .build();
        context.set_screen(screen);
    }
}

impl KoboApp for Gallery {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        for (tab, name, _) in Tab::ALL {
            if action == action_id(name) {
                self.tab = tab;
                self.show(context);
                return;
            }
        }

        for (name, label) in [
            ("file-keep", "Keep for later"),
            ("file-share", "Share it"),
            ("file-archive", "Archive"),
            ("file-discard", "Discard"),
            ("file-other", "something typed"),
        ] {
            if action == action_id(name) {
                self.answer = Some(label.to_owned());
                self.show(context);
                return;
            }
        }

        if action == action_id("start-fetch") {
            // The simulator has no network, so this is expected to be refused.
            // That is the point: the failure path runs during development
            // rather than for the first time on someone's device.
            match context.spawn(Task::Fetch {
                url: "https://example.invalid/catalog".to_owned(),
                offset: 0,
                max_bytes: 4096,
            }) {
                Some(task) => {
                    self.task = Some(task);
                    self.loading = true;
                    self.outcome = None;
                }
                None => self.outcome = Some("Too much already in flight.".to_owned()),
            }
            self.show(context);
            return;
        }

        if action == action_id("cancel-fetch") {
            if let Some(task) = self.task {
                context.cancel(task);
                context.log(LogLevel::Info, "reader cancelled the request");
            }
            return;
        }

        if let Some(tile) = [
            "open-reader",
            "open-books",
            "open-notes",
            "open-clock",
            "open-search",
            "open-chart",
            "open-wifi",
            "open-battery",
            "open-folder",
            "open-settings",
            "open-power",
            "open-app",
        ]
        .into_iter()
        .find(|name| action == action_id(name))
        {
            context.log(LogLevel::Info, format!("tile tapped: {tile}"));
        }
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        if self.task != Some(task) {
            return;
        }
        self.task = None;
        self.loading = false;
        self.outcome = Some(match outcome {
            TaskOutcome::Completed(bytes) => format!("Received {} bytes.", bytes.len()),
            TaskOutcome::Failed(error) => format!("Request failed: {error}"),
            TaskOutcome::Cancelled => "Request cancelled.".to_owned(),
        });
        self.show(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("gallery", Gallery::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("gallery: {error}");
            ExitCode::FAILURE
        }
    }
}
