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
    action_id, ActionId, AppInfo, Capability, Context, DeviceIdentity, DeviceRequest, DeviceResult,
    Glyph, KoboApp, ScreenBuilder, Tile, TileShape,
};
use std::process::ExitCode;

mod notices;

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
    /// Reached from the bar at the foot of every page rather than from a tile.
    ///
    /// Settings and Books each have a tab there. A tile as well was a second
    /// door to the same room, and it spent two of the first page's places on
    /// things the owner could already reach without turning a page.
    on_tab_bar: bool,
}

/// Where Settings sits in [`ENTRIES`].
///
/// Named because a navigation destination starts it by position, and a bare
/// index in that code says nothing about which application it will run.
const SETTINGS: usize = 0;
/// Where Books sits in [`ENTRIES`].
///
/// The Books tab is a launch, not a view this application draws. A missing
/// handler left the tab on the bar as a control that answered with nothing.
const BOOKS: usize = 1;

const ENTRIES: &[Entry] = &[
    Entry {
        name: "settings",
        title: "Settings",
        label: "Settings",
        summary: "Connect Wi-Fi, headphones, speakers and keyboards.",
        needs: "Changes the device's Wi-Fi and Bluetooth radios.",
        glyph: Glyph::Settings,
        on_tab_bar: true,
    },
    Entry {
        name: "books",
        title: "Books",
        label: "Books",
        summary: "The documents already on this device.",
        needs: "Reads the books already on the card. It does not write.",
        glyph: Glyph::Book,
        on_tab_bar: true,
    },
    Entry {
        name: "store",
        title: "App Store",
        label: "App Store",
        summary: "Install, update and remove public Cobalt apps over Wi-Fi.",
        needs: "Uses Wi-Fi to refresh Cobalt's signed GitHub app catalog.",
        glyph: Glyph::Download,
        on_tab_bar: false,
    },
    Entry {
        name: "terminal",
        title: "Terminal",
        label: "Terminal",
        summary: "A shell on the panel, with keys that send rather than collect.",
        needs: "Runs commands on this device. File changes can survive a reboot.",
        glyph: Glyph::Terminal,
        on_tab_bar: false,
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
    /// The Notifications list, on this page, perhaps asking whether to clear
    /// it.
    Notices {
        page: usize,
        asking: bool,
    },
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
    /// The runtime's notices, absent until it sends them. Absent also means
    /// a runtime that keeps none, so no bell is drawn and nothing about
    /// notices is ever asked of it.
    notices: Option<kobo_sdk::NoticeList>,
    /// What was new when the owner opened the list, marked as new for the
    /// rest of that visit though the runtime has been told it was seen.
    fresh: Vec<u64>,
    /// Why the last row tapped could not be opened, said over the list.
    notice_trouble: Option<&'static str>,
}

impl Launcher {
    fn show(&mut self, context: &mut Context) {
        let screen = match self.view {
            View::Home => self.home(context),
            View::Starting(index) => self.starting(index),
            View::Leaving => Self::leaving(),
            View::Notices { page, asking } => notices::screen(
                context,
                self.notices
                    .as_ref()
                    .unwrap_or(&kobo_sdk::NoticeList::default()),
                &self.fresh,
                page,
                asking,
                self.notice_trouble,
            ),
        };
        context.set_screen(screen);
    }

    /// The entries on each page, for the panel this is actually running on.
    ///
    /// Asked of the runtime rather than assumed. Six fit a Clara and more fit
    /// a Sage, and an application that picked a number would be wrong on every
    /// panel but one.
    fn pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let tiles = self.tiles();
        let pages = context.paginate_tiles(tiles.len(), TileShape::Square, true);
        if pages.is_empty() {
            return vec![Vec::new()];
        }
        pages
            .into_iter()
            .map(|page| page.into_iter().map(|place| tiles[place]).collect())
            .collect()
    }

    /// The entries that get a tile, in the order they are drawn.
    fn tiles(&self) -> Vec<usize> {
        (0..self.entry_count())
            .filter(|&index| ENTRIES.get(index).is_none_or(|entry| !entry.on_tab_bar))
            .collect()
    }

    fn app_grid(screen: ScreenBuilder, entries: Vec<(DisplayEntry, bool)>) -> ScreenBuilder {
        screen.tile_grid(
            TileShape::Square,
            entries.into_iter().map(|(entry, busy)| {
                (
                    opening(&entry.name),
                    entry.label,
                    entry.glyph,
                    // Every tile carries a caption line, blank but for the
                    // one left running. A grid reserves the line for all its
                    // tiles once any has one, so a caption that came and went
                    // moved the whole grid each time an app was left open.
                    move |tile: Tile| tile.with_caption(if busy { "Resume" } else { " " }),
                )
            }),
        )
    }

    /// The home screen: a compact resume target followed by app icons.
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
        // Home is every application on the device, paged the way the drawer
        // is. It used to be a short row of six with the wordmark set as
        // display type above it, which spent the top eighth of the panel on
        // our own name and then hid most of what the reader came here to
        // open. The name now sits in the running head at the same size every
        // other screen titles itself with, because a launcher's subject is
        // the applications, not the launcher.
        let pages = self.pages(context);
        self.page = self.page.min(pages.len() - 1);
        let page = self.page;
        let page_count = u16::try_from(pages.len()).unwrap_or(u16::MAX);
        let page_number = u16::try_from(page.saturating_add(1)).unwrap_or(u16::MAX);
        let entries = pages[page]
            .iter()
            .map(|&index| (self.entry(index), self.working == Some(index)))
            .collect();
        // "1 of 5" and its arrows say where the reader is and how to turn.
        // A rail down the side said the first again and narrowed the grid.
        let mut screen = ScreenBuilder::new("launcher").top_bar("Cobalt");
        // The bell first, so it holds the trailing edge, and the count of new
        // ones beside it, so the place the owner looks never moves.
        if let Some(list) = &self.notices {
            screen = screen.top_bar_glyph(notices::OPEN_LIST, "Notifications", Glyph::Bell);
            if let Some(label) = notices::unread_label(list) {
                screen = screen.top_bar_action(notices::OPEN_LIST, label);
            }
        }
        Self::app_grid(screen, entries)
            .page_turns("previous", "next")
            .page_position(page_number, page_count)
            .nav_bar_marked(
                0,
                [
                    ("home", "Apps", Glyph::App),
                    ("books", "Books", Glyph::Book),
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
            needs: needs(&entry.capabilities),
            glyph: entry.glyph,
        }
    }
}

/// What a Store app will reach for, in the words the Store used for it.
///
/// This used to print the manifest's own identifiers, so the splash under a
/// newly installed app read "Uses keep-awake, scheduled-wake." The wording
/// now comes from the runtime's policy crate, the same sentences the Store
/// and `kobo apps` show.
fn needs(capabilities: &[String]) -> String {
    if capabilities.is_empty() {
        return "It needs no special permissions.".to_owned();
    }
    capabilities
        .iter()
        .map(|name| {
            Capability::parse(name).map_or_else(
                || format!("Another permission ({name})."),
                |capability| format!("{}.", capability.plain_wording()),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    /// The platform's own blue: this screen is Cobalt itself rather than an
    /// application running on it, and it wears the colour of the logo.
    fn accent(&self) -> Option<kobo_sdk::Accent> {
        Some(kobo_sdk::Accent::Cobalt)
    }

    fn on_start(&mut self, context: &mut Context) {
        context.applications().installed();
        context.device().read_identity();
        self.show(context);
    }

    /// Puts the list back the moment the panel is handed over.
    ///
    /// Leaving an entry paints "Starting…" so the wait is explained, and the
    /// runtime repaints a returning application from the last screen it drew
    /// rather than waiting for a new one, which is what makes coming back
    /// instant. Together those two correct decisions meant that tapping back
    /// out of an application landed on a "Starting…" screen for an application
    /// that had already started and already finished.
    ///
    /// Clearing it on the way back in is a round trip too late: the runtime has
    /// already repainted what it held, and on this panel that is a full refresh
    /// of a splash for something that started a minute ago. So the launcher
    /// clears the transient on the way out instead, while it still owns what
    /// the runtime will hold.
    ///
    /// Only losing the panel means the launch succeeded. A missing binary or an
    /// application that exits before it draws never takes the panel, the
    /// launcher is never told to go behind, and the splash stays up with the
    /// way back on it, which is exactly what that screen is for.
    fn on_background(&mut self, context: &mut Context) {
        let View::Starting(index) = self.view else {
            return;
        };
        // The entry took the panel, which on this platform means it is now
        // running behind the launcher rather than gone. Remember which, so its
        // tile can say so.
        self.working = Some(index);
        self.view = View::Home;
        self.show(context);
    }

    /// Refreshes the list whenever the panel comes back to the launcher.
    ///
    /// The catalogue can have changed while the panel was elsewhere: the Store
    /// is one of the applications the owner can leave for, and it installs.
    fn on_foreground(&mut self, context: &mut Context) {
        if !matches!(self.view, View::Home | View::Notices { .. }) {
            self.view = View::Home;
        }
        context.applications().installed();
        context.device().read_identity();
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.notice_action(context, action) {
            return;
        }
        if action == action_id("home") {
            // Apps tapped while already on Apps goes to its first page, as
            // the tab a reader is on does everywhere else.
            if matches!(self.view, View::Home) {
                self.page = 0;
            }
            self.view = View::Home;
            self.show(context);
            return;
        }
        if action == action_id("settings") {
            self.view = View::Starting(SETTINGS);
            self.show(context);
            context.launch(ENTRIES[SETTINGS].name);
            return;
        }
        if action == action_id("books") {
            self.view = View::Starting(BOOKS);
            self.show(context);
            context.launch(ENTRIES[BOOKS].name);
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
            self.view = View::Starting(index);
            self.show(context);
            context.launch(self.entry(index).name);
        }
    }

    fn on_notices(&mut self, context: &mut Context, list: kobo_sdk::NoticeList) {
        let open = list.open;
        self.notices = Some(list);
        if open {
            self.open_notices(context);
            return;
        }
        if let View::Notices { .. } = self.view {
            // Something arrived while the list was open: it has been seen.
            self.note_fresh(context);
        }
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: DeviceRequest,
        result: DeviceResult,
    ) {
        if let (DeviceRequest::OpenNotice { .. }, DeviceResult::Failed(_)) = (&request, &result) {
            self.notice_trouble = Some("That article is no longer on this reader.");
            self.show(context);
            return;
        }
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

impl Launcher {
    /// Opens the list and tells the runtime it has been seen.
    fn open_notices(&mut self, context: &mut Context) {
        self.fresh.clear();
        self.notice_trouble = None;
        self.note_fresh(context);
        self.view = View::Notices {
            page: 0,
            asking: false,
        };
        self.show(context);
    }

    /// Keeps the mark on whatever is new for this visit and tells the
    /// runtime it has been seen.
    fn note_fresh(&mut self, context: &mut Context) {
        let Some(list) = &self.notices else {
            return;
        };
        let unread: Vec<u64> = list
            .entries
            .iter()
            .filter(|notice| notice.unread)
            .map(|notice| notice.id)
            .collect();
        if !unread.is_empty() {
            self.fresh.extend(unread);
            context.notices().mark_read();
        }
    }

    /// Everything the list does. Returns whether `action` was one of its own.
    fn notice_action(&mut self, context: &mut Context, action: ActionId) -> bool {
        if self.notices.is_none() {
            return false;
        }
        if action == action_id(notices::OPEN_LIST) {
            self.open_notices(context);
            return true;
        }
        let View::Notices { page, asking } = self.view else {
            return false;
        };
        let list = self.notices.clone().unwrap_or_default();
        if action == ActionId::BACK || (action == action_id(notices::KEEP) && !asking) {
            self.view = View::Home;
        } else if action == action_id(notices::KEEP) {
            self.view = View::Notices {
                page,
                asking: false,
            };
        } else if action == action_id(notices::CLEAR) {
            self.view = View::Notices { page, asking: true };
        } else if action == action_id(notices::CLEAR_CONFIRMED) {
            context.notices().clear();
            self.fresh.clear();
            if let Some(list) = &mut self.notices {
                list.entries.clear();
            }
            self.view = View::Notices {
                page: 0,
                asking: false,
            };
        } else if action == action_id(notices::NEXT) || action == action_id(notices::PREVIOUS) {
            let pages = notices::pages(context, &list, &self.fresh).len();
            let page = if action == action_id(notices::NEXT) {
                (page + 1).min(pages.saturating_sub(1))
            } else {
                page.saturating_sub(1)
            };
            self.view = View::Notices {
                page,
                asking: false,
            };
        } else if let Some(notice) = list
            .entries
            .iter()
            .find(|notice| action == action_id(&notices::opening(notice.id)))
        {
            // A row whose article is gone already says so, and one with
            // nothing to open is news and nothing more; neither goes anywhere.
            if notice.target != kobo_sdk::NoticeTarget::Ready {
                return true;
            }
            // Read now, so the row no longer says New when Back from the
            // article comes to this list again.
            let id = notice.id;
            self.fresh.retain(|fresh| *fresh != id);
            if let Some(held) = self
                .notices
                .as_mut()
                .and_then(|held| held.entries.iter_mut().find(|held| held.id == id))
            {
                held.unread = false;
            }
            self.notice_trouble = None;
            context.notices().open(id);
            return true;
        } else {
            return false;
        }
        self.show(context);
        true
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
    use super::{opening, Entry, Launcher, View, BOOKS, ENTRIES, SETTINGS};
    use kobo_sdk::{
        action_id, AppInfo, AppRunner, Command, DeviceRequest, DeviceResult, Glyph, Lifecycle,
    };
    use kobo_ui::{
        render_with, tone, Chrome, DisplayMetrics, LayoutKind, Node, Surface, TextScale, TileShape,
        CLARA_BW_METRICS,
    };

    /// Where the App Store sits in [`ENTRIES`]: the first entry with a tile.
    const STORE: usize = 2;

    /// The entries the grid draws, which is every one without a tab.
    fn tiled() -> Vec<&'static Entry> {
        ENTRIES.iter().filter(|entry| !entry.on_tab_bar).collect()
    }

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
            let mut screen = painted(runner.start());
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
                let first = tiled()
                    .first()
                    .map(|entry| action_id(&opening(entry.name)))
                    .expect("the catalogue is not empty");
                if found.contains(&first) && found.len() >= tiled().len() {
                    break;
                }
            }
            found.sort_unstable();
            found.dedup();
            assert_eq!(
                found.len(),
                tiled().len(),
                "{name}: {} of {} entries were reachable",
                found.len(),
                tiled().len()
            );
        }
    }

    #[test]
    fn every_tile_on_a_page_is_drawn_rather_than_dropped() {
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let screen = painted(runner.start());
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
    fn launcher_home_keeps_its_compact_grid_on_every_panel() {
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
            assert_eq!(sections, 0, "{name}: the compact home grid grew a section");
            assert_eq!(tiles, tiled().len(), "{name}: a home tile was dropped");
            assert_eq!(destinations, 4, "{name}: launcher navigation changed");
        }
    }

    #[test]
    fn launcher_home_uses_a_compact_resume_mark() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.action(action_id(&opening(ENTRIES[STORE].name)));
        // The list the runtime holds while the panel is elsewhere, which is
        // the one it repaints when the owner comes back.
        let screen = painted(runner.lifecycle(Lifecycle::Background));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        let text = layout
            .nodes
            .iter()
            .flat_map(|node| &node.text_lines)
            .map(String::as_str)
            .collect::<Vec<_>>();

        assert!(
            text.contains(&"Resume"),
            "the active app has no resume mark"
        );
        for retired in [
            "Your Folio desk",
            "Continue",
            "Details",
            "Featured",
            "View all ↗",
            "Left open · tap to return",
        ] {
            assert!(
                !text.contains(&retired),
                "{retired:?} remained visible on the home screen"
            );
        }
        assert!(runner
            .action(action_id(&opening(ENTRIES[STORE].name)))
            .iter()
            .any(
                |command| matches!(command, Command::Launch(name) if name == ENTRIES[STORE].name)
            ));
    }

    #[test]
    fn launcher_home_is_tappable_and_renders_on_every_panel() {
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let home = painted(runner.start());
            // Apps is no longer a screen this application draws; it starts the
            // store, which renders its own and is measured by its own tests.
            for (view, screen) in [("home", home)] {
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
        let commands = runner.action(action_id(&opening(ENTRIES[STORE].name)));
        assert!(
            commands.iter().any(
                |command| matches!(command, Command::Launch(name) if name == ENTRIES[STORE].name)
            ),
            "a tile tap did not start the application"
        );
        assert!(matches!(runner.app().view, View::Starting(STORE)));
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
            words.contains(ENTRIES[STORE].title),
            "the splash did not name what is starting: {words}"
        );
        assert!(
            words.contains(&head(ENTRIES[STORE].summary)),
            "the description was not shown: {words}"
        );
        // What it will reach for is the one thing worth knowing before it is
        // running, so it does not get dropped along with the intermediate screen.
        assert!(
            words.contains(&head(ENTRIES[STORE].needs)),
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
    /// control on the screen that went anywhere.
    ///
    /// The list is now left behind on the way out, so coming back has nothing
    /// to redraw and asks no refresh of the panel. What it does do is ask the
    /// runtime what is installed, because the Store is one of the places the
    /// owner can have been.
    #[test]
    fn coming_back_from_an_application_shows_the_list_again() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.action(action_id(&opening(ENTRIES[0].name)));
        assert!(matches!(runner.app().view, View::Starting(0)));

        let left_behind = painted(runner.lifecycle(Lifecycle::Background));
        let commands = runner.lifecycle(Lifecycle::Foreground);

        assert!(
            matches!(runner.app().view, View::Home),
            "the launcher stayed on the screen it painted while leaving"
        );
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ListInstalledApps))));
        assert!(
            left_behind
                .nodes
                .iter()
                .any(|node| matches!(node, Node::TileGrid { .. })),
            "the list came back without any entries on it"
        );
    }

    /// Clearing the transient when the panel comes back is a round trip too
    /// late. The runtime repaints a returning application from the screen it
    /// holds, and what it held for the launcher was "Starting…", so backing
    /// out of an application spent a full E Ink refresh on a splash for
    /// something that had already started before the list replaced it. The
    /// screen the runtime holds has to be the list before the launcher is ever
    /// asked for it.
    #[test]
    fn leaving_for_an_application_leaves_the_list_behind_not_the_splash() {
        let mut runner = AppRunner::new(Launcher::default());
        let list = painted(runner.start()).id;
        runner.action(action_id(&opening(ENTRIES[0].name)));

        let commands = runner.lifecycle(Lifecycle::Background);

        let held = commands
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("the launcher left the splash as the screen the runtime holds");
        assert_eq!(
            held.id, list,
            "coming back repaints this, and it is not the list"
        );
        assert!(
            matches!(runner.app().view, View::Home),
            "the launcher stayed on the screen it painted while leaving"
        );
    }

    /// An application the owner opened keeps running behind the launcher on
    /// this platform. Home marks it for resume while the complete Apps view
    /// retains the busy state.
    #[test]
    fn the_active_app_is_marked_for_resume_on_home() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.action(action_id(&opening(ENTRIES[STORE].name)));
        let home = painted(runner.lifecycle(Lifecycle::Background));
        let home_layout = home.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        let home_text = home_layout
            .nodes
            .iter()
            .flat_map(|node| &node.text_lines)
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert!(
            home_text.contains(&"Resume"),
            "the active app lost its Home resume marker"
        );
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
    fn the_grid_is_the_selected_tab_and_the_reader_exit_stays_visible() {
        let mut runner = AppRunner::new(Launcher::default());
        let home = painted(runner.start());
        assert!(home.nodes.iter().any(|node| matches!(
            node,
            Node::TileGrid {
                shape: TileShape::Square,
                ..
            }
        )));
        let layout = home.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        // The grid is what the Apps tab shows, so that tab is the marked one.
        assert!(layout.nodes.iter().any(|node| {
            matches!(node.kind, LayoutKind::NavDestinationSelected(action, _)
                if action == action_id("home"))
        }));
        for tab in ["books", "settings", "reader"] {
            assert!(
                layout.nodes.iter().any(|node| {
                    matches!(node.kind, LayoutKind::NavDestination(action, _)
                        if action == action_id(tab))
                }),
                "the {tab} tab was not drawn"
            );
        }
    }

    #[test]
    fn the_books_tab_starts_the_books_application() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        let commands = runner.action(action_id("books"));
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::Launch(name) if name == "books")),
            "the Books tab did not start the books application: {commands:?}"
        );
        assert!(
            matches!(runner.app().view, View::Starting(BOOKS)),
            "the Books tab did not leave a starting screen"
        );
        assert_eq!(ENTRIES[BOOKS].name, "books");
    }
    #[test]
    fn terminal_launch_does_not_promise_reboot_will_undo_file_changes() {
        let app = Launcher::default();
        let terminal = app.entry(3);
        assert_eq!(terminal.name, "terminal");
        assert!(terminal.needs.contains("File changes can survive a reboot"));
        assert!(!terminal.needs.contains("Nothing it does survives"));
    }

    #[test]
    fn settings_and_books_are_reached_from_their_tabs_and_not_also_from_tiles() {
        for (name, metrics) in panels() {
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            let mut screen = painted(runner.start());
            for _ in 0..=ENTRIES.len() {
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                for duplicate in [SETTINGS, BOOKS] {
                    let tile = action_id(&opening(ENTRIES[duplicate].name));
                    assert!(
                        !layout
                            .nodes
                            .iter()
                            .any(|node| matches!(node.kind, LayoutKind::Tile(action, _) if action == tile)),
                        "{name}: {} has a tile as well as a tab",
                        ENTRIES[duplicate].title
                    );
                }
                match repainted(runner.action(action_id("next"))) {
                    Some(next) => screen = next,
                    None => break,
                }
            }
        }
    }

    /// Names of catalog apps long enough to test a tile at large sizes.
    const LONG_NAMES: [&str; 4] = ["Backgammon", "Flashcards", "Kitchen card", "Nonograms"];

    /// The runtime's answer listing `names` as installed apps.
    fn installed(names: &[&str]) -> DeviceResult {
        DeviceResult::Apps {
            entries: names
                .iter()
                .map(|name| AppInfo {
                    id: name.to_lowercase().replace(' ', "-"),
                    title: (*name).to_owned(),
                    label: (*name).to_owned(),
                    summary: "An app.".to_owned(),
                    version: "1.0.0".to_owned(),
                    minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
                    glyph: Glyph::App,
                    capabilities: Vec::new(),
                    installed_version: Some("1.0.0".to_owned()),
                })
                .collect(),
        }
    }

    /// Each tile name on every page, as the panel draws it at `metrics`,
    /// with [`LONG_NAMES`] installed.
    fn drawn_names(metrics: DisplayMetrics) -> Vec<Vec<String>> {
        let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
        runner.start();
        let mut screen = painted(runner.device_result(installed(&LONG_NAMES)));
        let mut names = Vec::new();
        loop {
            let layout = kobo_ui::with_text_scale(metrics.text_scale, || {
                screen.layout_with(&metrics, &Chrome::with_back(false))
            });
            names.extend(
                layout
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.kind, LayoutKind::TileLabel))
                    .map(|node| node.text_lines.clone()),
            );
            match repainted(runner.action(action_id("next"))) {
                Some(next) if next != screen => screen = next,
                _ => break,
            }
        }
        names
    }

    #[test]
    fn no_tile_name_is_broken_part_way_through_a_word_at_any_text_size() {
        let labels: Vec<&str> = tiled()
            .iter()
            .map(|entry| entry.label)
            .chain(LONG_NAMES)
            .collect();
        for scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            for lines in drawn_names(metrics) {
                let joined = lines.join(" ");
                assert!(
                    labels.contains(&joined.as_str()) || joined.ends_with('\u{2026}'),
                    "{scale:?}: {lines:?} splits a word"
                );
            }
        }
    }

    #[test]
    fn no_page_of_the_grid_raises_a_layout_warning() {
        // Every page past the first used to be warned of a fifth ink, counted
        // from tile cells that draw nothing of their own.
        let mut runner = AppRunner::with_metrics(Launcher::default(), CLARA_BW_METRICS);
        runner.start();
        let mut screen = painted(runner.device_result(installed(&LONG_NAMES)));
        loop {
            let issues = screen
                .diagnostics(&CLARA_BW_METRICS, &Chrome::with_back(false))
                .issues;
            assert!(issues.is_empty(), "{issues:?}");
            match repainted(runner.action(action_id("next"))) {
                Some(next) if next != screen => screen = next,
                _ => break,
            }
        }
    }

    #[test]
    fn leaving_an_app_running_moves_no_tile_on_home() {
        let tiles = |screen: &kobo_sdk::Screen| {
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false))
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::TileLabel))
                .map(|node| node.rect)
                .collect::<Vec<_>>()
        };
        let mut runner = AppRunner::new(Launcher::default());
        let before = tiles(&painted(runner.start()));
        runner.action(action_id(&opening(ENTRIES[STORE].name)));
        let after = tiles(&painted(runner.lifecycle(Lifecycle::Background)));
        assert_eq!(before, after);
    }

    #[test]
    fn tapping_apps_while_on_apps_goes_back_to_its_first_page() {
        let metrics = DisplayMetrics {
            text_scale: TextScale::Default,
            ..CLARA_BW_METRICS
        };
        let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: (0..12)
                .map(|index| AppInfo {
                    id: format!("app-{index}"),
                    title: format!("App {index}"),
                    label: format!("App {index}"),
                    summary: "An app.".to_owned(),
                    version: "1.0.0".to_owned(),
                    minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
                    glyph: Glyph::App,
                    capabilities: Vec::new(),
                    installed_version: Some("1.0.0".to_owned()),
                })
                .collect(),
        });
        runner.action(action_id("next"));
        assert_eq!(
            runner.app().page,
            1,
            "the grid has a second page to turn to"
        );
        runner.action(action_id("home"));
        assert_eq!(runner.app().page, 0);
    }

    #[test]
    fn a_store_app_says_what_it_needs_in_words_rather_than_identifiers() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.device_result(DeviceResult::Apps {
            entries: vec![AppInfo {
                id: "frame".to_owned(),
                title: "Frame".to_owned(),
                label: "Frame".to_owned(),
                summary: "Show photos from your computer.".to_owned(),
                version: "1.0.0".to_owned(),
                minimum_cobalt_version: env!("CARGO_PKG_VERSION").to_owned(),
                glyph: Glyph::App,
                capabilities: vec!["keep-awake".to_owned(), "scheduled-wake".to_owned()],
                installed_version: Some("1.0.0".to_owned()),
            }],
        });
        let shown = painted(runner.action(action_id(&opening("frame"))));
        let words = shown
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false))
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(words.contains("Can keep the reader awake"), "{words}");
        assert!(!words.contains("keep-awake"), "{words}");
    }

    fn notice(
        id: u64,
        title: &str,
        unread: bool,
        target: kobo_sdk::NoticeTarget,
    ) -> kobo_sdk::Notice {
        kobo_sdk::Notice {
            id,
            heading: "Article arrived".to_owned(),
            title: title.to_owned(),
            detail: "example.com".to_owned(),
            glyph: Glyph::Book,
            at: kobo_sdk::clock::Clock::now(&kobo_sdk::clock::SystemClock::reader())
                .map_or(0, |now| now.unix_millis),
            unread,
            target,
        }
    }

    fn kept(open: bool) -> kobo_sdk::NoticeList {
        kobo_sdk::NoticeList {
            entries: vec![
                notice(2, "Notes on bread", true, kobo_sdk::NoticeTarget::Ready),
                notice(
                    1,
                    "A walk in the hills",
                    false,
                    kobo_sdk::NoticeTarget::Gone,
                ),
            ],
            open,
        }
    }

    fn says(screen: &kobo_sdk::Screen, words: &str) -> bool {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false))
            .nodes
            .iter()
            .any(|node| node.text_lines.iter().any(|line| line.contains(words)))
    }

    #[test]
    fn the_bell_appears_only_once_the_runtime_has_said_it_keeps_notices() {
        let mut runner = AppRunner::new(Launcher::default());
        let home = painted(runner.start());
        assert!(
            !says(&home, "Notifications"),
            "a bell under a runtime that keeps none"
        );
        let home = painted(runner.notices(kept(false)));
        assert!(says(&home, "Notifications"));
        assert!(says(&home, "1 new"));
        // Nothing was asked of the runtime just for being told.
        assert!(!runner
            .notices(kept(false))
            .iter()
            .any(|command| matches!(command, Command::Device(_))));
    }

    #[test]
    fn opening_the_list_marks_it_read_but_keeps_what_was_new_marked_for_this_visit() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.notices(kept(false));
        let commands = runner.action(action_id("notifications"));
        assert!(commands
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::MarkNoticesRead))));
        let list = painted(commands);
        assert!(says(&list, "Today"));
        assert!(says(&list, "Notes on bread"));
        assert!(says(&list, "New"));
        assert!(says(&list, "No longer on this reader"));
        // The runtime answers with everything read, and the screen does not
        // move: the mark stays for the rest of this visit.
        let mut read = kept(false);
        for entry in &mut read.entries {
            entry.unread = false;
        }
        assert!(repainted(runner.notices(read)).is_none());
    }

    #[test]
    fn a_card_standing_for_several_opens_the_list_from_anywhere_in_the_launcher() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        let list = painted(runner.notices(kept(true)));
        assert!(says(&list, "Clear all"));
        assert!(matches!(runner.app().view, View::Notices { .. }));
    }

    #[test]
    fn a_row_opens_its_article_and_a_row_whose_article_is_gone_does_nothing() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.notices(kept(true));
        assert!(runner.app().fresh.contains(&2));
        let opened = runner.action(action_id(&super::notices::opening(2)));
        assert!(opened.iter().any(|command| matches!(
            command,
            Command::Device(DeviceRequest::OpenNotice { id: 2 })
        )));
        // Back from the article finds the row no longer marked New.
        assert!(!runner.app().fresh.contains(&2));
        assert!(runner
            .app()
            .notices
            .as_ref()
            .and_then(|list| list.entries.iter().find(|notice| notice.id == 2))
            .is_some_and(|notice| !notice.unread));
        let gone = runner.action(action_id(&super::notices::opening(1)));
        assert!(!gone
            .iter()
            .any(|command| matches!(command, Command::Device(_))));
    }

    #[test]
    fn clearing_the_list_asks_first_and_keeping_changes_nothing() {
        let mut runner = AppRunner::new(Launcher::default());
        runner.start();
        runner.notices(kept(true));
        let asking = painted(runner.action(action_id("clear-notifications")));
        assert!(says(&asking, "Clear all notifications?"));
        let kept_list = runner.action(action_id("keep-notifications"));
        assert!(!kept_list
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ClearNotices))));
        assert!(says(&painted(kept_list), "Notes on bread"));
        runner.action(action_id("clear-notifications"));
        let cleared = runner.action(action_id("clear-notifications-confirmed"));
        assert!(cleared
            .iter()
            .any(|command| matches!(command, Command::Device(DeviceRequest::ClearNotices))));
        assert!(says(&painted(cleared), "Nothing new"));
    }

    #[test]
    fn the_list_and_the_bell_fit_every_panel_at_the_largest_text() {
        for (name, metrics) in panels() {
            let metrics = DisplayMetrics {
                text_scale: TextScale::Largest,
                ..metrics
            };
            let mut runner = AppRunner::with_metrics(Launcher::default(), metrics);
            runner.start();
            let home = painted(runner.notices(kept(false)));
            let layout = home.layout_with(&metrics, &Chrome::with_back(false));
            let bell = action_id("notifications");
            assert!(
                layout
                    .nodes
                    .iter()
                    .any(|node| node.kind.acts_on() == Some(bell)),
                "{name}: no bell"
            );
            assert!(
                layout
                    .nodes
                    .iter()
                    .any(|node| node.kind == LayoutKind::TopBarTitle
                        && node.text_lines.iter().any(|line| line == "Cobalt")),
                "{name}: the bell pushed the title out"
            );
            let list = painted(runner.action(bell));
            let layout = list.layout_with(&metrics, &Chrome::with_back(true));
            for action in [
                action_id("clear-notifications"),
                action_id(&super::notices::opening(2)),
            ] {
                assert!(
                    layout
                        .nodes
                        .iter()
                        .any(|node| node.kind.acts_on() == Some(action)),
                    "{name}: {action:?} is not reachable"
                );
            }
        }
    }
}
