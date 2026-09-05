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

use kobo_sdk::{
    action_id, ActionId, AppInfo, BandAlign, Context, DeviceIdentity, DeviceRequest, DeviceResult,
    Glyph, KoboApp, ScreenBuilder, SlotWidth, Tile, TileShape, TileState,
};
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
        name: "settings",
        title: "Settings",
        label: "Settings",
        summary: "Connect Wi-Fi, headphones, speakers and keyboards.",
        needs: "Changes the device's Wi-Fi and Bluetooth radios.",
        glyph: Glyph::Settings,
    },
    Entry {
        name: "store",
        title: "App Store",
        label: "App Store",
        summary: "Install, update and remove public Cobalt apps over Wi-Fi.",
        needs: "Uses Wi-Fi to refresh Cobalt's signed GitHub app catalog.",
        glyph: Glyph::Download,
    },
    Entry {
        name: "terminal",
        title: "Terminal",
        label: "Terminal",
        summary: "A shell on the panel, with keys that send rather than collect.",
        needs: "Runs commands on this device. Nothing it does survives a reboot.",
        glyph: Glyph::Terminal,
    },
];

#[derive(Default)]
enum View {
    #[default]
    Home,
    Apps,
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
    /// The last entry handed the panel, if any.
    ///
    /// Leaving an application no longer stops it -- the brief keeps fetching
    /// and the terminal keeps its shell -- so the one the owner most recently
    /// opened is, as far as this launcher can honestly say, still running
    /// behind it. Marked on its tile rather than claimed in words, because the
    /// launcher has no telemetry to say more than "you left this open".
    working: Option<usize>,
    /// Store-installed applications discovered from verified manifests.
    installed: Vec<AppInfo>,
    /// Runtime-owned model and version facts, absent until the daemon replies.
    identity: Option<DeviceIdentity>,
    /// Context menu for the focal continuation tile.
    details: Option<usize>,
}

impl Launcher {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(context),
            View::Apps => self.apps(context),
            View::Starting(index) => self.starting(index),
            View::Leaving => Self::leaving(),
        };
        context.set_screen(screen);
    }

    /// The entries on each page, for the panel this is actually running on.
    ///
    /// Asked of the runtime rather than assumed. Six fit a Clara and more fit
    /// a Sage, and an application that picked a number would be wrong on every
    /// panel but one.
    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let pages = context.paginate_tiles(self.entry_count(), TileShape::Square, true);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }

    fn continue_panel(
        screen: ScreenBuilder,
        entry: DisplayEntry,
        caption: &'static str,
        continuing: bool,
    ) -> ScreenBuilder {
        screen
            .section("Continue")
            .section_link("continue-details", "Details")
            .tile_grid(
                TileShape::Card,
                [(
                    opening(&entry.name),
                    entry.label,
                    entry.glyph,
                    move |tile: Tile| {
                        let tile = tile
                            .with_caption(caption)
                            .with_menu(action_id("continue-details"));
                        if continuing {
                            tile.with_state(TileState::Busy)
                        } else {
                            tile
                        }
                    },
                )],
            )
    }

    fn featured_entries(&self, excluding: usize) -> Vec<(DisplayEntry, bool)> {
        (0..self.entry_count())
            .filter(|index| *index != excluding)
            .take(4)
            .map(|index| (self.entry(index), self.working == Some(index)))
            .collect()
    }

    fn featured_panel(screen: ScreenBuilder, featured: Vec<(DisplayEntry, bool)>) -> ScreenBuilder {
        screen
            .section("Featured")
            .section_link("apps", "View all ↗")
            .tile_grid(
                TileShape::Card,
                featured.into_iter().map(|(entry, busy)| {
                    (
                        opening(&entry.name),
                        entry.label,
                        entry.glyph,
                        move |tile: Tile| {
                            let tile = tile.with_caption(entry.summary);
                            if busy {
                                tile.with_state(TileState::Busy)
                            } else {
                                tile
                            }
                        },
                    )
                }),
            )
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
    /// Folio's desk: one honest continuation, a small featured set, and the
    /// runtime-owned status strip above it. The full catalogue is Apps.
    fn home(&mut self, context: &Context) -> kobo_sdk::Screen {
        let continue_index = self.working.unwrap_or(1);
        let continue_entry = self.entry(continue_index);
        let continue_caption = if self.working.is_some() {
            "Left open · tap to return"
        } else {
            "New here · open the App Store to add your first app"
        };
        let continuing = self.working == Some(continue_index);
        let screen = ScreenBuilder::new("launcher")
            .heading("COBALT")
            .secondary("Your Folio desk")
            .divider();
        let featured = self.featured_entries(continue_index);
        let screen = if context.metrics().width > context.metrics().height {
            let continuation = continue_entry.clone();
            screen.band(
                BandAlign::Top,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(move |slot: ScreenBuilder| {
                            Self::continue_panel(slot, continuation, continue_caption, continuing)
                        })
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Fill,
                        Box::new(move |slot: ScreenBuilder| Self::featured_panel(slot, featured)),
                    ),
                ],
            )
        } else {
            let screen =
                Self::continue_panel(screen, continue_entry.clone(), continue_caption, continuing);
            Self::featured_panel(screen, featured)
        }
        .secondary(self.folio_line())
        .nav_bar_marked(
            0,
            [
                ("home", "Home", Glyph::App),
                ("apps", "Apps", Glyph::App),
                ("settings", "Settings", Glyph::Settings),
                ("reader", "Kobo reader", Glyph::Reader),
            ],
        );
        let screen = if self.details == Some(continue_index) {
            screen.popover(opening(&continue_entry.name), |menu| {
                menu.rows([
                    (
                        opening(&continue_entry.name),
                        format!("Open {}", continue_entry.label),
                        continue_entry.summary.clone(),
                        continue_entry.glyph,
                    ),
                    (
                        "close-details".to_owned(),
                        "Keep browsing".to_owned(),
                        "Dismiss this menu".to_owned(),
                        Glyph::Check,
                    ),
                ])
            })
        } else {
            screen
        };
        screen.build()
    }

    /// The app drawer remains dense and paginated; its rail says where the
    /// reader is without turning the display into a scrolling surface.
    fn apps(&mut self, context: &Context) -> kobo_sdk::Screen {
        let pages = self.pages(context);
        self.page = self.page.min(pages.len() - 1);
        let page = self.page;
        let page_count = u16::try_from(pages.len()).unwrap_or(u16::MAX);
        let page_index = u16::try_from(page).unwrap_or(u16::MAX);
        let page_number = u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX);
        ScreenBuilder::new("launcher-apps")
            .top_bar("Apps")
            .page_rail(page_index, page_count)
            .tile_grid(
                TileShape::Square,
                pages[page].iter().map(|&index| {
                    let entry = self.entry(index);
                    let busy = self.working == Some(index);
                    (
                        opening(&entry.name),
                        entry.label,
                        entry.glyph,
                        move |tile: Tile| {
                            let tile = tile.with_caption(entry.summary);
                            if busy {
                                tile.with_state(TileState::Busy)
                            } else {
                                tile
                            }
                        },
                    )
                }),
            )
            .page_turns("previous", "next")
            .page_position(page_number, page_count)
            .nav_bar_marked(
                1,
                [
                    ("home", "Home", Glyph::App),
                    ("apps", "Apps", Glyph::App),
                    ("settings", "Settings", Glyph::Settings),
                    ("reader", "Kobo reader", Glyph::Reader),
                ],
            )
            .build()
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
    fn starting(&self, index: usize) -> kobo_sdk::Screen {
        let entry = self.entry(index);
        ScreenBuilder::new("launcher-starting")
            .top_bar("Starting")
            .splash(
                Some(entry.glyph),
                entry.title,
                format!("{} {}", entry.summary, entry.needs),
            )
            // A normal recovery action is content-width; the persistent
            // bottom band is reserved for destinations, not this one verb.
            .button("back", "Back")
            .build()
    }

    fn leaving() -> kobo_sdk::Screen {
        ScreenBuilder::new("launcher-leaving")
            .top_bar("Returning")
            .heading("Returning to the Kobo reader")
            .text("The reader takes about half a minute to start and rescan.")
            .build()
    }

    fn entry_count(&self) -> usize {
        ENTRIES.len() + self.installed.len()
    }

    fn folio_line(&self) -> String {
        match &self.identity {
            Some(identity) if !identity.model.is_empty() => {
                format!("{} · {}", identity.runtime_version, identity.model)
            }
            _ => format!("Cobalt {}", env!("CARGO_PKG_VERSION")),
        }
    }

    fn entry(&self, index: usize) -> DisplayEntry {
        if let Some(entry) = ENTRIES.get(index) {
            return DisplayEntry {
                name: entry.name.to_owned(),
                title: entry.title.to_owned(),
                label: entry.label.to_owned(),
                summary: entry.summary.to_owned(),
                needs: entry.needs.to_owned(),
                glyph: entry.glyph,
            };
        }
        let entry = &self.installed[index - ENTRIES.len()];
        DisplayEntry {
            name: entry.id.clone(),
            title: entry.title.clone(),
            label: entry.label.clone(),
            summary: entry.summary.clone(),
            needs: if entry.capabilities.is_empty() {
                "Needs no additional capabilities.".to_owned()
            } else {
                format!("Uses {}.", entry.capabilities.join(", "))
            },
            glyph: entry.glyph,
        }
    }
}

#[derive(Clone)]
struct DisplayEntry {
    name: String,
    title: String,
    label: String,
    summary: String,
    needs: String,
    glyph: Glyph,
}

impl KoboApp for Launcher {
    fn on_start(&mut self, context: &mut Context) {
        context.applications().installed();
        context.device().read_identity();
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
        if let View::Starting(index) = self.view {
            // Control came back while an entry was starting, which on this
            // platform means it is now running behind the launcher rather than
            // gone. Remember which, so its tile can say so.
            self.working = Some(index);
        }
        if !matches!(self.view, View::Home) {
            self.view = View::Home;
        }
        self.details = None;
        context.applications().installed();
        context.device().read_identity();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if (action == ActionId::BACK || action == action_id("close-details"))
            && self.details.take().is_some()
        {
            self.show(context);
            return;
        }
        if action == action_id("home") {
            self.view = View::Home;
            self.show(context);
            return;
        }
        if action == action_id("apps") {
            self.view = View::Apps;
            self.show(context);
            return;
        }
        if action == action_id("continue-details") {
            self.details = Some(self.working.unwrap_or(1));
            self.show(context);
            return;
        }
        if action == action_id("settings") {
            self.view = View::Starting(0);
            self.show(context);
            context.launch(ENTRIES[0].name);
            return;
        }
        if action == action_id("next") || action == action_id("previous") {
            let pages = self.pages(context).len();
            self.page = if action == action_id("next") {
                // Clamped rather than wrapped, to match a bar that only offers
                // a direction there is a page in. Nothing dispatches this at
                // the end any more, and if something did, standing still is
                // the honest answer: the screen is unchanged, so the runner
                // drops the repaint and the panel does not flash.
                (self.page + 1).min(pages - 1)
            } else {
                self.page.saturating_sub(1)
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
        if let Some(index) = (0..self.entry_count())
            .position(|index| action == action_id(&opening(&self.entry(index).name)))
        {
            // Paint first, then ask. The runtime stops this application to
            // start the other one, so this is the last chance to leave
            // something on the panel explaining the wait.
            self.details = None;
            self.view = View::Starting(index);
            self.show(context);
            context.launch(self.entry(index).name);
        }
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        match (request, result) {
            (DeviceRequest::ListInstalledApps, DeviceResult::Apps { mut entries }) => {
                entries.sort_by(|left, right| {
                    left.title
                        .to_ascii_lowercase()
                        .cmp(&right.title.to_ascii_lowercase())
                        .then_with(|| left.id.cmp(&right.id))
                });
                self.installed = entries;
                self.page = self.page.min(self.pages(context).len() - 1);
                self.show(context);
            }
            (DeviceRequest::ReadIdentity, DeviceResult::Identity(identity)) => {
                self.identity = Some(identity);
                self.show(context);
            }
            _ => {}
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
    use kobo_sdk::{
        action_id, AppInfo, AppRunner, Command, DeviceIdentity, DeviceRequest, DeviceResult, Glyph,
        Lifecycle,
    };
    use kobo_ui::{
        render_with, tone, Chrome, DisplayMetrics, LayoutKind, Node, Surface, TextScale, TileShape,
        TileState, CLARA_BW_METRICS,
    };

    fn panels() -> Vec<(String, DisplayMetrics)> {
        kobo_profile::SUPPORTED_PROFILES
            .iter()
            .flat_map(|profile| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width fits layout"),
                    height: i32::try_from(profile.height).expect("profile height fits layout"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale: TextScale::Default,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (format!("{} portrait", profile.id), portrait),
                    (format!("{} landscape", profile.id), landscape),
                ]
            })
            .collect()
    }

    fn painted(commands: Vec<Command>) -> kobo_sdk::Screen {
        repainted(commands).expect("a screen was painted")
    }

    #[test]
    fn a_store_installed_app_appears_and_launches_by_manifest_id() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![AppInfo {
                id: "word-count".to_owned(),
                title: "Word Count".to_owned(),
                label: "Words".to_owned(),
                summary: "Counts words in a note.".to_owned(),
                version: "1.0.0".to_owned(),
                minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
                glyph: Glyph::Note,
                capabilities: Vec::new(),
                installed_version: Some("1.0.0".to_owned()),
            }],
        });
        assert_eq!(runner.app().installed[0].id, "word-count");
        let commands = runner.action(action_id(&opening("word-count")));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Launch(id) if id == "word-count")));
    }

    /// The screen these commands would put on the panel, if any.
    ///
    /// `None` is a real answer: the runner drops a screen identical to the one
    /// already showing, so a "next" that wraps a single-page catalogue back
    /// onto itself sends nothing at all. A test that insists on a screen there
    /// is asserting a repaint nobody wanted.
    fn repainted(commands: Vec<Command>) -> Option<kobo_sdk::Screen> {
        commands.into_iter().find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen),
            _ => None,
        })
    }

    #[test]
    fn the_way_back_to_the_reader_is_on_every_page_of_every_panel() {
        // The one control that must never be unreachable. Placed at the end of
        // the flow it is the first thing a long catalogue pushes off the
        // bottom, and the layout engine drops what does not fit in silence,
        // so the failure would look like a launcher that cannot be left.
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut seen = 0;
            let mut screen = painted(runner.start());
            loop {
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                let reader = action_id("reader");
                let found = layout.nodes.iter().any(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::Button(action, ..) | LayoutKind::NavDestination(action, ..)
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
    fn a_page_turn_is_offered_only_where_there_is_a_page_to_turn_to() {
        // Two labels for one destination is what this catches. With the two
        // pages this catalogue actually has, a bar that showed both directions
        // on every page sent "Previous" and "More apps" to the same screen,
        // and on the last page "More apps" promised applications that were not
        // there and jumped back to the first page instead.
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut screen = painted(runner.start());
            let mut page = 0;
            loop {
                let offered = bar_actions(&screen, &metrics);
                assert_eq!(
                    offered.contains(&action_id("previous")),
                    page > 0,
                    "{name}: page {page} offers the wrong backward control"
                );
                assert!(
                    offered.contains(&action_id("reader")),
                    "{name}: page {page} has no way back to the reader"
                );
                if !offered.contains(&action_id("next")) {
                    break;
                }
                screen = repainted(runner.action(action_id("next"))).unwrap_or_else(|| {
                    panic!("{name}: page {page} offered More apps and painted nothing")
                });
                page += 1;
                assert!(page <= ENTRIES.len(), "{name}: the pages never ran out");
            }
            // And back down, which must arrive at the first page and stop
            // offering to go further.
            while page > 0 {
                screen = repainted(runner.action(action_id("previous"))).unwrap_or_else(|| {
                    panic!("{name}: page {page} offered Previous and painted nothing")
                });
                page -= 1;
            }
            assert!(
                !bar_actions(&screen, &metrics).contains(&action_id("previous")),
                "{name}: the first page still offers Previous"
            );
        }
    }

    /// Every action reachable from the pinned bottom band of a screen.
    fn bar_actions(screen: &kobo_sdk::Screen, metrics: &DisplayMetrics) -> Vec<kobo_ui::ActionId> {
        screen
            .layout_with(metrics, &Chrome::with_back(false))
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Button(action, ..) | LayoutKind::NavDestination(action, ..) => {
                    Some(action)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_entry_appears_on_exactly_one_page() {
        // An entry that lands on no page is an application that cannot be
        // started at all, and one on two pages is a list that never ends.
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut runs = 0;
            let mut found = Vec::new();
            runner.start();
            let mut screen = painted(runner.action(action_id("apps")));
            loop {
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                for node in &layout.nodes {
                    if let LayoutKind::Tile(action, _) = node.kind {
                        found.push(action);
                    }
                }
                runs += 1;
                if runs > ENTRIES.len() {
                    break;
                }
                if let Some(next) = repainted(runner.action(action_id("next"))) {
                    screen = next;
                }
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
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            runner.start();
            let screen = painted(runner.action(action_id("apps")));
            let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
            let rows = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
                .collect::<Vec<_>>();
            assert!(!rows.is_empty(), "{name}: the first page drew no entries");
            let floor = metrics.height - metrics.nav_bar_height();
            for row in &rows {
                assert!(
                    row.rect.y + row.rect.height <= floor,
                    "{name}: an entry ran under the pinned bar"
                );
            }
            let first_y = rows[0].rect.y;
            let columns = rows
                .iter()
                .take_while(|tile| tile.rect.y == first_y)
                .count();
            assert!(
                (2..=4).contains(&columns),
                "{name}: the app drawer chose {columns} columns"
            );
        }
    }

    #[test]
    fn folio_home_keeps_its_complete_hierarchy_on_every_panel() {
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let screen = painted(runner.start());
            let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
            let sections = layout
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::Section)
                .count();
            let tiles = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
                .count();
            let destinations = layout
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
                    )
                })
                .count();
            assert_eq!(sections, 2, "{name}: a Folio section was dropped");
            assert_eq!(tiles, ENTRIES.len(), "{name}: a Folio card was dropped");
            assert_eq!(destinations, 4, "{name}: launcher navigation changed");
            for label in ["Details", "View all ↗"] {
                let sources = layout
                    .nodes
                    .iter()
                    .flat_map(|node| &node.text_lines)
                    .filter(|line| line.as_str() == label)
                    .count();
                assert_eq!(sources, 1, "{name}: {label:?} has {sources} render sources");
            }
            for link in layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::SectionLink(_)))
            {
                assert!(
                    link.rect.x >= layout.content.x
                        && link.rect.y >= layout.content.y
                        && link.rect.x + link.rect.width <= layout.content.x + layout.content.width
                        && link.rect.y + link.rect.height
                            <= layout.content.y + layout.content.height,
                    "{name}: {:?} is outside the content bounds",
                    link.kind
                );
            }
        }
    }

    #[test]
    fn launcher_home_and_apps_are_tappable_and_render_on_every_panel() {
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let home = painted(runner.start());
            let apps = painted(runner.action(action_id("apps")));
            for (view, screen) in [("home", home), ("apps", apps)] {
                let chrome = Chrome::with_back(false);
                let layout = screen.layout_with(&metrics, &chrome);
                let controls = layout
                    .nodes
                    .iter()
                    .filter(|node| node.kind.acts_on().is_some())
                    .collect::<Vec<_>>();
                for (index, control) in controls.iter().enumerate() {
                    assert!(
                        control.rect.width >= metrics.touch_target_minimum()
                            && control.rect.height >= metrics.touch_target_minimum(),
                        "{name} {view}: {:?} is smaller than a touch target",
                        control.kind
                    );
                    for other in &controls[index + 1..] {
                        if control.id != other.id {
                            assert!(
                                control.rect.intersection(other.rect).is_none(),
                                "{name} {view}: {:?} overlaps {:?}",
                                control.kind,
                                other.kind
                            );
                        }
                    }
                }

                let mut surface = Surface::new(
                    usize::try_from(metrics.width).expect("positive profile width"),
                    usize::try_from(metrics.height).expect("positive profile height"),
                );
                render_with(&screen, &metrics, &chrome, &mut surface, None);
                assert!(
                    surface.pixels.iter().any(|pixel| *pixel != tone::PAPER),
                    "{name} {view}: rendering produced a blank frame"
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
        for (name, metrics) in panels() {
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
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListInstalledApps))));
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

    /// An application the owner opened keeps running behind the launcher on
    /// this platform, so the one they most recently left is marked on its tile
    /// rather than drawn as if it were idle. The mark is a fact about the tile,
    /// carried by the tile, so the label stays the application's own name.
    #[test]
    fn the_app_you_last_opened_is_marked_running() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.action(action_id(&opening(ENTRIES[0].name)));
        runner.lifecycle(Lifecycle::Background);
        let commands = runner.lifecycle(Lifecycle::Foreground);
        let screen = commands
            .into_iter()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("coming back repaints the list");
        let busy = screen.nodes.iter().any(|node| match node {
            Node::TileGrid { tiles, .. } => tiles.iter().any(|tile| tile.state == TileState::Busy),
            _ => false,
        });
        assert!(busy, "the application left running was drawn as if idle");
    }

    #[test]
    fn nothing_is_drawn_past_the_edge_of_the_panel() {
        for (name, metrics) in panels() {
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

    #[test]
    fn folio_navigation_reaches_apps_and_keeps_the_reader_exit_visible() {
        let mut runner = AppRunner::new(Launcher::default());
        let home = painted(runner.start());
        assert!(home.nodes.iter().any(|node| matches!(
            node,
            Node::TileGrid {
                shape: TileShape::Card,
                ..
            }
        )));
        let apps = painted(runner.action(action_id("apps")));
        assert!(matches!(runner.app().view, View::Apps));
        let layout = apps.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(layout.nodes.iter().any(|node| {
            matches!(node.kind, LayoutKind::NavDestinationSelected(action, _)
                if action == action_id("apps"))
        }));
        assert!(layout.nodes.iter().any(|node| {
            matches!(node.kind, LayoutKind::NavDestination(action, _)
                if action == action_id("reader"))
        }));
    }

    #[test]
    fn folio_line_only_names_runtime_identity_when_the_daemon_supplied_it() {
        let mut launcher = Launcher::default();
        assert_eq!(
            launcher.folio_line(),
            format!("Cobalt {}", env!("CARGO_PKG_VERSION"))
        );
        launcher.identity = Some(DeviceIdentity {
            model: "Kobo Clara BW".to_owned(),
            runtime_version: "0.3.4".to_owned(),
            ..DeviceIdentity::default()
        });
        assert_eq!(launcher.folio_line(), "0.3.4 · Kobo Clara BW");
    }
}
