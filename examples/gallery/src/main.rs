//! Every UI primitive, on one device, so each can be checked by eye.
//!
//! This is a test instrument as much as a demonstration. If a primitive looks
//! wrong here it looks wrong everywhere, and the layout tests only prove that
//! sizes are right, not that the result is worth reading.

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BandAlign, BannerLevel, Context, Glyph, KoboApp, LogLevel, PictureHandle,
    RowLead, Screen, ScreenBuilder, SlotWidth, Task, TaskId, TaskOutcome, Tile, TilePicture,
    TileShape, TileState,
};
use std::process::ExitCode;

/// Every icon the system draws, with the name it is called by.
///
/// Derived from `Glyph::ALL` rather than written out, because the hand-written
/// table drifted: the enum grew from eighteen glyphs to twenty-nine and this
/// list stayed at eighteen, so eleven icons shipped without anyone ever having
/// looked at one on a panel. A table that cannot be out of date is worth more
/// than a prettier caption.
fn icons() -> Vec<(String, String, Glyph)> {
    Glyph::ALL
        .iter()
        .map(|glyph| {
            let name = format!("{glyph:?}");
            (format!("icon-{}", name.to_lowercase()), name, *glyph)
        })
        .collect()
}

/// The options the choice offers, named once so that what is drawn, what a tap
/// means, and which row is marked can never disagree.
const FILINGS: [(&str, &str); 4] = [
    ("file-keep", "Keep for later"),
    ("file-share", "Share it"),
    ("file-archive", "Archive"),
    ("file-discard", "Discard"),
];

/// The step wedge: sixteen bands, one per grey the panel can actually resolve.
///
/// This is the instrument that tab exists for. A gradient of 256 values says
/// nothing on a display that quantises to sixteen; sixteen flat bands show
/// immediately whether the ends are clipping and whether the middle separates
/// at all under the reading light in the room.
const WEDGE_WIDTH: u32 = 320;
const WEDGE_HEIGHT: u32 = 96;

fn wedge() -> Vec<u8> {
    let mut grey = Vec::with_capacity((WEDGE_WIDTH * WEDGE_HEIGHT) as usize);
    for _ in 0..WEDGE_HEIGHT {
        for x in 0..WEDGE_WIDTH {
            let step = x * 16 / WEDGE_WIDTH;
            // 0, 17, 34 ... 255: the sixteen levels, evenly spaced.
            grey.push(u8::try_from(step * 17).unwrap_or(u8::MAX));
        }
    }
    grey
}

/// Something cover-shaped, for the portrait tile beside it.
const CARD_WIDTH: u32 = 190;
const CARD_HEIGHT: u32 = 300;

fn card() -> Vec<u8> {
    let mut grey = Vec::with_capacity((CARD_WIDTH * CARD_HEIGHT) as usize);
    for y in 0..CARD_HEIGHT {
        for x in 0..CARD_WIDTH {
            let border = x < 8 || y < 8 || x >= CARD_WIDTH - 8 || y >= CARD_HEIGHT - 8;
            let band = (y / 24) % 2 == 0;
            grey.push(if border {
                0
            } else if band {
                0xEE
            } else {
                0x66
            });
        }
    }
    grey
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Tab {
    #[default]
    Text,
    Lists,
    Input,
    States,
    Work,
}

impl Tab {
    /// Five, and five is the ceiling: `max_nav_destinations` will not draw a
    /// sixth on a six inch panel, and a bar that silently drops its last
    /// destination is worse than a bar that never offered it. Everything past
    /// five is reached with `tabs()` inside a page, which is what `tabs()` is
    /// for.
    const ALL: [(Self, &'static str, &'static str); 5] = [
        (Self::Text, "tab-text", "Type"),
        (Self::Lists, "tab-lists", "Lists"),
        (Self::Input, "tab-input", "Input"),
        (Self::States, "tab-states", "States"),
        (Self::Work, "tab-work", "Work"),
    ];

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|(tab, _, _)| *tab == self)
            .unwrap_or(0)
    }

    /// The pages inside this tab, when one panel will not hold it.
    fn pages(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Text => &[
                ("page-type", "Type"),
                ("page-quotes", "Quotes"),
                ("page-tone", "Tone"),
            ],
            Self::Lists => &[
                ("page-rows", "Rows"),
                ("page-tiles", "Tiles"),
                ("page-covers", "Covers"),
                ("page-icons", "Icons"),
            ],
            // Four, and four is the ceiling: `MAX_TABS` is 4, and a strip that
            // drops its fifth entry leaves a page with no way to reach it.
            Self::Input => &[
                ("page-groups", "Groups"),
                ("page-input", "Fields"),
                ("page-choice", "Choice"),
                ("page-over", "Overlays"),
            ],
            Self::States => &[
                ("page-nothing", "Empty"),
                ("page-offline", "Offline"),
                ("page-denied", "Denied"),
                ("page-trouble", "Error"),
            ],
            // One state per page, because a standard state is a splash and a
            // splash centres itself in the whole of what is left. Two of them
            // on one panel is two half-pages, which is neither.
            Self::Work => &[
                ("page-transfer", "Transfer"),
                ("page-request", "Requests"),
                ("page-splash", "Waiting"),
            ],
        }
    }
}

/// What is floating above the screen, when something is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Floating {
    Menu,
    Sheet,
    Confirm,
}

struct Gallery {
    tab: Tab,
    /// Which page of the current tab is showing.
    ///
    /// One slot per tab rather than one number, so that leaving a tab and
    /// coming back returns to the page that was open. A tab bar that resets
    /// its own sub-page on every visit makes a five-tab app feel like it keeps
    /// forgetting where you were.
    page: [usize; Tab::ALL.len()],
    /// The two pictures, once the runtime has been given them.
    ///
    /// `None` until then, and a screen drawn while they are `None` is a
    /// perfectly good screen, a missing picture is a normal condition in this
    /// system, not an error, which is why every tile keeps its glyph.
    card: Option<TilePicture>,
    swatch: Option<TilePicture>,
    entry: TextEntry,
    answer: Option<String>,
    /// Which page of the icon sheet is showing.
    icon_page: usize,
    /// Which of the three checklist rows have been ticked.
    ticked: [bool; 2],
    /// Which filter chip is on.
    chip: usize,
    /// Whether the pushed detail screen is showing.
    detail: bool,
    /// What, if anything, is floating above the screen.
    ///
    /// One field rather than one bool per overlay, because only one thing can
    /// float at a time and two bools can both be true.
    floating: Option<Floating>,
    /// Bytes of the demonstration download that have arrived.
    received: u64,
    /// Whether that download is pretending to have failed.
    stalled: bool,
    loading: bool,
    task: Option<TaskId>,
    outcome: Option<String>,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            page: [0; Tab::ALL.len()],
            card: None,
            swatch: None,
            // Binding the free-text row here is the whole mechanism: the row
            // drawn by `or_type` emits this action, and the field opens itself
            // when it sees it.
            entry: TextEntry::new().opened_by("file-other"),
            answer: None,
            icon_page: 0,
            ticked: [true, false],
            chip: 0,
            detail: false,
            floating: None,
            received: 1_150_000,
            stalled: false,
            loading: false,
            task: None,
            outcome: None,
        }
    }
}

impl Gallery {
    /// Which sub-page of the tab showing is open, clamped to what that tab has.
    fn page(&self) -> usize {
        let pages = self.tab.pages().len();
        self.page[self.tab.index()].min(pages.saturating_sub(1))
    }

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

        // A pushed screen replaces the panel the same way: it is a whole
        // screen with its own bar, not a panel drawn inside the last one.
        if self.detail {
            context.set_screen(Self::detail_page());
            return;
        }

        let page = self.page();
        let screen = ScreenBuilder::new("gallery").top_bar(match self.tab {
            Tab::Text => "Type and tone",
            Tab::Lists => "Lists, rows and tiles",
            Tab::Input => "Groups, fields and asking",
            Tab::States => "Nothing to show",
            Tab::Work => "Work in flight",
        });

        // The sub-tab strip is drawn by the tab in the SDK rather than by a
        // row of buttons, so the selected page is a state the renderer knows
        // about and can draw as selected, rather than a mark someone wrote
        // into a label.
        let screen = screen.tabs(page, self.tab.pages().iter().copied());

        let screen = match (self.tab, page) {
            (Tab::Text, 0) => Self::type_page(screen),
            (Tab::Text, 1) => Self::quotes_page(screen),
            (Tab::Text, _) => self.tone_page(screen),
            (Tab::Lists, 0) => self.rows_page(screen),
            (Tab::Lists, 1) => self.tiles_page(screen),
            (Tab::Lists, 2) => self.covers_page(screen),
            (Tab::Lists, _) => self.icons_page(screen),
            (Tab::Input, 0) => self.groups_page(screen),
            (Tab::Input, 1) => self.input_page(screen),
            (Tab::Input, 2) => self.choice_page(screen),
            (Tab::Input, _) => self.overlay_page(screen),
            (Tab::States, 0) => Self::nothing_page(screen),
            (Tab::States, 1) => Self::offline_page(screen),
            (Tab::States, 2) => Self::denied_page(screen),
            (Tab::States, _) => Self::trouble_page(screen),
            (Tab::Work, 0) => self.transfer_page(screen),
            (Tab::Work, 1) => self.request_page(screen),
            (Tab::Work, _) => Self::splash_page(screen),
        };

        let screen = screen
            .nav_bar(
                self.tab.index(),
                Tab::ALL.map(|(_, name, label)| (name, label)),
            )
            .build();
        context.set_screen(screen);
    }

    /// Every way the system sets words: the roles, the groupings and the
    /// facts, with no spacers, because the spacing is the component's job.
    fn type_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .heading("Heading")
            .text(
                "Body copy wraps at a measure chosen from the panel's physical width, \
                 not its pixel count.",
            )
            .secondary("Secondary, for the sentence under the sentence.")
            .section("A section title")
            .text("A section is a title with the space around it already decided.")
            .section_with_value("With a value", "42")
            .facts([
                ("Author", "Virginia Woolf"),
                ("Language", "English"),
                ("Downloads", "12,455"),
                ("Format", "EPUB, plain text"),
            ])
    }

    /// Nesting, and the two rules that separate things.
    fn quotes_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .section("Quoting")
            .quote(1, "A reply, set in from what it answers.")
            .quote(2, "A reply to the reply, one level further in.")
            .quote(9, "Past the cap the indent stops moving.")
            .divider()
            .secondary("A divider above, and the page position below the tab bar.")
            .page_position(2, 3)
    }

    /// Tone: the greys the panel resolves, the banners, and the two ways of
    /// showing that something is happening without saying how far along.
    fn tone_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = screen
            .banner(BannerLevel::Info, "An informational banner.")
            .banner(
                BannerLevel::Attention,
                "An attention banner, drawn inverted. This is what replaces flashing \
                 the frontlight.",
            )
            .section("Sixteen greys, one flat band each");
        let screen = match self.card {
            Some(card) => screen.picture(card, 40),
            None => screen.skeleton(2),
        };
        screen
            .section_with_value("Determinate", "65%")
            .progress(65)
            .section("Not yet known")
            .skeleton(3)
            .page_position(3, 3)
    }

    /// A picture beside its metadata, and two columns that stay lined up.
    ///
    /// `hero` is not a node: it is a band with a picture in one slot and a
    /// stack of facts in the other, which is why it stacks by itself when the
    /// panel is too narrow to keep both readable.
    fn groups_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .hero(
                self.swatch,
                28,
                "Mrs Dalloway",
                Some("Virginia Woolf".to_owned()),
                [("Language", "English"), ("Downloads", "12,455")],
            )
            .section("Two columns, aligned")
            .band(
                BandAlign::Middle,
                [
                    (
                        SlotWidth::Fill,
                        Box::new(|screen: ScreenBuilder| screen.text("Fill takes what is left."))
                            as Box<dyn FnOnce(ScreenBuilder) -> ScreenBuilder>,
                    ),
                    (
                        SlotWidth::Natural,
                        Box::new(|screen: ScreenBuilder| screen.secondary("Natural")),
                    ),
                ],
            )
            // Two rows, not three. A third fitted the panel this test used to
            // measure against and was drawn through the navigation bar on the
            // device, and the count in the section header has to be the count
            // of what is under it.
            .section_rows(
                "A section with its own rows",
                Some("2 items".to_owned()),
                [
                    (
                        "grp-one",
                        "First",
                        "with a subtitle",
                        RowLead::from(Glyph::Book),
                    ),
                    (
                        "grp-two",
                        "Second",
                        "and another",
                        RowLead::from(Glyph::Note),
                    ),
                ],
            )
    }

    /// A pushed screen: its own back arrow, and verbs where the navigation was.
    ///
    /// The panel has one bottom band. A screen that navigates cannot also act,
    /// which is not a limitation so much as the shape of the hardware, and it
    /// is why an action bar only ever appears on a screen you arrived at.
    fn detail_page() -> Screen {
        ScreenBuilder::new("gallery")
            .top_bar("First")
            .owns_back(true)
            .heading("First")
            .text(
                "Tapping a row in a section pushes a screen. This one owns its \
                 back arrow, so the runtime hands the press to the application \
                 rather than closing the app.",
            )
            .facts([("Kind", "Row"), ("Section", "A section with its own rows")])
            .action_bar([("grp-save", "Save"), ("grp-share", "Share")])
            .build()
    }

    /// Text entry, filters, and the two-button question.
    fn input_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let typed = self.answer.clone().unwrap_or_default();
        let screen =
            screen
                .section("A field")
                .field("field-open", typed.clone(), "Search the catalogue");
        let screen = if typed.is_empty() {
            screen
        } else {
            screen.field_clear("field-clear")
        };
        screen
            .section("Filters")
            .chips(
                ["Everything", "Unread", "Downloaded"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, label)| (format!("chip-{index}"), label, index == self.chip)),
            )
            .button("open-confirm", "Delete this shelf")
            .compose(|screen| {
                // A confirmation is an overlay, so it is drawn only while it
                // is being asked. Built unconditionally it is a modal on the
                // panel from the moment the page opens, with a scrim over
                // everything else -- which is how this page spent an
                // afternoon appearing to have a dead tab bar.
                if self.floating == Some(Floating::Confirm) {
                    screen.confirm(
                        "Delete this shelf?",
                        "The books stay on the device. Only the shelf goes.",
                        ("confirm-yes", "Delete shelf"),
                        ("confirm-no", "Keep it"),
                    )
                } else {
                    screen
                }
            })
    }

    /// Rows: the trailing value, the picture lead, and the checklist.
    fn rows_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let cover = |glyph| match self.swatch {
            Some(picture) => RowLead::Picture(picture, glyph),
            None => RowLead::Icon(glyph),
        };
        screen
            .section_with_value("Rows with a trailing value", "3")
            .rows_with_trailing([
                (
                    "row-one",
                    "Mrs Dalloway",
                    "Virginia Woolf",
                    cover(Glyph::Book),
                    "12,455",
                ),
                (
                    "row-two",
                    "Ulysses",
                    "James Joyce",
                    cover(Glyph::Book),
                    "8,102",
                ),
                (
                    "row-three",
                    "The Waves",
                    "Virginia Woolf",
                    RowLead::from(Glyph::Book),
                    "3,914",
                ),
            ])
            .section("A checklist")
            .checklist([
                ("tick-0", "Downloaded", "on this device", self.ticked[0]),
                ("tick-1", "Read", "finished last month", self.ticked[1]),
            ])
    }

    /// Tiles: every state a tile can be in, and what a badge does to one.
    fn tiles_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let swatch = self.swatch;
        screen.section("Tiles carry their own state").tile_grid(
            TileShape::Square,
            [
                (
                    "tile-normal",
                    "Normal",
                    Glyph::Book,
                    Box::new(|tile: Tile| tile.with_subtitle("nothing pending"))
                        as Box<dyn FnOnce(Tile) -> Tile>,
                ),
                (
                    "tile-badge",
                    "Unread",
                    Glyph::News,
                    Box::new(|tile: Tile| tile.with_badge("12").with_subtitle("since Tuesday")),
                ),
                (
                    "tile-busy",
                    "Syncing",
                    Glyph::Wifi,
                    Box::new(|tile: Tile| tile.with_state(TileState::Busy)),
                ),
                (
                    "tile-off",
                    "No radio",
                    Glyph::Power,
                    Box::new(|tile: Tile| {
                        tile.with_state(TileState::Unavailable)
                            .with_subtitle("turn wifi on")
                    }),
                ),
                (
                    "tile-held",
                    "Paused",
                    Glyph::Clock,
                    Box::new(|tile: Tile| tile.with_state(TileState::Held)),
                ),
                (
                    "tile-art",
                    "With artwork",
                    Glyph::Book,
                    Box::new(move |tile: Tile| match swatch {
                        Some(picture) => tile.with_picture(picture),
                        None => tile,
                    }),
                ),
            ],
        )
    }

    /// Portrait tiles, which is what a shelf of covers is.
    ///
    /// Its own page because a portrait tile is nearly five hundred pixels
    /// tall: put it under the square grid and the last row of covers falls off
    /// the bottom of the panel, which is exactly what the conformance test
    /// caught the first time this page was written.
    fn covers_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .secondary("One tile with artwork, one still waiting for it.")
            .picture_tiles(
                TileShape::Portrait,
                [
                    ("picture-one", "With artwork", Glyph::Book, self.swatch),
                    ("picture-two", "Still arriving", Glyph::Book, None),
                ],
            )
    }

    /// How many icons fit on one panel without the last row falling off it.
    ///
    /// Two rows of three. Three rows fits to within a few pixels on this panel
    /// and does not on a smaller one, and a sheet that silently drops its last
    /// row is the failure this whole page exists to catch.
    const ICONS_PER_PAGE: usize = 6;

    /// Every icon the system draws, so an icon nobody has looked at cannot
    /// ship.
    ///
    /// Paged with real page turns rather than one long grid: a grid does not
    /// scroll, so twenty-nine icons in one `tiles()` call is twenty-three
    /// icons drawn past the bottom of the panel.
    fn icons_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let all = icons();
        let pages = all.len().div_ceil(Self::ICONS_PER_PAGE);
        let page = self.icon_page.min(pages.saturating_sub(1));
        screen
            .secondary("Every icon the system draws, six to a page.")
            .tiles(
                all.into_iter()
                    .skip(page * Self::ICONS_PER_PAGE)
                    .take(Self::ICONS_PER_PAGE),
            )
            .page_turns("icons-back", "icons-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages).unwrap_or(u16::MAX),
            )
    }

    fn choice_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .section_with_value(
                "Filing",
                self.answer
                    .clone()
                    .unwrap_or_else(|| "not chosen".to_owned()),
            )
            .choose("How should this note be filed?", FILINGS)
            // State rather than a mark in a label: the renderer draws it
            // from the icon atlas, so it exists whatever the face contains.
            .chosen(
                FILINGS
                    .iter()
                    .position(|(_, label)| Some(*label) == self.answer.as_deref())
                    .unwrap_or(usize::MAX),
            )
            .or_type("file-other", "Something else...")
    }

    /// The two things that float above a screen, and the three dots that open
    /// one of them.
    ///
    /// The dismissal is not written here. A popover raised over a scrim is
    /// dismissed by the runtime, which turns a tap anywhere off it into
    /// `ActionId::BACK`; an app that had to notice that tap itself would be an
    /// app that sometimes forgot.
    fn overlay_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = screen
            .top_bar_glyph("bar-search", "Search", Glyph::Search)
            .top_bar_overflow(
                "bar-more",
                self.floating == Some(Floating::Menu),
                [("menu-rename", "Rename"), ("menu-delete", "Delete")],
            )
            .secondary(
                "The three dots in the bar open a menu. Tapping anywhere else closes it, \
                 and the app is not told about that tap.",
            )
            .button("open-sheet", "Open a modal");
        if self.floating == Some(Floating::Sheet) {
            screen.modal("Details", |sheet| {
                sheet
                    .facts([("Format", "EPUB"), ("Size", "1.1 MB")])
                    .button("sheet-close", "Close")
            })
        } else {
            screen
        }
    }

    /// A download, in all four of the states a download is ever in.
    fn transfer_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = screen.section("A download");
        let screen = if self.stalled {
            screen
                .transfer("Mrs Dalloway", self.received, Some(2_400_000))
                .transfer_failed("the connection dropped", true)
                .transfer_retry("xfer-retry", "Resume")
        } else {
            screen
                .transfer("Mrs Dalloway", self.received, Some(2_400_000))
                .cancellable("xfer-cancel", "Cancel")
                .button("xfer-fail", "Pretend it fails")
        };
        screen
            .section("Size not sent by the server")
            .transfer("The Waves", 412_000, None)
    }

    /// Work with no byte count, which is the other half of what a transfer is.
    ///
    /// Its own page rather than the foot of the transfer page: with both on
    /// one panel the last button was drawn through the navigation bar on the
    /// device, and a conformance screen that overflows teaches the overflow.
    fn request_page(&self, screen: ScreenBuilder) -> ScreenBuilder {
        let screen = screen.section("Work with no bytes at all");
        if self.loading {
            screen
                .activity("Fetching the catalogue", None)
                .cancellable("cancel-fetch", "Cancel")
        } else {
            screen
                .secondary(self.outcome.as_deref().unwrap_or("Idle."))
                .button("start-fetch", "Start a request")
        }
    }

    /// The four ways a screen has nothing to show, each of which says
    /// something different about whose problem it is.
    fn nothing_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .empty_state("No books on this shelf. Anything you download lands here.")
            .button("state-browse", "Browse the catalogue")
    }

    /// The same, for a shelf that is only empty because the radio is off.
    fn offline_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .offline_state("No wifi. The shelf is what arrived last time.")
            .button("state-retry", "Try again")
    }

    /// A state nobody can recover from by tapping, so nothing is chained on.
    fn denied_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen.permission_denied_state("This app cannot read the library folder.")
    }

    /// The two states that are somebody's fault, as opposed to merely empty.
    ///
    /// On their own page because each carries an attention banner, and a panel
    /// with four inverted bars on it teaches readers to ignore all of them.
    fn trouble_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen
            .error_state("The catalogue came back malformed. Nothing was changed.")
            .button("state-retry", "Try again")
    }

    /// A mark, a name and a sentence, centred in what is left.
    ///
    /// Alone on its page because that is the only way it is ever right: it
    /// takes the rest of the content area, so anything after it has nowhere to
    /// go.
    fn splash_page(screen: ScreenBuilder) -> ScreenBuilder {
        screen.splash(
            Some(Glyph::Wifi),
            "Looking for the library",
            "This takes a few seconds on a cold radio.",
        )
    }

    /// Raises and lowers the two things that float above a screen.
    ///
    /// Returns whether it took the tap.
    fn overlays(&mut self, context: &mut Context, action: ActionId) -> bool {
        let floating = if action == action_id("bar-more") {
            Some(Floating::Menu)
        } else if action == action_id("open-sheet") {
            Some(Floating::Sheet)
        } else if action == action_id("open-confirm") {
            Some(Floating::Confirm)
        } else if action == action_id("sheet-close")
            || action == action_id("menu-rename")
            || action == action_id("menu-delete")
            || action == action_id("confirm-yes")
            || action == action_id("confirm-no")
        {
            None
        } else {
            return false;
        };
        self.floating = floating;
        self.show(context);
        true
    }

    /// Moves the demonstration download between its four states.
    ///
    /// Returns whether it took the tap.
    fn transfers(&mut self, context: &mut Context, action: ActionId) -> bool {
        let stalled = if action == action_id("xfer-fail") {
            true
        } else if action == action_id("xfer-retry") || action == action_id("xfer-cancel") {
            false
        } else {
            return false;
        };
        self.stalled = stalled;
        self.show(context);
        true
    }

    /// Decoded artwork, at both sizes the system draws it.
    ///
    /// Generated rather than downloaded, so this works with the radio off and
    /// shows exactly the same thing every time: a gallery whose picture
    /// depends on a server is a gallery that is sometimes empty for reasons
    /// that have nothing to do with the renderer.
    ///
    /// Handed over once, on start, rather than on every repaint: a picture is
    /// held by the runtime under its handle, and re-sending it on each paint
    /// would put the whole image back on the wire every time a tab was tapped.
    fn put_pictures(&mut self, context: &mut Context) {
        self.card = context.put_picture(PictureHandle(1), WEDGE_WIDTH, WEDGE_HEIGHT, wedge());
        self.swatch = context.put_picture(PictureHandle(2), CARD_WIDTH, CARD_HEIGHT, card());
    }
}

impl KoboApp for Gallery {
    fn on_start(&mut self, context: &mut Context) {
        self.put_pictures(context);
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

        // A tap off an overlay arrives as BACK, which is the runtime telling
        // us the reader dismissed it. Nothing else in this app owns back, so
        // this is the whole of the dismissal logic.
        if action == ActionId::BACK {
            if self.floating.take().is_some() || self.detail {
                self.detail = false;
                self.show(context);
            }
            return;
        }

        if action == action_id("grp-one") {
            self.detail = true;
            self.show(context);
            return;
        }

        for (index, (name, _)) in self.tab.pages().iter().enumerate() {
            if action == action_id(name) {
                let tab = self.tab.index();
                self.page[tab] = index;
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

        for (name, label) in FILINGS {
            if action == action_id(name) {
                self.answer = Some(label.to_owned());
                self.show(context);
                return;
            }
        }

        if action == action_id("icons-next") {
            let pages = icons().len().div_ceil(Self::ICONS_PER_PAGE);
            self.icon_page = (self.icon_page + 1).min(pages - 1);
            self.show(context);
            return;
        }

        if action == action_id("icons-back") {
            self.icon_page = self.icon_page.saturating_sub(1);
            self.show(context);
            return;
        }

        for index in 0..self.ticked.len() {
            if action == action_id(&format!("tick-{index}")) {
                self.ticked[index] = !self.ticked[index];
                self.show(context);
                return;
            }
        }

        for index in 0..3 {
            if action == action_id(&format!("chip-{index}")) {
                self.chip = index;
                self.show(context);
                return;
            }
        }

        if self.overlays(context, action) || self.transfers(context, action) {
            return;
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

        context.log(LogLevel::Info, format!("unhandled tap: {action:?}"));
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
    use super::{
        card, icons, wedge, Gallery, Tab, CARD_HEIGHT, CARD_WIDTH, WEDGE_HEIGHT, WEDGE_WIDTH,
    };
    use kobo_sdk::{
        action_id, Command, Context, DiagnosticSeverity, Glyph, KoboApp, Node, Screen,
        CLARA_BW_METRICS,
    };

    /// Paints one page and hands back what was drawn.
    ///
    /// `dispatch` drops a `SetScreen` identical to what is already displayed,
    /// so a test that asserts "the tap drew a screen" has to go through the
    /// app's own `show` rather than through an action, or it will occasionally
    /// find nothing and be right to.
    fn painted(gallery: &Gallery) -> Screen {
        let mut context = Context::default();
        gallery.show(&mut context);
        context
            .take_commands()
            .into_iter()
            .rev()
            .find_map(|command| match command {
                Command::SetScreen(screen) => Some(screen),
                _ => None,
            })
            .expect("a screen was painted")
    }

    fn every_page() -> Vec<(String, Screen)> {
        // Building a runner installs the same typeface the runtime lays out
        // with. Without it every measurement here comes from the built-in
        // fallback bitmap, whose lines are about two thirds the height of the
        // real ones -- which is how the first version of this test passed on a
        // page whose last three nodes were off the bottom of the panel.
        let _ = kobo_sdk::AppRunner::new(Gallery::default());
        let mut screens = Vec::new();
        for (tab, _, _) in Tab::ALL {
            for (index, (name, _)) in tab.pages().iter().enumerate() {
                let mut gallery = Gallery::default();
                let mut context = Context::default();
                gallery.on_start(&mut context);
                gallery.tab = tab;
                gallery.page[tab.index()] = index;
                screens.push(((*name).to_owned(), painted(&gallery)));
                // The states that only exist after a tap are pages too: an
                // overlay nobody opens and a failure nobody provokes are two
                // more screens that have never been laid out.
                if tab == Tab::Input && index == 0 {
                    gallery.detail = true;
                    screens.push(("page-detail".to_owned(), painted(&gallery)));
                    gallery.detail = false;
                }
                if tab == Tab::Input && index == 1 {
                    gallery.floating = Some(super::Floating::Confirm);
                    screens.push(("page-input-confirm".to_owned(), painted(&gallery)));
                    gallery.floating = None;
                }
                if tab == Tab::Input && index == 3 {
                    gallery.floating = Some(super::Floating::Menu);
                    screens.push(("page-over-menu".to_owned(), painted(&gallery)));
                    gallery.floating = Some(super::Floating::Sheet);
                    screens.push(("page-over-sheet".to_owned(), painted(&gallery)));
                }
                if tab == Tab::Work && index == 0 {
                    gallery.stalled = true;
                    screens.push(("page-transfer-failed".to_owned(), painted(&gallery)));
                    gallery.stalled = false;
                    gallery.loading = true;
                    screens.push(("page-transfer-busy".to_owned(), painted(&gallery)));
                }
                if tab == Tab::Lists && *name == "page-icons" {
                    let pages = super::icons().len().div_ceil(Gallery::ICONS_PER_PAGE);
                    for icon_page in 1..pages {
                        gallery.icon_page = icon_page;
                        screens.push((format!("page-icons-{icon_page}"), painted(&gallery)));
                    }
                }
            }
        }
        screens
    }

    /// The gallery is the conformance screen: if a page of it lays out with an
    /// error the same mistake is waiting in every app that copies from here.
    ///
    /// Errors only. Warnings are advice and several of them are deliberately
    /// provoked on these pages: the tone page uses more inks than a reading
    /// screen should, because showing them is the point.
    ///
    /// Measured against the status band the runtime draws above every screen.
    /// With a bare chrome the content starts sixty pixels higher than it does
    /// on the device, and that slack is enough to hide a page that overflows:
    /// the groups page passed this test while drawing the last row's subtitle
    /// through the navigation bar on the panel.
    #[test]
    fn every_page_lays_out_without_an_error() {
        let mut failures = Vec::new();
        for (name, screen) in every_page() {
            let errors = screen
                .diagnostics(&CLARA_BW_METRICS, &kobo_sdk::Chrome::measuring(false))
                .issues
                .into_iter()
                .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                .map(|issue| format!("{:?}", issue.kind))
                .collect::<Vec<_>>();
            if !errors.is_empty() {
                failures.push(format!("{name}: {errors:?}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// Every component in the vocabulary is on a panel somewhere here.
    ///
    /// This is what stops the gallery drifting back into a demo. A component
    /// the gallery does not draw is a component nobody has ever seen rendered,
    /// and the layout tests only prove that its rectangle is the right size.
    #[test]
    fn every_node_the_system_has_is_drawn_somewhere() {
        let drawn = every_page()
            .into_iter()
            .flat_map(|(_, screen)| {
                let mut kinds: Vec<String> = screen
                    .nodes
                    .iter()
                    .map(|node| format!("{node:?}"))
                    .map(|text| {
                        text.split(|c: char| !c.is_alphanumeric())
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect();
                if screen.overlay.is_some() {
                    kinds.push("Overlay".to_owned());
                }
                kinds
            })
            .collect::<Vec<_>>();
        for wanted in [
            "Heading",
            "Text",
            "Secondary",
            "Section",
            "Facts",
            "Quote",
            "Divider",
            "Banner",
            "Picture",
            "Progress",
            "Skeleton",
            "Band",
            "Rows",
            "TileGrid",
            "Chips",
            "Tabs",
            "Field",
            "Choice",
            "Activity",
            "Button",
            "Splash",
            "Overlay",
        ] {
            assert!(
                drawn.iter().any(|kind| kind == wanted),
                "Node::{wanted} is never drawn in the gallery"
            );
        }

        // A checklist is `Node::Rows` whose rows carry `done`, not a node of
        // its own, so the only way to prove it is drawn is to find a ticked
        // row and an unticked one.
        let rows = every_page()
            .into_iter()
            .flat_map(|(_, screen)| screen.nodes)
            .filter_map(|node| match node {
                Node::Rows { rows, .. } => Some(rows),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row.state == kobo_sdk::RowState::Done)
                && rows.iter().any(|row| row.state != kobo_sdk::RowState::Done),
            "the checklist is never drawn both ticked and unticked"
        );
        assert!(
            rows.iter().any(|row| row.trailing.is_some()),
            "no row is drawn with a trailing value"
        );
        assert!(
            rows.iter()
                .any(|row| matches!(row.lead, kobo_sdk::RowLead::Picture(..))),
            "no row is drawn with a cover"
        );
    }

    /// Leaving a tab and coming back returns to the page that was open.
    #[test]
    fn a_tab_remembers_which_of_its_pages_was_open() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("tab-lists"));
        gallery.on_action(&mut context, action_id("page-icons"));
        assert_eq!(gallery.page(), 3);
        gallery.on_action(&mut context, action_id("tab-text"));
        assert_eq!(gallery.page(), 0, "a different tab inherited the page");
        gallery.on_action(&mut context, action_id("tab-lists"));
        assert_eq!(gallery.page(), 3, "the tab forgot where it was");
    }

    /// A tap off an overlay arrives as BACK, and closes it.
    ///
    /// The app never sees the coordinates: the runtime raises the scrim and
    /// turns anything that misses the popover into `ActionId::BACK`. An app
    /// that had to notice that tap itself is an app that sometimes leaves a
    /// menu stuck open.
    #[test]
    fn tapping_away_from_the_menu_closes_it() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("tab-input"));
        gallery.on_action(&mut context, action_id("page-over"));
        gallery.on_action(&mut context, action_id("bar-more"));
        assert_eq!(
            gallery.floating,
            Some(super::Floating::Menu),
            "the three dots opened nothing"
        );
        assert!(painted(&gallery).overlay.is_some(), "no overlay was raised");
        gallery.on_action(&mut context, kobo_sdk::ActionId::BACK);
        assert_eq!(gallery.floating, None, "the menu stayed up");
        assert!(painted(&gallery).overlay.is_none());
    }

    #[test]
    fn the_answer_already_given_is_marked_on_the_row_that_gave_it() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        gallery.on_action(&mut context, action_id("tab-input"));
        gallery.on_action(&mut context, action_id("page-choice"));
        gallery.on_action(&mut context, action_id("file-archive"));
        let choice = painted(&gallery)
            .nodes
            .iter()
            .find_map(|node| match node {
                Node::Choice { selected, .. } => Some(*selected),
                _ => None,
            })
            .expect("the ask tab offers a choice");
        assert_eq!(choice, Some(2));
    }

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

    /// The gallery is a test instrument, so an icon it does not draw is an
    /// icon nobody has ever looked at on real hardware.
    ///
    /// The table is generated from `Glyph::ALL`, so this cannot fail by
    /// omission any more; what it still catches is two glyphs whose names
    /// collide into one action, which would make one of them untappable.
    #[test]
    fn every_glyph_is_on_the_panel_somewhere() {
        let table = icons();
        assert_eq!(table.len(), Glyph::ALL.len());
        for glyph in Glyph::ALL {
            assert!(
                table.iter().any(|(_, _, drawn)| *drawn == glyph),
                "{glyph:?} is never drawn in the gallery"
            );
        }
        let mut names = table
            .iter()
            .map(|(name, _, _)| name.clone())
            .collect::<Vec<_>>();
        names.sort();
        let mut unique = names.clone();
        unique.dedup();
        assert_eq!(names, unique, "two icons share an action");
    }

    #[test]
    fn the_generated_pictures_are_exactly_the_size_they_claim() {
        // The runtime refuses a picture whose bytes and dimensions disagree,
        // and refuses it silently, because a missing picture is a normal
        // condition here. So a mistake in this arithmetic would show up as a
        // page that is simply always empty.
        assert_eq!(wedge().len(), (WEDGE_WIDTH * WEDGE_HEIGHT) as usize);
        assert_eq!(card().len(), (CARD_WIDTH * CARD_HEIGHT) as usize);
        let mut levels = wedge()
            .into_iter()
            .take(WEDGE_WIDTH as usize)
            .collect::<Vec<_>>();
        levels.dedup();
        assert_eq!(levels.len(), 16, "the wedge is not one band per grey");
        assert_eq!(levels.first(), Some(&0));
        assert_eq!(levels.last(), Some(&255));
    }

    #[test]
    fn the_pictures_are_handed_over_once_and_then_drawn_by_handle() {
        let mut gallery = Gallery::default();
        let mut context = Context::default();
        gallery.on_start(&mut context);
        let commands = context.take_commands();
        let given = commands
            .iter()
            .filter(|command| matches!(command, Command::PutPicture { .. }))
            .count();
        assert_eq!(given, 2, "the pictures were not handed over on start");

        gallery.tab = Tab::Input;
        let screen = painted(&gallery);
        let mut context = Context::default();
        gallery.show(&mut context);
        assert!(
            !context
                .take_commands()
                .iter()
                .any(|command| matches!(command, Command::PutPicture { .. })),
            "a repaint put the whole image back on the wire"
        );
        assert!(
            screen
                .nodes
                .iter()
                .any(|node| matches!(node, Node::Band { .. })),
            "the hero drew no band"
        );
    }
}
