//! Every UI primitive, on one device, so each can be checked by eye.
//!
//! This is a test instrument as much as a demonstration. If a primitive looks
//! wrong here it looks wrong everywhere, and the layout tests only prove that
//! sizes are right, not that the result is worth reading.

use kobo_sdk::keyboard::{TextEntry, Typing};
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

struct Gallery {
    tab: Tab,
    entry: TextEntry,
    answer: Option<String>,
    loading: bool,
    task: Option<TaskId>,
    outcome: Option<String>,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            // Binding the free-text row here is the whole mechanism: the row
            // drawn by `or_type` emits this action, and the field opens itself
            // when it sees it.
            entry: TextEntry::new().opened_by("file-other"),
            answer: None,
            loading: false,
            task: None,
            outcome: None,
        }
    }
}

impl Gallery {
    fn show(&self, context: &mut Context) {
        // A raised keyboard is modal: it covers the panel, so nothing else is
        // drawn under it, including the tab bar.
        if self.entry.is_open() {
            context.set_screen(
                ScreenBuilder::new("gallery")
                    .text_entry(&self.entry, "Something else", "Use this")
                    .build(),
            );
            return;
        }
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

            Tab::Ask => self.ask(screen),

            Tab::Work => self.work(screen),
        };

        let screen = screen
            .nav_bar(
                self.tab.index(),
                Tab::ALL.map(|(_, name, label)| (name, label)),
            )
            .build();
        context.set_screen(screen);
    }

    fn ask(&self, screen: ScreenBuilder) -> ScreenBuilder {
        {
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
    }

    fn work(&self, screen: ScreenBuilder) -> ScreenBuilder {
        {
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

        // The typed row is handled first, because while the keyboard is up it
        // owns the panel. This is what makes the free-text row actually raise
        // a keyboard rather than answer for the reader, which is what it used
        // to do.
        if let Some(event) = self.entry.handle(action) {
            if let Typing::Submitted(text) = event {
                self.answer = Some(text);
            }
            self.show(context);
            return;
        }

        for (name, label) in [
            ("file-keep", "Keep for later"),
            ("file-share", "Share it"),
            ("file-archive", "Archive"),
            ("file-discard", "Discard"),
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

#[cfg(test)]
mod tests {
    use super::Gallery;
    use kobo_sdk::{action_id, Context, KoboApp};

    #[test]
    fn the_free_text_row_raises_the_keyboard_rather_than_answering_for_the_reader() {
        // This is a regression. The row used to be wired to a canned string,
        // so tapping "Something else..." filled in an answer nobody typed and
        // the keyboard never appeared.
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("file-other"));
        assert!(gallery.entry.is_open(), "the free-text row opened nothing");
        assert!(
            gallery.answer.is_none(),
            "an answer appeared without typing"
        );
    }

    #[test]
    fn what_was_typed_becomes_the_answer() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_action(&mut context, action_id("file-other"));
        for key in ["kb.r0c0", "kb.r0c1"] {
            gallery.on_action(&mut context, action_id(key));
        }
        gallery.on_action(&mut context, action_id("kb.enter"));
        assert_eq!(gallery.answer.as_deref(), Some("qw"));
        assert!(!gallery.entry.is_open());
    }
}
