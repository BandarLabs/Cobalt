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

use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, ScreenBuilder, TileShape};
use std::process::ExitCode;

/// One entry in the launcher.
///
/// `name` is the action identity and is never shown; `title` is shown and is
/// never used for identity. Keeping those apart means renaming a label cannot
/// silently change what a tap does.
struct Entry {
    name: &'static str,
    /// What the splash calls it. May be as long as it needs to be.
    title: &'static str,
    /// What the tile calls it. A cell is about 25 millimetres wide, so a name
    /// longer than a couple of words is ellipsised into something nobody can
    /// read; a separate short form is more honest than trimming the real one.
    label: &'static str,
    summary: &'static str,
    /// What starting it costs the device, in one sentence. A launcher that
    /// starts something without saying what it will reach for is asking the
    /// owner to find out afterwards. Said on the splash, under the name,
    /// while it starts -- which is the last moment it is still useful.
    needs: &'static str,
    glyph: Glyph,
}

const ENTRIES: &[Entry] = &[
    Entry {
        name: "gutenbird",
        title: "Gutenbird",
        label: "Gutenbird",
        summary: "Sixty thousand free books from Project Gutenberg.",
        needs: "Needs the network while searching and downloading.",
        glyph: Glyph::Book,
    },
    Entry {
        name: "terminal",
        title: "Terminal",
        label: "Terminal",
        summary: "A shell on the panel, with keys that send rather than collect.",
        needs: "Runs commands on this device. Nothing it does survives a reboot.",
        glyph: Glyph::Terminal,
    },
    Entry {
        name: "gallery",
        title: "Components",
        label: "Components",
        summary: "Every UI primitive on real hardware, for checking by eye.",
        needs: "Runs entirely on the device.",
        glyph: Glyph::Chart,
    },
    Entry {
        name: "tictactoe",
        title: "Tic-tac-toe",
        label: "Tic-tac-toe",
        summary: "Two players, one panel. Nought goes first.",
        needs: "Runs entirely on the device.",
        glyph: Glyph::Grid,
    },
    Entry {
        name: "brief",
        title: "Daily Brief",
        label: "Daily Brief",
        summary: "Collects the day's stories while you read something else.",
        needs: "Needs the network in the background, on a schedule the runtime sets.",
        glyph: Glyph::Clock,
    },
    Entry {
        name: "todo",
        title: "Todo",
        label: "Todo",
        summary: "A list that remembers itself. Tap an item to finish it.",
        needs: "Keeps its list in this application's own storage.",
        glyph: Glyph::Check,
    },
    Entry {
        name: "chat",
        title: "AI Command Center",
        label: "AI Chat",
        summary: "Ask a question and tap the answer, rather than typing one.",
        needs: "Needs the network, and a key you provide.",
        glyph: Glyph::Chat,
    },
    Entry {
        name: "hn",
        title: "Hacker News",
        label: "Hacker News",
        summary: "Top, New, Ask and Show, with whole comment threads.",
        needs: "Needs the network while loading stories and threads.",
        glyph: Glyph::News,
    },
    Entry {
        name: "rss",
        title: "Feeds",
        label: "Feeds",
        summary: "Follow a site by name and read its articles, not its layout.",
        needs: "Needs the network while searching and while fetching a feed. Keeps the list of feeds you follow in this application's own storage.",
        glyph: Glyph::Rss,
    },
];

#[derive(Default)]
enum View {
    #[default]
    Home,
    /// A tile was tapped and the runtime has been asked to start it. The
    /// screen says so, because the panel is slow enough that a tap with no
    /// visible answer reads as a tap that was missed.
    Starting(usize),
    Leaving,
}

/// The action a tile carries.
///
/// Distinct from the entry name on purpose, so that the identity which starts
/// an application and the identity which merely describes it can never be
/// confused by a rename.
fn opening(name: &str) -> String {
    format!("open-{name}")
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
        let pages = context.paginate_tiles(ENTRIES.len(), TileShape::Square, true);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    /// The home screen: a grid of icons and names, and nothing else.
    ///
    /// Tiles rather than rows, which is a reversal. Rows were chosen because a
    /// tile said only three words while a row also carried the summary; the
    /// answer to that is not a denser row but a second screen. A phone home
    /// screen shows an icon and a name and costs a tap to learn more, and it
    /// is the arrangement every reader already knows. What the grid buys is
    /// that the catalogue is recognisable at a glance instead of read.
    ///
    /// A tap starts the application. There used to be a screen in between,
    /// carrying the description and a pair of buttons, so that a brush against
    /// the grid could not cost half a minute of an application starting; in
    /// practice it cost a deliberate tap every single time and taught nobody
    /// anything they had not learnt on the first. What it was protecting
    /// against is now handled where it belongs: the splash names what is
    /// starting and carries the way back, so a mistaken tap is one tap to
    /// undo.
    fn home(&mut self, context: &Context) -> kobo_sdk::Screen {
        let pages = Self::pages(context);
        self.page = self.page.min(pages.len() - 1);
        let showing = &pages[self.page];
        let title = if pages.len() > 1 {
            format!("Cobalt \u{2014} {} of {}", self.page + 1, pages.len())
        } else {
            "Cobalt".to_owned()
        };
        let screen = ScreenBuilder::new("launcher").top_bar(title).tiles(
            showing
                .iter()
                .map(|&index| &ENTRIES[index])
                .map(|entry| (opening(entry.name), entry.label, entry.glyph)),
        );
        // The way out is pinned to the panel rather than placed after the
        // list. Layout reserves the bar before any content, so however many
        // entries there are, the button that gives the device back is on the
        // screen. Put at the end of the flow it would be the first thing a
        // long catalogue pushed off the bottom.
        if pages.len() > 1 {
            screen
                .nav_bar(
                    None,
                    [
                        ("previous", "Previous"),
                        ("reader", "Return to Kobo reader"),
                        ("next", "More apps"),
                    ],
                )
                .build()
        } else {
            // One page, so there is nothing to turn and a bar would be two
            // thirds empty. The way out becomes the one pinned control
            // instead, which occupies exactly the band the grid was measured
            // against. As a trailing button it did not: a rule, two gaps and a
            // finger-high control need more room than a bar does, and the
            // difference went over the bottom edge of the panel, where the
            // renderer clipped it in silence.
            screen
                .bottom_action("reader", "Return to Kobo reader")
                .build()
        }
    }

    /// Painted between tapping a tile and that application appearing.
    ///
    /// Centred, and the mark from the tile that was tapped, so the screen
    /// reads as the thing that was asked for rather than as a page of text
    /// about it. The name and the sentence are here because this is now the
    /// only place either is said; the grid has room for a label and nothing
    /// else.
    ///
    /// It carries a way back even though it is normally on the panel for under
    /// a second, because every way this screen can fail to be replaced ends
    /// with the reader looking at it. The runtime deliberately does not end
    /// the session when a launch cannot be satisfied (that would cost the
    /// owner the reader, half a minute and every other running application
    /// over one missing entry) and it has no way to tell the launcher either,
    /// so a missing binary, an application that exits before it draws, or one
    /// that is simply slow all leave this screen up. One button answers all of
    /// them.
    fn starting(index: usize) -> kobo_sdk::Screen {
        let entry = &ENTRIES[index];
        ScreenBuilder::new("launcher-starting")
            .top_bar("Starting")
            .top_bar_action("back", "Back")
            .splash(
                Some(entry.glyph),
                entry.title,
                format!("{} {}", entry.summary, entry.needs),
            )
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

    /// Returns to the list whenever the panel comes back to the launcher.
    ///
    /// Leaving an entry paints "Starting…" so the wait is explained, and the
    /// runtime repaints a returning application from the last screen it drew
    /// rather than waiting for a new one, which is what makes coming back
    /// instant. Together those two correct decisions meant that tapping back
    /// out of an application landed on a "Starting…" screen for an application
    /// that had already started and already finished, with no way forward. The
    /// transient has to be cleared by the only party that knows it was a
    /// transient.
    fn on_foreground(&mut self, context: &mut Context) {
        if !matches!(self.view, View::Home) {
            self.view = View::Home;
        }
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
            .position(|entry| action == action_id(&opening(entry.name)))
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
    use super::{opening, Launcher, View, ENTRIES};
    use kobo_sdk::{action_id, AppRunner, Command, Lifecycle};
    use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, Node, TextScale, CLARA_BW_METRICS};

    const PANELS: [(&str, DisplayMetrics); 3] = [
        ("clara-bw", CLARA_BW_METRICS),
        (
            "nia",
            DisplayMetrics {
                width: 758,
                height: 1024,
                pixels_per_inch: 212,
                text_scale: TextScale::Default,
            },
        ),
        (
            "sage",
            DisplayMetrics {
                width: 1440,
                height: 1920,
                pixels_per_inch: 300,
                text_scale: TextScale::Default,
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
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                let reader = action_id("reader");
                let found = layout.nodes.iter().any(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::Button(action, ..) | LayoutKind::NavDestination(action)
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
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                for node in &layout.nodes {
                    if let LayoutKind::Tile(action) = node.kind {
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
                    .map(|entry| action_id(&opening(entry.name)))
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
    fn every_tile_on_a_page_is_drawn_rather_than_dropped() {
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let screen = painted(runner.start());
            let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
            let rows = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(_)))
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

    /// A tap on a tile starts the application, and the splash it lands on says
    /// which one and what it will reach for. There used to be a screen in
    /// between with the description and an Open button; it cost a deliberate
    /// tap every time and the description is just as readable while the thing
    /// is starting.
    #[test]
    fn tapping_a_tile_starts_the_entry_and_says_what_is_starting() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        let commands = runner.action(action_id(&opening(ENTRIES[0].name)));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Launch(name) if name == ENTRIES[0].name)),
            "a tile tap did not start the application"
        );
        assert!(matches!(runner.app().view, View::Starting(0)));
        let shown = painted(commands);
        let layout = shown.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        let words = layout
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let head = |text: &str| {
            text.split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(
            words.contains(ENTRIES[0].title),
            "the splash did not name what is starting: {words}"
        );
        assert!(
            words.contains(&head(ENTRIES[0].summary)),
            "the description was not shown: {words}"
        );
        // What it will reach for is the one thing worth knowing before it is
        // running, so it does not get dropped along with the details screen.
        assert!(
            words.contains(&head(ENTRIES[0].needs)),
            "what it needs was not shown: {words}"
        );
    }

    /// The splash is centred, which is the reason it exists rather than being
    /// a heading and a paragraph. Four words ranged left from the top of a
    /// panel read as a page that failed to load.
    #[test]
    fn the_splash_is_centred_on_every_panel() {
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::new(Launcher::default());
            runner.start();
            let shown = painted(runner.action(action_id(&opening(ENTRIES[0].name))));
            let layout = shown.layout_with(&metrics, &Chrome::with_back(false));
            let title = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::SplashTitle))
                .unwrap_or_else(|| panic!("{name}: the splash was not laid out"));
            let slack = (metrics.width - title.rect.width) / 2;
            assert!(
                (title.rect.x - slack).abs() <= 2,
                "{name}: the splash is not centred across the panel"
            );
            let mark = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::SplashGlyph(_)))
                .unwrap_or_else(|| panic!("{name}: the splash carried no mark"));
            assert!(
                mark.rect.y > metrics.height / 8,
                "{name}: the splash was drawn against the top of the panel"
            );
        }
    }

    /// A launch the runtime cannot satisfy leaves the panel on this screen and
    /// tells the launcher nothing, by design, ending the session over one
    /// missing entry would cost the owner the reader and every other running
    /// application. So the screen itself has to offer the way out.
    #[test]
    fn the_starting_screen_is_never_a_dead_end() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        let commands = runner.action(action_id(&opening(ENTRIES[0].name)));
        let painted = commands
            .iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen.clone()),
                _ => None,
            })
            .expect("leaving paints an explanation");
        let layout = painted.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::Button(..) | LayoutKind::BarAction(_))),
            "nothing on this screen goes anywhere, and the runtime will not \
             repaint it if the application never arrives"
        );

        runner.action(action_id("back"));
        assert!(
            matches!(runner.app().view, View::Home),
            "the way back did not lead back"
        );
    }

    /// Tapping back out of an application used to land on "Starting Terminal",
    /// for a terminal that had already started and already been left, with no
    /// control on the screen that went anywhere. The runtime repaints a
    /// returning application from the last screen it drew (that is what makes
    /// coming back instant) so the transient has to be cleared here.
    #[test]
    fn coming_back_from_an_application_shows_the_list_again() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.action(action_id(&opening(ENTRIES[0].name)));
        assert!(matches!(runner.app().view, View::Starting(0)));

        runner.lifecycle(Lifecycle::Background);
        let commands = runner.lifecycle(Lifecycle::Foreground);

        assert!(
            matches!(runner.app().view, View::Home),
            "the launcher stayed on the screen it painted while leaving"
        );
        let painted = commands
            .iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("coming back has to repaint, or the stale screen stays on the panel");
        assert!(
            painted
                .nodes
                .iter()
                .any(|node| matches!(node, Node::TileGrid { .. })),
            "the list came back without any entries on it"
        );
    }

    /// The defect this test exists for shipped: the way back to the reader was
    /// drawn from y=1335 to y=1453 on a panel 1448 pixels tall, so its bottom
    /// five pixels (and any descender in the label) were clipped by the edge
    /// of the screen. Nothing failed; the renderer simply stops at the panel.
    #[test]
    fn nothing_is_drawn_past_the_edge_of_the_panel() {
        for (name, metrics) in PANELS {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut screen = painted(runner.start());
            for page in 0..=ENTRIES.len() {
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                for node in &layout.nodes {
                    let bottom = node.rect.y + node.rect.height;
                    assert!(
                        bottom <= metrics.height,
                        "{name}: {:?} on page {page} ends at {bottom}, past the panel's {}",
                        node.kind,
                        metrics.height
                    );
                    let right = node.rect.x + node.rect.width;
                    assert!(
                        right <= metrics.width,
                        "{name}: {:?} on page {page} ends at {right}, past the panel's {}",
                        node.kind,
                        metrics.width
                    );
                }
                let commands = runner.action(action_id("next"));
                if commands.is_empty() {
                    break;
                }
                screen = painted(commands);
            }
        }
    }
}
