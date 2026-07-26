//! The framework launcher.
//!
//! This is deliberately an ordinary application written against `kobo-sdk`.
//! It gets no privileged drawing path, no private widgets and no hardware
//! access the counter example could not also ask for. The only thing that will
//! eventually distinguish it is a permission to enumerate and start other
//! applications. If the launcher cannot be expressed with the public SDK, the
//! SDK is not good enough yet, so keeping it honest here is the point.
//!
//! Returning to the stock reader is a first-class, always-visible destination
//! rather than something hidden in a menu. The reader is not an application and
//! cannot be one: it owns the framebuffer, input, power and Wi-Fi while it
//! runs, and its lifecycle belongs to vendor init. Showing it again means
//! ending this session and restarting it. Making that the most obvious control
//! on the screen also makes it the most exercised path in the system, which is
//! exactly where the reliability is wanted.

use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, ScreenBuilder};
use std::process::ExitCode;

/// One entry in the launcher.
///
/// `name` is the action identity and is never shown; `title` is shown and is
/// never used for identity. Keeping those apart means renaming a label cannot
/// silently change what a tap does.
struct Entry {
    name: &'static str,
    title: &'static str,
    summary: &'static str,
    glyph: Glyph,
}

const ENTRIES: &[Entry] = &[
    Entry {
        name: "gutenshelf",
        title: "Gutenshelf",
        summary: "Sixty thousand free books from Project Gutenberg.",
        glyph: Glyph::Book,
    },
    Entry {
        name: "terminal",
        title: "Terminal",
        summary: "A shell on the panel, with keys that send rather than collect.",
        glyph: Glyph::Terminal,
    },
    Entry {
        name: "gallery",
        title: "Components",
        summary: "Every UI primitive on real hardware, for checking by eye.",
        glyph: Glyph::Chart,
    },
    Entry {
        name: "tictactoe",
        title: "Tic-tac-toe",
        summary: "Two players, one panel. Nought goes first.",
        glyph: Glyph::Grid,
    },
    Entry {
        name: "brief",
        title: "Daily Brief",
        summary: "Collects the day's stories while you read something else.",
        glyph: Glyph::Clock,
    },
    Entry {
        name: "todo",
        title: "Todo",
        summary: "A list that remembers itself. Tap an item to finish it.",
        glyph: Glyph::Check,
    },
    Entry {
        name: "chat",
        title: "AI Command Center",
        summary: "Ask a question and tap the answer, rather than typing one.",
        glyph: Glyph::Chart,
    },
];

#[derive(Default)]
enum View {
    #[default]
    Home,
    /// An entry was chosen and the runtime has been asked to start it. The
    /// screen says so, because the panel is slow enough that a tap with no
    /// visible answer reads as a tap that was missed.
    Starting(usize),
    Leaving,
}

#[derive(Default)]
struct Launcher {
    view: View,
    /// Which page of entries is showing.
    ///
    /// Held rather than derived, because the catalogue is longer than one
    /// panel and nothing here scrolls: the list is turned like a page.
    page: usize,
}

impl Launcher {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(context),
            View::Starting(index) => Self::starting(index),
            View::Leaving => Self::leaving(),
        };
        context.set_screen(screen);
    }

    /// The entries on each page, for the panel this is actually running on.
    ///
    /// Asked of the runtime rather than assumed. Six fit a Clara and more fit
    /// a Sage, and an application that picked a number would be wrong on every
    /// panel but one.
    fn pages(context: &Context) -> Vec<Vec<usize>> {
        let rows = ENTRIES
            .iter()
            .map(|entry| (entry.title, entry.summary))
            .collect::<Vec<_>>();
        let pages = context.paginate_rows(&rows, true);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// The home screen.
    ///
    /// Rows rather than tiles. A tile is square, so three of them filled a
    /// 1072 by 1448 panel while saying only three words between them, and the
    /// summary each entry already carried had nowhere to go. A row spends the
    /// same finger-height band on a title, that summary and a glyph.
    fn home(&mut self, context: &Context) -> kobo_sdk::Screen {
        let pages = Self::pages(context);
        self.page = self.page.min(pages.len() - 1);
        let showing = &pages[self.page];
        let title = if pages.len() > 1 {
            format!("Cobalt \u{2014} {} of {}", self.page + 1, pages.len())
        } else {
            "Cobalt".to_owned()
        };
        let screen = ScreenBuilder::new("launcher").top_bar(title).rows(
            showing
                .iter()
                .map(|&index| &ENTRIES[index])
                .map(|entry| (entry.name, entry.title, entry.summary, entry.glyph)),
        );
        // The way out is pinned to the panel rather than placed after the
        // list. Layout reserves the bar before any content, so however many
        // entries there are, the button that gives the device back is on the
        // screen. Put at the end of the flow it would be the first thing a
        // long catalogue pushed off the bottom.
        if pages.len() > 1 {
            screen
                .nav_bar(
                    usize::MAX,
                    [
                        ("previous", "Previous"),
                        ("reader", "Return to Kobo reader"),
                        ("next", "More apps"),
                    ],
                )
                .build()
        } else {
            // One page, so there is nothing to turn and the bar would be two
            // thirds empty. The way out becomes an ordinary button at the end
            // of the list, which is safe here precisely because the list was
            // measured against the smaller area a bar would have left: the
            // button cannot be pushed off a screen the entries already fit.
            screen
                .divider()
                .button("reader", "Return to Kobo reader")
                .build()
        }
    }

    fn starting(index: usize) -> kobo_sdk::Screen {
        let entry = &ENTRIES[index];
        ScreenBuilder::new("launcher-starting")
            .top_bar(entry.title)
            .heading(entry.title)
            .text(entry.summary)
            .activity("Starting", None)
            .build()
    }

    fn leaving() -> kobo_sdk::Screen {
        ScreenBuilder::new("launcher-leaving")
            .top_bar("Returning")
            .heading("Returning to the Kobo reader")
            .text("The reader takes about half a minute to start and rescan.")
            .build()
    }
}

impl KoboApp for Launcher {
    fn on_start(&mut self, context: &mut Context) {
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if action == action_id("next") || action == action_id("previous") {
            let pages = Self::pages(context).len();
            self.page = if action == action_id("next") {
                // Wrapping rather than stopping. With a bar that always shows
                // both directions, a control that silently does nothing at the
                // end reads as a missed tap on a panel this slow.
                (self.page + 1) % pages
            } else {
                (self.page + pages - 1) % pages
            };
            self.show(context);
            return;
        }
        if action == action_id("reader") {
            self.view = View::Leaving;
            // The screen is painted before leaving so the panel explains the
            // wait. E Ink holds the last image at zero power, so this costs one
            // refresh and nothing else.
            self.show(context);
            context.exit();
            return;
        }
        if action == action_id("back") {
            self.view = View::Home;
            self.show(context);
            return;
        }
        if let Some(index) = ENTRIES
            .iter()
            .position(|entry| action == action_id(entry.name))
        {
            // Paint first, then ask. The runtime stops this application to
            // start the other one, so this is the last chance to leave
            // something on the panel explaining the wait.
            self.view = View::Starting(index);
            self.show(context);
            context.launch(ENTRIES[index].name);
        }
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("launcher", Launcher::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("launcher: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Launcher, View, ENTRIES};
    use kobo_sdk::{action_id, AppRunner, Command};
    use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, CLARA_BW_METRICS};

    const PANELS: [(&str, DisplayMetrics); 3] = [
        ("clara-bw", CLARA_BW_METRICS),
        (
            "nia",
            DisplayMetrics {
                width: 758,
                height: 1024,
                pixels_per_inch: 212,
            },
        ),
        (
            "sage",
            DisplayMetrics {
                width: 1440,
                height: 1920,
                pixels_per_inch: 300,
            },
        ),
    ];

    fn painted(commands: Vec<Command>) -> kobo_sdk::Screen {
        commands
            .into_iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("a screen was painted")
    }

    #[test]
    fn the_way_back_to_the_reader_is_on_every_page_of_every_panel() {
        // The one control that must never be unreachable. Placed at the end of
        // the flow it is the first thing a long catalogue pushes off the
        // bottom, and the layout engine drops what does not fit in silence,
        // so the failure would look like a launcher that cannot be left.
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut seen = 0;
            let mut screen = painted(runner.start());
            loop {
                let layout = screen.layout_with(&metrics, Chrome::with_back(false));
                let reader = action_id("reader");
                let found = layout.nodes.iter().any(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::Button(action) | LayoutKind::NavDestination(action)
                        if action == reader
                    )
                });
                assert!(found, "{name}: no way back to the reader on page {seen}");
                seen += 1;
                if seen > ENTRIES.len() {
                    break;
                }
                let commands = runner.action(action_id("next"));
                if commands.is_empty() {
                    break;
                }
                screen = painted(commands);
            }
        }
    }

    #[test]
    fn every_entry_appears_on_exactly_one_page() {
        // An entry that lands on no page is an application that cannot be
        // started at all, and one on two pages is a list that never ends.
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut runs = 0;
            let mut found = Vec::new();
            let mut screen = painted(runner.start());
            loop {
                let layout = screen.layout_with(&metrics, Chrome::with_back(false));
                for node in &layout.nodes {
                    if let LayoutKind::Row(action) = node.kind {
                        found.push(action);
                    }
                }
                runs += 1;
                if runs > ENTRIES.len() {
                    break;
                }
                screen = painted(runner.action(action_id("next")));
                let first = ENTRIES
                    .first()
                    .map(|entry| action_id(entry.name))
                    .expect("the catalogue is not empty");
                if found.contains(&first) && found.len() >= ENTRIES.len() {
                    break;
                }
            }
            found.sort_unstable();
            found.dedup();
            assert_eq!(
                found.len(),
                ENTRIES.len(),
                "{name}: {} of {} entries were reachable",
                found.len(),
                ENTRIES.len()
            );
        }
    }

    #[test]
    fn every_row_on_a_page_is_drawn_rather_than_dropped() {
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let screen = painted(runner.start());
            let layout = screen.layout_with(&metrics, Chrome::with_back(false));
            let rows = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
                .collect::<Vec<_>>();
            assert!(!rows.is_empty(), "{name}: the first page drew no entries");
            let floor = metrics.height - metrics.nav_bar_height();
            for row in rows {
                assert!(
                    row.rect.y + row.rect.height <= floor,
                    "{name}: an entry ran under the pinned bar"
                );
            }
        }
    }

    #[test]
    fn tapping_an_entry_asks_the_runtime_to_start_it() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        let commands = runner.action(action_id(ENTRIES[0].name));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Launch(name) if name == ENTRIES[0].name)));
        assert!(matches!(runner.app().view, View::Starting(0)));
    }
}
