#![forbid(unsafe_code)]

//! Plain Rust application API for producing Kobo UI commands.
//!
//! Applications own their state and call [`AppRunner::start`] and
//! [`AppRunner::action`] from their platform event loop.

pub use kobo_protocol::{
    Credential, DenyReason, DeviceRequest, DeviceResult, Frame, Header, Lifecycle, LogLevel,
    Message, SecretHeader, ShellError, ShellEvent, ShellRequest, StoreError, StoreRequest,
    StoreResult, StreamError, Task, TaskError, TaskId, TaskOutcome, MAX_HEADERS, MAX_HEADER_NAME,
    MAX_HEADER_VALUE, MAX_INLINE_PICTURE_BYTES, MAX_PICTURE_BYTES, MAX_PICTURE_CHUNK_BYTES,
    MAX_SHELL_CHUNK, MAX_STORE_KEYS, MAX_STORE_VALUE, MAX_TASK_BYTES, MAX_URL_LEN,
};
pub use kobo_ui::{
    terminal_grid, terminal_grid_for, ActionId, BannerLevel, BarAction, BottomAction, Caret, Cell,
    Chrome, DisplayMetrics, Freeform, Glyph, NavBar, Node, NodeId, Percent, PictureHandle,
    ProseArea, Row, RowLead, Screen, Space, Tile, TilePicture, TileShape, TopBar, MAX_CELLS,
    MAX_CHOICE_OPTIONS, MAX_COLUMNS, MAX_QUOTE_DEPTH, MAX_ROWS, MAX_TERMINAL_COLUMNS,
    MAX_TERMINAL_ROWS,
};
use std::collections::VecDeque;
use std::fmt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// The capability and power model shared with the runtime.
pub use kobo_policy as permissions;

pub use kobo_policy::{Capability, Declared, Grant, Grants, PowerPolicy};

/// Common application and builder types.
pub mod keyboard;
pub mod terminal;

pub mod prelude {
    pub use crate::{
        action_id, ActionId, AppRunner, AppShell, AppStore, Capability, Client, ClientEvent,
        Command, Context, DenyReason, Device, DeviceRequest, DeviceResult, Grant, Grants, KoboApp,
        Lifecycle, Node, NodeId, PowerPolicy, Screen, ScreenBuilder, ShellError, ShellEvent,
        ShellRequest, StoreError, StoreRequest, StoreResult,
    };
}

/// Builds a retained screen with deterministic identifiers.
///
/// Node identifiers are allocated in declaration order. Action identifiers are
/// derived from their string names, so applications can dispatch actions with
/// [`action_id`] without retaining a builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenBuilder {
    id: u32,
    next_node: u32,
    top_bar: Option<TopBar>,
    nodes: Vec<Node>,
    nav_bar: Option<NavBar>,
    bottom_action: Option<BottomAction>,
    page_turns: Option<kobo_ui::PageTurns>,
    actions: Vec<(String, ActionId)>,
}

impl ScreenBuilder {
    #[must_use]
    pub fn new(name: impl AsRef<str>) -> Self {
        Self {
            id: stable_id(name.as_ref()),
            next_node: 1,
            top_bar: None,
            nodes: Vec::new(),
            nav_bar: None,
            bottom_action: None,
            page_turns: None,
            actions: Vec::new(),
        }
    }

    #[must_use]
    pub fn heading(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Heading {
            id,
            text: text.into(),
        });
        self
    }

    #[must_use]
    pub fn text(mut self, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Text {
            id,
            text: text.into(),
        });
        self
    }

    /// A paragraph set in from the left by `depth` levels, with a rule beside
    /// it, for a reply that answers what came before it.
    ///
    /// Depth is clamped to [`MAX_QUOTE_DEPTH`], so a thread that nests forty
    /// deep still reads: the deepest replies share an indent and say how deep
    /// they really are in their own words.
    #[must_use]
    pub fn quote(mut self, depth: u8, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Quote {
            id,
            depth: depth.min(MAX_QUOTE_DEPTH),
            text: text.into(),
        });
        self
    }

    #[must_use]
    pub fn button(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        let id = self.next_id();
        self.nodes.push(Node::Button {
            id,
            action,
            label: label.into(),
        });
        self
    }

    #[must_use]
    pub fn divider(mut self) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Divider { id });
        self
    }

    /// Adds vertical space from the design scale.
    ///
    /// There is deliberately no pixel argument. Authors choose an intent and
    /// the renderer decides what that measures on the panel in front of it.
    #[must_use]
    pub fn spacer(mut self, space: Space) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Spacer { id, space });
        self
    }

    /// Adds a progress bar. Values above a hundred are clamped rather than
    /// rejected, because that is a caller mistake and not a reason to fail.
    #[must_use]
    pub fn progress(mut self, value: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Progress {
            id,
            value: Percent::new(value),
        });
        self
    }

    #[must_use]
    pub fn paged_list<I, S>(mut self, page: u16, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let id = self.next_id();
        self.nodes.push(Node::PagedList {
            id,
            page,
            items: items.into_iter().map(Into::into).collect(),
        });
        self
    }

    #[must_use]
    pub fn action(&self, name: &str) -> Option<ActionId> {
        self.actions
            .iter()
            .find_map(|(known, id)| (known == name).then_some(*id))
    }

    /// Adds the fixed top bar.
    ///
    /// Calling this twice replaces the bar rather than adding a second one. A
    /// screen has at most one, which is a property of the type rather than a
    /// rule the author has to follow.
    #[must_use]
    pub fn top_bar(mut self, title: impl Into<String>) -> Self {
        let id = self.next_id();
        self.top_bar = Some(TopBar::new(id, title));
        self
    }

    /// Adds the single permitted action to the top bar.
    ///
    /// A no-op if there is no top bar, because an action with nowhere to live
    /// is an author mistake that should not silently become a floating button.
    #[must_use]
    pub fn top_bar_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(top_bar) = self.top_bar.take() {
            self.top_bar = Some(TopBar {
                action: Some(BarAction::new(action, label)),
                ..top_bar
            });
        }
        self
    }

    /// Adds the fixed bottom bar.
    ///
    /// Note there is no back destination to add: back belongs to the runtime's
    /// navigation stack, so it appears automatically wherever there is
    /// somewhere to go back to and cannot be omitted by an application.
    /// Turns the sides of the content area into page turns.
    ///
    /// This is how every Kobo has worked since the first one: tap the left of
    /// the page to go back, anywhere else to go on. Actions are named, like
    /// every other action, so the same two intents can later be raised by the
    /// physical page buttons some models have.
    ///
    /// Controls always win. A tap that lands on a button, a row or a keyboard
    /// key is that control's; the zones only ever collect taps that would
    /// otherwise have done nothing.
    #[must_use]
    pub fn page_turns(mut self, previous: impl AsRef<str>, next: impl AsRef<str>) -> Self {
        let previous = self.register(previous.as_ref());
        let next = self.register(next.as_ref());
        self.page_turns = Some(kobo_ui::PageTurns::new(previous, next));
        self
    }

    /// Adds the fixed bar at the bottom of the screen.
    ///
    /// `selected` takes an index or `None`. `None` is for a bar whose entries
    /// are actions rather than places — page back, page forward, the way out —
    /// where marking any of them as current would tell the reader they are
    /// somewhere they are not.
    #[must_use]
    pub fn nav_bar<I, N, L, S>(mut self, selected: S, destinations: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
        S: Into<Option<usize>>,
    {
        let id = self.next_id();
        let destinations = destinations
            .into_iter()
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect::<Vec<_>>();
        self.nav_bar = Some(NavBar::new(id, destinations, selected.into()));
        self.bottom_action = None;
        self
    }

    /// Pins one control to the bottom of the panel, where a bar would go.
    ///
    /// For a screen with a single way off it. Prefer this to a button at the
    /// end of the flow whenever the control must always be reachable: layout
    /// reserves this band before it places any content, so nothing above can
    /// push the control off the panel, and a page that runs long loses its
    /// last line rather than the only way out. A trailing button reserves
    /// nothing, and the launcher shipped with its way back to the Kobo reader
    /// hanging over the bottom edge of the screen because of it.
    ///
    /// Mutually exclusive with [`Self::nav_bar`] — they are the same band.
    #[must_use]
    pub fn bottom_action(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let id = self.next_id();
        let action = BarAction::new(self.register(name.as_ref()), label);
        self.bottom_action = Some(BottomAction::new(id, action));
        self.nav_bar = None;
        self
    }

    /// Adds a grid of tiles. Columns are chosen from the panel's physical
    /// width, so the author never picks a count that is wrong on some device.
    #[must_use]
    pub fn tiles<I, N, L>(mut self, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph)| Tile::new(self.register(name.as_ref()), label, glyph))
            .collect();
        self.nodes.push(Node::TileGrid {
            id,
            tiles,
            shape: TileShape::Square,
        });
        self
    }

    /// Adds a grid of tiles that may each carry a picture.
    ///
    /// Use [`TileShape::Portrait`] for covers and posters: a square cell
    /// letterboxes a book cover into roughly half its own area, which is what
    /// makes a shelf of covers look like a grid of stamps.
    ///
    /// A tile whose picture the runtime does not have falls back to its glyph,
    /// so a shelf is usable while the covers are still arriving.
    #[must_use]
    pub fn picture_tiles<I, N, L>(mut self, shape: TileShape, tiles: I) -> Self
    where
        I: IntoIterator<Item = (N, L, Glyph, Option<TilePicture>)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let tiles = tiles
            .into_iter()
            .map(|(name, label, glyph, picture)| {
                let tile = Tile::new(self.register(name.as_ref()), label, glyph);
                match picture {
                    Some(picture) => tile.with_picture(picture),
                    None => tile,
                }
            })
            .collect();
        self.nodes.push(Node::TileGrid { id, tiles, shape });
        self
    }

    /// Shows one picture, as large as the width and `max_height_mm` allow.
    ///
    /// The height is a physical measurement rather than a pixel count so that
    /// the same screen gives a picture the same share of the panel on a Clara
    /// and on an Elipsa.
    #[must_use]
    pub fn picture(mut self, picture: TilePicture, max_height_mm: u16) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Picture {
            id,
            handle: picture.handle,
            source: picture.source,
            max_height_tenths_mm: max_height_mm.saturating_mul(10),
        });
        self
    }

    /// Lists entries that each need a sentence of explanation.
    ///
    /// Prefer this over [`Self::tiles`] whenever a one-word label would not be
    /// enough. A tile is square and spends most of its area on nothing, so a
    /// screen of tiles holds very few entries; a row holds a title, a summary
    /// and a glyph in a single finger-height band.
    #[must_use]
    pub fn rows<I, N, T, S, L>(mut self, rows: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, L)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
        L: Into<RowLead>,
    {
        let id = self.next_id();
        let rows = rows
            .into_iter()
            .take(MAX_ROWS)
            .map(|(name, title, summary, lead)| {
                Row::new(self.register(name.as_ref()), title, summary, lead)
            })
            .collect();
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A list of things to be done, some of which are.
    ///
    /// The same rows, with the state carried rather than drawn: an application
    /// says whether each entry is finished and the renderer decides what
    /// finished looks like. That is why there is no way to ask for a line
    /// through a piece of text anywhere else in this SDK.
    ///
    /// Tapping a row is what completes it, and only the row that changed is
    /// repainted, so ticking something off costs one fast partial refresh
    /// rather than a whole screen.
    #[must_use]
    pub fn checklist<I, N, T, S>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = (N, T, S, bool)>,
        N: AsRef<str>,
        T: Into<String>,
        S: Into<String>,
    {
        let id = self.next_id();
        let rows = items
            .into_iter()
            .take(MAX_ROWS)
            .map(|(name, title, summary, done)| {
                let glyph = if done { Glyph::Check } else { Glyph::Circle };
                Row::new(self.register(name.as_ref()), title, summary, glyph).done(done)
            })
            .collect();
        self.nodes.push(Node::Rows { id, rows });
        self
    }

    /// A grid of characters, for output that was written to be read in columns.
    ///
    /// Everything else in this builder takes meaning and lets the runtime
    /// decide on appearance. This takes rows that are already positioned,
    /// because in a character grid the position *is* the meaning: a table, a
    /// diff or a shell prompt stops saying what it said the moment something
    /// re-wraps it.
    ///
    /// The grid is not negotiable from here. Ask [`kobo_ui::terminal_grid_for`]
    /// what size the rows should be before filling them, so that whatever is
    /// producing the text is told the same width the panel will show.
    #[must_use]
    pub fn terminal<I, R>(mut self, rows: I, cursor: Option<Caret>) -> Self
    where
        I: IntoIterator<Item = R>,
        R: Into<String>,
    {
        let id = self.next_id();
        let rows = rows
            .into_iter()
            .take(MAX_TERMINAL_ROWS)
            .map(Into::into)
            .collect();
        self.nodes.push(Node::Terminal { id, rows, cursor });
        self
    }
    ///
    /// The general one: the caller picks the columns, so a board, a keypad and
    /// an on-screen keyboard are all this, rather than three primitives that
    /// each have to be added to the layout engine, the renderer, the hit test
    /// and the wire format before anybody can use them.
    ///
    /// `square` gives cells as tall as they are wide, which is what makes a
    /// board look like a board. Without it a cell is one touch target high,
    /// which is what a keyboard wants.
    #[must_use]
    pub fn grid<I, N, L>(mut self, columns: u8, square: bool, cells: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let cells = cells
            .into_iter()
            .take(MAX_CELLS)
            .map(|(name, label)| Cell::new(self.register(name.as_ref()), label))
            .collect();
        self.nodes.push(Node::Grid {
            id,
            columns: columns.clamp(1, MAX_COLUMNS),
            square,
            cells,
        });
        self
    }

    /// Asks a question by offering answers.
    ///
    /// Prefer this over a text field. Typing on this device means summoning a
    /// keyboard onto a slow panel and hunting for keys, and it is markedly
    /// worse than tapping for anything that can be enumerated.
    #[must_use]
    pub fn choose<I, N, L>(mut self, prompt: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (N, L)>,
        N: AsRef<str>,
        L: Into<String>,
    {
        let id = self.next_id();
        let options = options
            .into_iter()
            .take(MAX_CHOICE_OPTIONS)
            .map(|(name, label)| BarAction::new(self.register(name.as_ref()), label))
            .collect();
        self.nodes.push(Node::Choice {
            id,
            prompt: prompt.into(),
            options,
            selected: None,
            freeform: None,
        });
        self
    }

    /// Adds the free-text escape hatch to the choice just declared.
    ///
    /// Deliberately a second call rather than a parameter, so that offering
    /// typing is a decision an author makes on purpose. The keyboard is only
    /// raised if the reader actually taps this row.
    #[must_use]
    pub fn or_type(mut self, name: impl AsRef<str>, placeholder: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Choice { freeform, .. }) = self.nodes.last_mut() {
            *freeform = Some(Freeform::new(action, placeholder));
        }
        self
    }

    /// Marks which option of the choice just declared is already the answer.
    ///
    /// State rather than decoration: the renderer draws the mark from the icon
    /// atlas, so an application never has to put a tick character in a label
    /// and never gets a missing-glyph box on a device whose face lacks it. An
    /// index naming no option leaves every row unmarked.
    #[must_use]
    pub fn chosen(mut self, index: usize) -> Self {
        if let Some(Node::Choice {
            options, selected, ..
        }) = self.nodes.last_mut()
        {
            *selected = u8::try_from(index)
                .ok()
                .filter(|index| usize::from(*index) < options.len());
        }
        self
    }

    /// Adds an attention strip.
    ///
    /// This is what to reach for instead of flashing the frontlight, which is a
    /// photosensitivity hazard and the largest power draw on the device.
    #[must_use]
    pub fn banner(mut self, level: BannerLevel, text: impl Into<String>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Banner {
            id,
            level,
            text: text.into(),
        });
        self
    }

    /// Adds placeholder lines occupying the space real content will fill.
    ///
    /// Paint the real screen with these immediately and patch them as data
    /// arrives, rather than showing a splash. The panel is already displaying
    /// something at zero power, so there is no blank frame to cover.
    #[must_use]
    pub fn skeleton(mut self, lines: u8) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Skeleton { id, lines });
        self
    }

    /// States that work is in flight, for example a network request.
    ///
    /// The replacement for a spinner. Pass `None` for progress unless a real
    /// denominator is known; a bar that invents its own position is worse than
    /// no bar. Progress is snapped to coarse steps before it is drawn.
    #[must_use]
    pub fn activity(mut self, label: impl Into<String>, progress: Option<u8>) -> Self {
        let id = self.next_id();
        self.nodes.push(Node::Activity {
            id,
            label: label.into(),
            progress: progress.map(Percent::new),
            cancel: None,
        });
        self
    }

    /// Lets the reader abandon the activity just declared.
    #[must_use]
    pub fn cancellable(mut self, name: impl AsRef<str>, label: impl Into<String>) -> Self {
        let action = self.register(name.as_ref());
        if let Some(Node::Activity { cancel, .. }) = self.nodes.last_mut() {
            *cancel = Some(BarAction::new(action, label));
        }
        self
    }

    #[must_use]
    pub fn build(self) -> Screen {
        Screen {
            id: self.id,
            top_bar: self.top_bar,
            nodes: self.nodes,
            nav_bar: self.nav_bar,
            bottom_action: self.bottom_action,
            page_turns: self.page_turns,
        }
    }

    fn register(&mut self, name: &str) -> ActionId {
        let action = action_id(name);
        if !self.actions.iter().any(|(known, _)| known == name) {
            self.actions.push((name.to_owned(), action));
        }
        action
    }

    fn next_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node);
        self.next_node = self.next_node.saturating_add(1);
        id
    }
}

/// Deterministically maps an action name to a non-zero wire action ID.
#[must_use]
pub fn action_id(name: &str) -> ActionId {
    ActionId(stable_id(name))
}

fn stable_id(value: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in value.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    if hash == 0 {
        1
    } else {
        hash
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    SetScreen(Screen),
    Log {
        level: LogLevel,
        message: String,
    },
    Device(DeviceRequest),
    Spawn {
        task: TaskId,
        work: Task,
    },
    Cancel(TaskId),
    /// Read or write the application's own small state.
    Store(StoreRequest),
    /// Drive a terminal the runtime owns.
    Shell(ShellRequest),
    Exit,
    /// Hand the panel to another application by name.
    Launch(String),
    /// Give the runtime a picture to hold.
    PutPicture {
        handle: PictureHandle,
        width: u32,
        height: u32,
        grey: Vec<u8>,
    },
    /// Release a picture the runtime is holding.
    DropPicture(PictureHandle),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    commands: Vec<Command>,
    next_task: u32,
    in_flight: usize,
    metrics: DisplayMetrics,
}

impl Context {
    /// The panel this application is drawing to.
    ///
    /// An application never positions anything, so this is not for layout. It
    /// is for the few decisions only the application can make, above all where
    /// to break a long document into pages, which depends on how much text the
    /// panel physically holds.
    #[must_use]
    pub const fn metrics(&self) -> DisplayMetrics {
        self.metrics
    }

    /// Breaks prose into pages that fit this panel.
    ///
    /// Each page is a list of paragraphs to emit as separate text nodes, in
    /// order. Measuring is done with the runtime's own wrapping and line
    /// height, so a page that fits here is a page that will be drawn whole:
    /// layout stops at the bottom of the content area and silently drops the
    /// rest, which for a book means losing its last paragraph with nothing on
    /// the panel to say so.
    ///
    /// `nav_bar` says whether the reading screen pins its page controls to the
    /// bottom, which it should: controls at the end of the flow are the first
    /// thing a long page pushes off the panel.
    #[must_use]
    pub fn paginate(&self, text: &str, nav_bar: bool) -> Vec<Vec<String>> {
        kobo_ui::paginate(text, self.metrics.prose_area(true, nav_bar))
    }

    /// Breaks threaded prose into pages that fit this panel, keeping the
    /// depth of every paragraph.
    ///
    /// Indentation has to be measured, not applied afterwards: an indented
    /// paragraph is narrower, so it wraps to more lines and eats more of the
    /// page. Feed the result straight to [`ScreenBuilder::quote`].
    #[must_use]
    pub fn paginate_quoted(
        &self,
        paragraphs: &[(u8, &str)],
        nav_bar: bool,
    ) -> Vec<Vec<(u8, String)>> {
        kobo_ui::paginate_quoted(
            paragraphs,
            &self.metrics,
            self.metrics.prose_area(true, nav_bar),
        )
    }

    /// `text` cut to the single line a list row can show, ellipsised if it
    /// did not fit.
    ///
    /// A title that wraps makes its row taller than the one above it, and a
    /// list whose rows all differ in height is one the eye has to re-measure
    /// on every line. Measured against the same width the layout engine gives
    /// a row's words, so what fits here is what fits there.
    #[must_use]
    pub fn one_line_row(&self, text: &str, nav_bar: bool) -> String {
        self.clamped_row(text, 1, nav_bar)
    }

    /// `text` cut to at most `lines` lines of a list row.
    ///
    /// Two is the useful setting for anything written elsewhere — a headline,
    /// a subject line, a filename — because one line ellipsises most of them
    /// mid-sentence. Rows then differ in height, which [`Self::paginate_rows`]
    /// already accounts for.
    #[must_use]
    pub fn clamped_row(&self, text: &str, lines: usize, nav_bar: bool) -> String {
        let area = self.metrics.prose_area(true, nav_bar);
        kobo_ui::clamp_lines(
            text,
            kobo_ui::row_text_width(&self.metrics, area),
            kobo_ui::FontSize::Body,
            lines,
        )
    }

    /// Breaks a list of rows into pages that fit this panel.
    ///
    /// Returns the row indices belonging to each page. Nothing in this UI
    /// scrolls: a panel that takes most of a second to repaint cannot follow a
    /// finger, so a list longer than the screen is turned like a page rather
    /// than dragged. Without this an application has no way to know where the
    /// fold is, and the layout engine simply stops drawing at the bottom.
    #[must_use]
    pub fn paginate_rows(&self, rows: &[(&str, &str)], nav_bar: bool) -> Vec<Vec<usize>> {
        kobo_ui::paginate_rows(rows, &self.metrics, self.metrics.prose_area(true, nav_bar))
    }

    /// Breaks a grid of tiles into pages that fit this panel.
    ///
    /// Returns the tile indices belonging to each page. The count of tiles a
    /// panel holds is a measurement, not a constant: a Clara fits two columns
    /// and a Sage three, so an application that picked a number would silently
    /// lose its last entries on every panel but the one it was written on.
    #[must_use]
    pub fn paginate_tiles(&self, count: usize, shape: TileShape, nav_bar: bool) -> Vec<Vec<usize>> {
        kobo_ui::paginate_tiles(
            count,
            &self.metrics,
            shape,
            self.metrics.prose_area(true, nav_bar),
        )
    }

    /// Asks the runtime to hand the panel to another application.
    ///
    /// The name is looked up in the catalogue the runtime maintains; an
    /// application cannot name a path, so it cannot start anything that was not
    /// installed. Whether this is permitted at all is a capability, so an
    /// ordinary application asking for it is simply refused.
    ///
    /// This application stops when the other one starts and is started again
    /// when it finishes, so any state that must survive has to be saved first.
    pub fn launch(&mut self, name: impl Into<String>) {
        self.commands.push(Command::Launch(name.into()));
    }

    /// # Panics
    ///
    /// In debug builds only, on a screen the wire would refuse or one carrying
    /// a character the installed face cannot draw. Both are defects that are
    /// silent on the panel and obvious here.
    pub fn set_screen(&mut self, screen: Screen) {
        // A screen the wire refuses is not a rendering problem, it is a dead
        // connection: the runtime's reader stops at the malformed frame and
        // the application then waits forever for events from a socket nobody
        // is reading. On the panel that looks like every tap being ignored.
        // Checked in debug builds only, so an application's own tests fail on
        // the screen that built it rather than a device session doing nothing.
        debug_assert!(
            kobo_protocol::encode(&kobo_protocol::Frame {
                request_id: 1,
                message: kobo_protocol::Message::SetScreen(screen.clone()),
            })
            .is_ok(),
            "this screen cannot be sent to the runtime: {:?}",
            kobo_protocol::encode(&kobo_protocol::Frame {
                request_id: 1,
                message: kobo_protocol::Message::SetScreen(screen.clone()),
            })
            .err()
        );
        // The same idea one layer up: a character the installed face has no
        // glyph for is drawn as an empty box, which reads on the panel as a
        // broken renderer rather than as a missing character. Checked against
        // what will actually be drawn, so an application that marks its own
        // state with a symbol fails its own tests instead of shipping.
        #[cfg(debug_assertions)]
        {
            let layout = screen.layout_for(&self.metrics);
            for node in &layout.nodes {
                for line in &node.text_lines {
                    assert!(
                        kobo_ui::undrawable_in(line, kobo_ui::Face::Text).is_none(),
                        "this screen carries {:?}, which the installed face cannot draw: {line:?}",
                        kobo_ui::undrawable_in(line, kobo_ui::Face::Text).expect("just found one")
                    );
                }
            }
        }
        self.commands.push(Command::SetScreen(screen));
    }

    pub fn log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.commands.push(Command::Log {
            level,
            message: message.into(),
        });
    }

    pub fn exit(&mut self) {
        self.commands.push(Command::Exit);
    }

    /// Hands a decoded picture to the runtime and returns the reference to put
    /// on a screen.
    ///
    /// Send a picture once and refer to it afterwards. Screens are re-sent
    /// whole on every change, so a picture that travelled with the screen would
    /// travel again on every tap.
    ///
    /// Fit the picture to the space it will occupy before calling this. The
    /// renderer will shrink an oversized one, but sending pixels that are
    /// immediately averaged away costs the wire, the runtime's cache and the
    /// battery for nothing.
    ///
    /// Returns `None` when the picture is empty, mis-sized, or larger than the
    /// bounded per-picture budget. Large pictures are chunked transparently by
    /// the socket client and become visible only after the final chunk arrives.
    pub fn put_picture(
        &mut self,
        handle: PictureHandle,
        width: u32,
        height: u32,
        grey: Vec<u8>,
    ) -> Option<TilePicture> {
        let expected = usize::try_from(width)
            .ok()
            .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))?;
        if expected == 0 || expected != grey.len() || expected > MAX_PICTURE_BYTES {
            return None;
        }
        self.commands.push(Command::PutPicture {
            handle,
            width,
            height,
            grey,
        });
        Some(TilePicture::new(handle, width, height))
    }

    /// Releases a picture. Every picture is released anyway when the
    /// application exits, so this is for one that outlives its usefulness.
    pub fn drop_picture(&mut self, handle: PictureHandle) {
        self.commands.push(Command::DropPicture(handle));
    }

    /// Hands work to the runtime so the event loop keeps running.
    ///
    /// Returns `None` once too many tasks are already in flight, rather than
    /// queueing without limit. An application that cannot start more work
    /// should say so on screen; silently accumulating requests is how a device
    /// ends up holding the radio open with nothing to show for it.
    ///
    /// There is no blocking equivalent anywhere in this API. A blocking fetch
    /// would freeze the screen and the back control along with it.
    pub fn spawn(&mut self, work: Task) -> Option<TaskId> {
        if self.in_flight >= MAX_TASKS_IN_FLIGHT {
            return None;
        }
        self.next_task = self.next_task.saturating_add(1);
        let task = TaskId(self.next_task);
        self.in_flight += 1;
        self.commands.push(Command::Spawn { task, work });
        Some(task)
    }

    /// Abandons a task. The application still receives exactly one
    /// [`KoboApp::on_task`] for it, reporting [`TaskOutcome::Cancelled`].
    pub fn cancel(&mut self, task: TaskId) {
        self.commands.push(Command::Cancel(task));
    }

    /// Records that a task has reported back, freeing one slot.
    pub(crate) fn settle(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    #[must_use]
    pub const fn tasks_in_flight(&self) -> usize {
        self.in_flight
    }

    /// Hardware operations, expressed as intent rather than device access.
    ///
    /// Every call queues one request. The runtime answers each one exactly
    /// once through [`KoboApp::on_device_result`], including when it refuses.
    pub fn device(&mut self) -> Device<'_> {
        Device { context: self }
    }

    /// The application's own small state, which survives being closed.
    ///
    /// Every application has one and none has to ask for it, in the same way a
    /// phone does not ask permission to remember which tab you were on. It is
    /// keyed, never pathed, so there is no syntax that can name somewhere else.
    pub fn store(&mut self) -> AppStore<'_> {
        AppStore { context: self }
    }

    /// The terminal this application may run a program on.
    ///
    /// Nothing happens until [`AppShell::open`] is called, and nothing happens
    /// at all unless the application declared the `shell` capability. Like the
    /// network and the panel, the dangerous object stays behind the runtime:
    /// an application says what to type, never what to execute.
    pub fn shell(&mut self) -> AppShell<'_> {
        AppShell { context: self }
    }

    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    #[must_use]
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }
}

/// An application's own small state.
///
/// Sized for what an application needs in order to open where it closed: a
/// reading position, a list, a preference. Deliberately far too small for
/// content, which is what a task and a real file are for.
#[derive(Debug)]
pub struct AppStore<'a> {
    context: &'a mut Context,
}

impl AppStore<'_> {
    /// Writes a value, replacing whatever was under that key.
    ///
    /// The write is atomic: a reader sees the previous value or the new one and
    /// never a splice of the two, which matters on a device that loses power
    /// without warning.
    pub fn save(&mut self, key: impl Into<String>, value: impl Into<Vec<u8>>) {
        self.request(StoreRequest::Save {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Reads a value back. A key that was never written is not an error.
    pub fn load(&mut self, key: impl Into<String>) {
        self.request(StoreRequest::Load { key: key.into() });
    }

    /// Removes a key. Removing one that is not there is a success.
    pub fn forget(&mut self, key: impl Into<String>) {
        self.request(StoreRequest::Forget { key: key.into() });
    }

    /// Lists the keys this application has written.
    pub fn list(&mut self) {
        self.request(StoreRequest::List);
    }

    fn request(&mut self, request: StoreRequest) {
        self.context.commands.push(Command::Store(request));
    }
}

/// The application's terminal.
///
/// Every method here is a request, answered through
/// [`KoboApp::on_shell_event`]. There is no return value to check because
/// there is nothing to check yet: the runtime may refuse, the program may fail
/// to start, and either way the answer arrives as an event like any other.
#[derive(Debug)]
pub struct AppShell<'a> {
    context: &'a mut Context,
}

impl AppShell<'_> {
    /// Starts a program on a terminal of exactly this grid.
    ///
    /// Ask [`terminal_grid_for`] what the grid is rather than choosing one.
    /// A program told a width the panel does not have wraps its lines in a
    /// different place from where the reader sees them wrap, which makes every
    /// full-screen program unusable.
    pub fn open(&mut self, columns: u16, rows: u16) {
        self.request(ShellRequest::Open { columns, rows });
    }

    /// Sends keystrokes, already encoded as the bytes a terminal expects.
    pub fn input(&mut self, bytes: impl Into<Vec<u8>>) {
        self.request(ShellRequest::Input(bytes.into()));
    }

    /// Tells the program the grid changed.
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.request(ShellRequest::Resize { columns, rows });
    }

    /// Ends the program.
    pub fn close(&mut self) {
        self.request(ShellRequest::Close);
    }

    fn request(&mut self, request: ShellRequest) {
        self.context.commands.push(Command::Shell(request));
    }
}

/// Capability-gated hardware operations.
///
/// An application never opens a device node, chooses a waveform, writes a sysfs
/// file, or talks to a radio. It states what it wants; the runtime decides.
/// Durations are advisory upper bounds and are clamped by system policy, so a
/// grant may be shorter than the request.
#[derive(Debug)]
pub struct Device<'a> {
    context: &'a mut Context,
}

impl Device<'_> {
    /// Asks for the battery percentage and charging state.
    pub fn read_battery(&mut self) {
        self.request(DeviceRequest::ReadBattery);
    }

    /// Asks to keep Wi-Fi associated for at most `duration`.
    ///
    /// Use this for a dashboard that must stay reachable. It is the most
    /// expensive thing an application can ask for, so expect a shorter grant
    /// than requested and a refusal on a low battery.
    pub fn hold_wifi(&mut self, duration: Duration) {
        self.request(DeviceRequest::HoldWifi {
            seconds: whole_seconds(duration),
        });
    }

    /// Releases a Wi-Fi hold before it expires.
    pub fn release_wifi(&mut self) {
        self.request(DeviceRequest::ReleaseWifi);
    }

    /// Asks to stay out of suspend for at most `duration`.
    pub fn keep_awake(&mut self, duration: Duration) {
        self.request(DeviceRequest::KeepAwake {
            seconds: whole_seconds(duration),
        });
    }

    /// Releases a wake hold before it expires.
    pub fn allow_sleep(&mut self) {
        self.request(DeviceRequest::AllowSleep);
    }

    /// Asks to be woken after `delay` to refresh content.
    ///
    /// The runtime coalesces wakes across applications and enforces a minimum
    /// interval, so the granted delay is often longer than requested.
    pub fn schedule_wake(&mut self, delay: Duration) {
        self.request(DeviceRequest::ScheduleWake {
            seconds: whole_seconds(delay),
        });
    }

    /// Cancels a pending scheduled wake.
    pub fn cancel_wake(&mut self) {
        self.request(DeviceRequest::CancelWake);
    }

    /// Sets the front light, as a percentage of its range.
    pub fn set_frontlight(&mut self, percent: u8) {
        self.request(DeviceRequest::SetFrontlight {
            percent: percent.min(100),
        });
    }

    /// Asks for the current front light percentage.
    pub fn read_frontlight(&mut self) {
        self.request(DeviceRequest::ReadFrontlight);
    }

    fn request(&mut self, request: DeviceRequest) {
        self.context.commands.push(Command::Device(request));
    }
}

/// Converts a duration to whole seconds without overflowing or rounding to zero.
fn whole_seconds(duration: Duration) -> u32 {
    let seconds = duration.as_secs();
    if seconds == 0 && duration.subsec_nanos() > 0 {
        return 1;
    }
    u32::try_from(seconds).unwrap_or(u32::MAX)
}

/// Application lifecycle driven by the embedding platform.
pub trait KoboApp {
    fn on_start(&mut self, context: &mut Context);
    fn on_action(&mut self, context: &mut Context, action: ActionId);

    fn on_resume(&mut self, _context: &mut Context) {}

    fn on_suspend(&mut self, _context: &mut Context) {}

    fn on_scheduled_wake(&mut self, _context: &mut Context) {}

    fn on_exit(&mut self, _context: &mut Context) {}

    /// Receives the runtime's answer to exactly one earlier device request.
    ///
    /// Every request produces exactly one call, so an application never has to
    /// guess whether something was honoured.
    fn on_device_result(
        &mut self,
        _context: &mut Context,
        _request: DeviceRequest,
        _result: DeviceResult,
    ) {
    }

    /// Receives the outcome of exactly one earlier [`Context::spawn`].
    ///
    /// Like device results, a task always reports back, including when it fails
    /// or is cancelled, so an application never has to time out its own work.
    fn on_task(&mut self, _context: &mut Context, _task: TaskId, _outcome: TaskOutcome) {}

    /// The reader left this application for another one.
    ///
    /// Nothing is stopped: work in flight keeps running and answers keep
    /// arriving. What changes is that nothing drawn from here will be seen
    /// until [`KoboApp::on_foreground`], so this is the moment to write
    /// anything that would be missed if the device never came back.
    fn on_background(&mut self, _context: &mut Context) {}

    /// The reader came back.
    ///
    /// The panel still holds whatever was last drawn from this application, so
    /// there is no blank to cover, but anything that changed while it was away
    /// has to be drawn now.
    fn on_foreground(&mut self, _context: &mut Context) {}

    /// Receives the runtime's answer to exactly one earlier store request.
    ///
    /// Like device results, every request reports back, so an application never
    /// has to guess whether its state was written.
    fn on_store(&mut self, _context: &mut Context, _result: StoreResult) {}

    /// Receives everything a terminal has to say: that it opened, what the
    /// program printed, that it finished, or that the request was refused.
    fn on_shell_event(&mut self, _context: &mut Context, _event: ShellEvent) {}
}

/// The longest a lifecycle callback may run before the runtime intervenes.
///
/// This exists because an application that blocks in a callback would otherwise
/// hold the only thread that can repaint the screen, read a touch, or honour a
/// request to leave. That is a safety problem rather than a style one: the
/// reader's only remaining option would be a hard power cycle.
pub const CALLBACK_DEADLINE: Duration = Duration::from_millis(250);

/// The ceiling on tasks in flight for one application at once.
///
/// Each task is a real connection or file handle, and an unbounded queue is an
/// unbounded amount of radio time.
pub const MAX_TASKS_IN_FLIGHT: usize = 4;

#[derive(Debug)]
pub struct AppRunner<A> {
    app: A,
    metrics: DisplayMetrics,
    started: bool,
    pending: VecDeque<DeviceRequest>,
    /// Task counters live here rather than in `Context`, because a fresh
    /// context is built for every callback. Left in the context they would
    /// restart at one on each dispatch, so the second callback to spawn work
    /// would hand out an identifier already in use and the two tasks would
    /// report back as one.
    next_task: u32,
    in_flight: usize,
    settled: bool,
}

impl<A: KoboApp> AppRunner<A> {
    #[must_use]
    pub fn new(app: A) -> Self {
        // The same typeface the runtime lays out with, installed here as well
        // as on the socket path, because an application's own tests are where
        // wrapping, pagination and one-line labels are actually asserted. Left
        // to the built-in bitmap they would be asserted against a fixed-width
        // uppercase fallback nothing ever draws with.
        #[cfg(feature = "text")]
        let _ = kobo_text::install(DisplayMetrics::default());
        Self {
            app,
            metrics: DisplayMetrics::default(),
            started: false,
            pending: VecDeque::new(),
            next_task: 0,
            in_flight: 0,
            settled: false,
        }
    }

    /// Runs an application against a specific panel.
    ///
    /// [`AppRunner::new`] assumes the only panel with hardware support, which
    /// is right for a test and wrong for the device: the runtime states which
    /// panel it owns during the handshake.
    #[must_use]
    pub fn with_metrics(app: A, metrics: DisplayMetrics) -> Self {
        #[cfg(feature = "text")]
        let _ = kobo_text::install(metrics);
        Self {
            metrics,
            ..Self::new(app)
        }
    }

    #[must_use]
    pub const fn app(&self) -> &A {
        &self.app
    }

    pub fn app_mut(&mut self) -> &mut A {
        &mut self.app
    }

    /// A context measuring against the same panel the application ran on.
    ///
    /// For tests that need to ask what fits: a test computing its own widths
    /// is testing its own arithmetic, and the whole point of `one_line_row`
    /// and `clamped_row` is that there is exactly one measure.
    #[must_use]
    pub fn context(&self) -> Context {
        Context {
            commands: Vec::new(),
            next_task: self.next_task,
            in_flight: self.in_flight,
            metrics: self.metrics,
        }
    }

    pub fn start(&mut self) -> Vec<Command> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        self.dispatch(KoboApp::on_start)
    }

    pub fn action(&mut self, action: ActionId) -> Vec<Command> {
        self.dispatch(|app, context| app.on_action(context, action))
    }

    pub fn resume(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_resume)
    }

    pub fn suspend(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_suspend)
    }

    pub fn scheduled_wake(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_scheduled_wake)
    }

    pub fn exit(&mut self) -> Vec<Command> {
        self.dispatch(KoboApp::on_exit)
    }

    /// Delivers one device answer, matched to the request that produced it.
    ///
    /// Answers arrive in request order on a single ordered stream. An answer
    /// with nothing outstanding is ignored rather than mismatched.
    pub fn device_result(&mut self, result: DeviceResult) -> Vec<Command> {
        let Some(request) = self.pending.pop_front() else {
            return Vec::new();
        };
        self.dispatch(|app, context| app.on_device_result(context, request, result))
    }

    /// Delivers the outcome of one task.
    ///
    /// Reporting back frees the slot the task occupied, so an application that
    /// keeps starting work can keep starting it, while one that never hears
    /// back stays capped.
    pub fn task_outcome(&mut self, task: TaskId, outcome: TaskOutcome) -> Vec<Command> {
        self.settled = true;
        self.dispatch(|app, context| app.on_task(context, task, outcome))
    }

    /// Tells the application it gained or lost the panel.
    pub fn lifecycle(&mut self, state: Lifecycle) -> Vec<Command> {
        match state {
            Lifecycle::Foreground => self.dispatch(KoboApp::on_foreground),
            Lifecycle::Background => self.dispatch(KoboApp::on_background),
        }
    }

    /// Delivers one store answer.
    pub fn store_result(&mut self, result: StoreResult) -> Vec<Command> {
        self.dispatch(|app, context| app.on_store(context, result))
    }

    /// Delivers one terminal event.
    pub fn shell_event(&mut self, event: ShellEvent) -> Vec<Command> {
        self.dispatch(|app, context| app.on_shell_event(context, event))
    }

    /// The device requests still awaiting an answer, oldest first.
    #[must_use]
    pub fn outstanding_requests(&self) -> usize {
        self.pending.len()
    }

    #[must_use]
    pub const fn tasks_in_flight(&self) -> usize {
        self.in_flight
    }

    fn dispatch(&mut self, callback: impl FnOnce(&mut A, &mut Context)) -> Vec<Command> {
        let mut context = Context {
            commands: Vec::new(),
            next_task: self.next_task,
            in_flight: self.in_flight,
            metrics: self.metrics,
        };
        if std::mem::take(&mut self.settled) {
            context.settle();
        }
        let started = std::time::Instant::now();
        callback(&mut self.app, &mut context);
        let elapsed = started.elapsed();
        self.next_task = context.next_task;
        self.in_flight = context.in_flight;
        let mut commands = context.take_commands();
        for command in &commands {
            if let Command::Device(request) = command {
                self.pending.push_back(*request);
            }
        }
        // An application that blocks here has already held the only thread that
        // can repaint the screen or read a touch. The overrun cannot be
        // prevented from inside the callback, so it is reported instead, and
        // the host runtime is expected to act on a repeat offender.
        if elapsed > CALLBACK_DEADLINE {
            commands.insert(
                0,
                Command::Log {
                    level: LogLevel::Warn,
                    message: format!(
                        "a lifecycle callback ran for {} ms, over the {} ms deadline; move this work to Context::spawn",
                        elapsed.as_millis(),
                        CALLBACK_DEADLINE.as_millis()
                    ),
                },
            );
        }
        commands
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    Action(ActionId),
    Device(DeviceResult),
    Task { task: TaskId, outcome: TaskOutcome },
    Store(StoreResult),
    Lifecycle(Lifecycle),
    Shell(ShellEvent),
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientError {
    Stream(StreamError),
    UnexpectedMessage,
    /// The runtime did not say where to connect, which means this binary was
    /// started by something other than the runtime.
    MissingSocket,
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stream(error) => write!(formatter, "{error}"),
            Self::UnexpectedMessage => formatter.write_str("unexpected daemon message"),
            Self::MissingSocket => formatter.write_str(
                "KOBO_SOCKET is not set; a Kobo application is started by the runtime, not directly",
            ),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<StreamError> for ClientError {
    fn from(error: StreamError) -> Self {
        Self::Stream(error)
    }
}

/// Synchronous client for one managed Kobo application.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    next_request: u32,
    metrics: DisplayMetrics,
}

impl Client {
    /// Connects to `kobod` and completes the protocol handshake.
    ///
    /// # Errors
    ///
    /// Returns a stream error when the socket cannot be opened or the handshake
    /// cannot be exchanged, and `UnexpectedMessage` for a non-welcome response.
    pub fn connect(path: impl AsRef<Path>, app_name: &str) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(path).map_err(StreamError::from)?;
        Self::from_stream(stream, app_name)
    }

    /// Completes a handshake over an already connected private stream.
    ///
    /// # Errors
    ///
    /// Returns a protocol or stream error, or `UnexpectedMessage` when the peer
    /// does not identify itself as `kobod`.
    pub fn from_stream(mut stream: UnixStream, app_name: &str) -> Result<Self, ClientError> {
        kobo_protocol::write_to(
            &mut stream,
            &Frame {
                request_id: 1,
                message: Message::Hello {
                    name: app_name.to_owned(),
                },
            },
        )?;
        let response = kobo_protocol::read_from(&mut stream)?;
        let Message::Welcome {
            width,
            height,
            pixels_per_inch,
            text_scale,
        } = response.message
        else {
            return Err(ClientError::UnexpectedMessage);
        };
        Ok(Self {
            stream,
            next_request: 2,
            metrics: DisplayMetrics {
                width: i32::from(width),
                height: i32::from(height),
                pixels_per_inch: i32::from(pixels_per_inch),
                text_scale,
            },
        })
    }

    /// The panel this application is running on.
    ///
    /// Learned from the runtime rather than assumed, so an application that
    /// measures text measures it for the panel it is actually drawing to.
    #[must_use]
    pub const fn metrics(&self) -> DisplayMetrics {
        self.metrics
    }

    /// Sends all commands produced by an application callback.
    ///
    /// # Errors
    ///
    /// Returns a stream or protocol error if any command cannot be delivered.
    pub fn send_commands(
        &mut self,
        commands: impl IntoIterator<Item = Command>,
    ) -> Result<(), ClientError> {
        for command in commands {
            let command = match command {
                Command::PutPicture {
                    handle,
                    width,
                    height,
                    grey,
                } => {
                    if grey.len() <= MAX_INLINE_PICTURE_BYTES {
                        self.send(Message::PutPicture {
                            handle,
                            width,
                            height,
                            grey,
                        })?;
                    } else {
                        self.send(Message::BeginPicture {
                            handle,
                            width,
                            height,
                        })?;
                        for (index, chunk) in grey.chunks(MAX_PICTURE_CHUNK_BYTES).enumerate() {
                            let offset = index
                                .checked_mul(MAX_PICTURE_CHUNK_BYTES)
                                .and_then(|offset| u32::try_from(offset).ok())
                                .ok_or(ClientError::Stream(StreamError::Protocol(
                                    kobo_protocol::ProtocolError::FrameTooLarge,
                                )))?;
                            self.send(Message::PictureChunk {
                                handle,
                                offset,
                                grey: chunk.to_vec(),
                            })?;
                        }
                        self.send(Message::CommitPicture { handle })?;
                    }
                    continue;
                }
                other => other,
            };
            let message = match command {
                Command::SetScreen(screen) => Message::SetScreen(screen),
                Command::Log { level, message } => Message::Log { level, message },
                Command::Device(request) => Message::DeviceRequest(request),
                Command::Spawn { task, work } => Message::Spawn { task, work },
                Command::Cancel(task) => Message::Cancel { task },
                Command::Store(request) => Message::StoreRequest(request),
                Command::Shell(request) => Message::ShellRequest(request),
                Command::Exit => Message::Exit,
                Command::Launch(name) => Message::Launch { name },
                Command::PutPicture { .. } => unreachable!("handled above"),
                Command::DropPicture(handle) => Message::DropPicture { handle },
            };
            self.send(message)?;
        }
        Ok(())
    }

    /// Waits for the next user action or daemon exit request.
    ///
    /// # Errors
    ///
    /// Returns a stream/protocol error or `UnexpectedMessage` for a message that
    /// is not an application event.
    pub fn next_event(&mut self) -> Result<ClientEvent, ClientError> {
        match kobo_protocol::read_from(&mut self.stream)?.message {
            Message::Action { action } => Ok(ClientEvent::Action(action)),
            Message::DeviceResult(result) => Ok(ClientEvent::Device(result)),
            Message::TaskOutcome { task, outcome } => Ok(ClientEvent::Task { task, outcome }),
            Message::StoreResult(result) => Ok(ClientEvent::Store(result)),
            Message::Lifecycle(state) => Ok(ClientEvent::Lifecycle(state)),
            Message::ShellEvent(event) => Ok(ClientEvent::Shell(event)),
            Message::Exit => Ok(ClientEvent::Exit),
            _ => Err(ClientError::UnexpectedMessage),
        }
    }

    fn send(&mut self, message: Message) -> Result<(), ClientError> {
        let request_id = self.next_request;
        self.next_request = self.next_request.wrapping_add(1).max(2);
        kobo_protocol::write_to(
            &mut self.stream,
            &Frame {
                request_id,
                message,
            },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    struct Example;

    impl KoboApp for Example {
        fn on_start(&mut self, context: &mut Context) {
            context.set_screen(Screen::new(
                1,
                vec![Node::Button {
                    id: NodeId(1),
                    action: ActionId(1),
                    label: "Tap".into(),
                }],
            ));
        }

        fn on_action(&mut self, context: &mut Context, action: ActionId) {
            context.log(LogLevel::Info, format!("action {}", action.0));
        }
    }

    struct Tofu;

    impl KoboApp for Tofu {
        fn on_start(&mut self, context: &mut Context) {
            context.set_screen(
                ScreenBuilder::new("tofu")
                    .button("ok", "Chosen \u{2713}")
                    .build(),
            );
        }

        fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
    }

    /// A character the face has no glyph for is an empty box on the panel, and
    /// the only place that is cheap to find out is here.
    #[cfg(all(debug_assertions, feature = "text"))]
    #[test]
    #[should_panic(expected = "which the installed face cannot draw")]
    fn a_screen_carrying_a_character_the_face_cannot_draw_fails_here() {
        AppRunner::new(Tofu).start();
    }

    #[test]
    fn runner_collects_lifecycle_commands() {
        let mut runner = AppRunner::new(Example);
        assert!(matches!(runner.start().as_slice(), [Command::SetScreen(_)]));
        assert!(runner.start().is_empty());
        assert!(matches!(
            runner.action(ActionId(9)).as_slice(),
            [Command::Log { .. }]
        ));
    }

    #[test]
    fn builder_uses_stable_nodes_and_action_names() {
        let builder = ScreenBuilder::new("hello")
            .heading("Hello, Kobo")
            .text("A dependency-free app")
            .button("close", "Close");
        assert_eq!(builder.action("close"), Some(action_id("close")));
        let screen = builder.build();
        assert_eq!(screen.id, ScreenBuilder::new("hello").build().id);
        assert_eq!(
            screen.nodes.iter().map(Node::id).collect::<Vec<_>>(),
            vec![NodeId(1), NodeId(2), NodeId(3)]
        );
        assert!(matches!(
            screen.nodes.last(),
            Some(Node::Button { action, .. }) if *action == action_id("close")
        ));
    }

    #[test]
    fn action_ids_are_name_deterministic() {
        assert_eq!(action_id("increment"), action_id("increment"));
        assert_ne!(action_id("increment"), action_id("close"));
    }

    #[test]
    fn client_handshake_and_screen_delivery() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().expect("socket pair");
        let daemon = thread::spawn(move || {
            let hello = kobo_protocol::read_from(&mut daemon_stream).expect("hello");
            assert!(matches!(hello.message, Message::Hello { .. }));
            kobo_protocol::write_to(
                &mut daemon_stream,
                &Frame {
                    request_id: hello.request_id,
                    message: Message::Welcome {
                        width: 1072,
                        height: 1448,
                        pixels_per_inch: 300,
                        text_scale: kobo_ui::TextScale::Default,
                    },
                },
            )
            .expect("welcome");
            let screen = kobo_protocol::read_from(&mut daemon_stream).expect("screen");
            assert!(matches!(screen.message, Message::SetScreen(_)));
        });
        let mut client = Client::from_stream(client_stream, "counter").expect("connect");
        assert_eq!(client.metrics(), kobo_ui::CLARA_BW_METRICS);
        client
            .send_commands([Command::SetScreen(
                ScreenBuilder::new("counter").heading("Counter").build(),
            )])
            .expect("send screen");
        daemon.join().expect("daemon");
    }

    #[test]
    fn client_transparently_chunks_a_full_width_picture() {
        let (client_stream, mut daemon_stream) = UnixStream::pair().expect("socket pair");
        let daemon = thread::spawn(move || {
            let hello = kobo_protocol::read_from(&mut daemon_stream).expect("hello");
            kobo_protocol::write_to(
                &mut daemon_stream,
                &Frame {
                    request_id: hello.request_id,
                    message: Message::Welcome {
                        width: 1072,
                        height: 1448,
                        pixels_per_inch: 300,
                        text_scale: kobo_ui::TextScale::Default,
                    },
                },
            )
            .expect("welcome");

            assert!(matches!(
                kobo_protocol::read_from(&mut daemon_stream)
                    .expect("begin")
                    .message,
                Message::BeginPicture {
                    handle: PictureHandle(9),
                    width: 1072,
                    height: 1448
                }
            ));
            let expected = 1072_usize * 1448;
            let mut received = 0;
            while received < expected {
                let Message::PictureChunk {
                    handle,
                    offset,
                    grey,
                } = kobo_protocol::read_from(&mut daemon_stream)
                    .expect("chunk")
                    .message
                else {
                    panic!("expected a picture chunk");
                };
                assert_eq!(handle, PictureHandle(9));
                assert_eq!(usize::try_from(offset).expect("offset"), received);
                assert!(grey.len() <= MAX_PICTURE_CHUNK_BYTES);
                received += grey.len();
            }
            assert!(matches!(
                kobo_protocol::read_from(&mut daemon_stream)
                    .expect("commit")
                    .message,
                Message::CommitPicture {
                    handle: PictureHandle(9)
                }
            ));
        });
        let mut client = Client::from_stream(client_stream, "gallery").expect("connect");
        client
            .send_commands([Command::PutPicture {
                handle: PictureHandle(9),
                width: 1072,
                height: 1448,
                grey: vec![127; 1072 * 1448],
            }])
            .expect("upload");
        daemon.join().expect("daemon");
    }
}

#[cfg(test)]
mod task_tests {
    use super::*;

    #[derive(Default)]
    struct Spawner {
        outcomes: Vec<(TaskId, TaskOutcome)>,
        spawn_on_action: bool,
    }

    impl KoboApp for Spawner {
        fn on_start(&mut self, context: &mut Context) {
            context.spawn(Task::Sleep { seconds: 1 });
        }

        fn on_action(&mut self, context: &mut Context, _action: ActionId) {
            if self.spawn_on_action {
                context.spawn(Task::Sleep { seconds: 1 });
            }
        }

        fn on_task(&mut self, _context: &mut Context, task: TaskId, outcome: TaskOutcome) {
            self.outcomes.push((task, outcome));
        }
    }

    fn spawned(commands: &[Command]) -> Vec<TaskId> {
        commands
            .iter()
            .filter_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn task_identifiers_stay_unique_across_separate_callbacks() {
        // A fresh Context is built for every callback. With the counters living
        // there, the second callback to spawn work handed out an identifier
        // already in use, and the two tasks would have reported back as one.
        let mut runner = AppRunner::new(Spawner {
            spawn_on_action: true,
            ..Spawner::default()
        });
        let first = spawned(&runner.start());
        let second = spawned(&runner.action(ActionId(1)));
        let third = spawned(&runner.action(ActionId(2)));
        assert_eq!(first, vec![TaskId(1)]);
        assert_eq!(second, vec![TaskId(2)]);
        assert_eq!(third, vec![TaskId(3)]);
    }

    #[test]
    fn work_in_flight_is_capped_rather_than_queued_without_limit() {
        struct Greedy(usize);
        impl KoboApp for Greedy {
            fn on_start(&mut self, context: &mut Context) {
                for _ in 0..MAX_TASKS_IN_FLIGHT + 5 {
                    if context.spawn(Task::Sleep { seconds: 1 }).is_some() {
                        self.0 += 1;
                    }
                }
            }
            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }
        let mut runner = AppRunner::new(Greedy(0));
        let commands = runner.start();
        assert_eq!(spawned(&commands).len(), MAX_TASKS_IN_FLIGHT);
        assert_eq!(runner.app().0, MAX_TASKS_IN_FLIGHT);
    }

    #[test]
    fn a_settled_task_frees_its_slot() {
        struct Filler;
        impl KoboApp for Filler {
            fn on_start(&mut self, context: &mut Context) {
                while context.spawn(Task::Sleep { seconds: 1 }).is_some() {}
            }
            fn on_action(&mut self, context: &mut Context, _action: ActionId) {
                context.spawn(Task::Sleep { seconds: 1 });
            }
            fn on_task(&mut self, _context: &mut Context, _task: TaskId, _outcome: TaskOutcome) {}
        }
        let mut runner = AppRunner::new(Filler);
        runner.start();
        assert_eq!(runner.tasks_in_flight(), MAX_TASKS_IN_FLIGHT);
        // Nothing has reported back, so there is still no room.
        assert!(spawned(&runner.action(ActionId(1))).is_empty());
        runner.task_outcome(TaskId(1), TaskOutcome::Completed(Vec::new()));
        assert_eq!(spawned(&runner.action(ActionId(2))).len(), 1);
    }

    #[test]
    fn a_cancelled_task_still_reaches_the_application() {
        let mut runner = AppRunner::new(Spawner::default());
        runner.start();
        runner.task_outcome(TaskId(1), TaskOutcome::Cancelled);
        assert_eq!(
            runner.app().outcomes,
            vec![(TaskId(1), TaskOutcome::Cancelled)]
        );
    }

    #[test]
    fn a_failed_task_still_reaches_the_application() {
        let mut runner = AppRunner::new(Spawner::default());
        runner.start();
        runner.task_outcome(TaskId(1), TaskOutcome::Failed(TaskError::Denied));
        assert_eq!(
            runner.app().outcomes,
            vec![(TaskId(1), TaskOutcome::Failed(TaskError::Denied))]
        );
    }

    #[test]
    fn a_callback_that_overruns_the_deadline_is_reported() {
        // The runtime cannot stop a callback from blocking, because it is the
        // callback that holds the thread. What it can do is refuse to let the
        // overrun go unnoticed.
        struct Slow;
        impl KoboApp for Slow {
            fn on_start(&mut self, _context: &mut Context) {
                std::thread::sleep(CALLBACK_DEADLINE + Duration::from_millis(60));
            }
            fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
        }
        let commands = AppRunner::new(Slow).start();
        assert!(matches!(
            commands.first(),
            Some(Command::Log {
                level: LogLevel::Warn,
                message,
            }) if message.contains("deadline")
        ));
    }

    #[test]
    fn a_prompt_callback_is_not_reported() {
        let commands = AppRunner::new(Spawner::default()).start();
        assert!(!commands.iter().any(|command| matches!(
            command,
            Command::Log {
                level: LogLevel::Warn,
                ..
            }
        )));
    }
}

/// Connects to the runtime and runs an application until it exits.
///
/// This is the whole of an application's `main`. It exists because every
/// application would otherwise hand-roll the same event loop, and each
/// hand-rolled copy is a chance to forget one of the things that has to be
/// right: collecting outstanding device answers before leaving, forwarding
/// every command, honouring a runtime request to exit, and never blocking.
///
/// The socket path comes from the environment the runtime provides, so an
/// application never names a path and never has to be told where it is running.
///
/// # Errors
///
/// Returns the first transport error. There is deliberately no retry: if the
/// runtime is gone, the screen belongs to something else now.
pub fn run<A: KoboApp>(name: &str, app: A) -> Result<(), ClientError> {
    let socket = std::env::var("KOBO_SOCKET").map_err(|_| ClientError::MissingSocket)?;
    run_on(name, app, Path::new(&socket))
}

/// Runs an application against a specific runtime socket.
///
/// # Errors
///
/// Returns the first transport error.
pub fn run_on<A: KoboApp>(name: &str, app: A, socket: &Path) -> Result<(), ClientError> {
    let mut client = Client::connect(socket, name)?;
    // The same typeface the runtime lays out with, so an application that
    // measures its own text agrees with what will actually be drawn. Failure
    // is not fatal: both sides then fall back to the built-in bitmap, which is
    // still one shared answer rather than two different ones.
    #[cfg(feature = "text")]
    let _ = kobo_text::install(client.metrics());
    let mut runner = AppRunner::with_metrics(app, client.metrics());
    client.send_commands(runner.start())?;

    // A test harness needs the application to settle and leave rather than wait
    // for a touch that will never come.
    let oneshot = std::env::var_os("KOBO_SIM_ONESHOT").is_some();
    if oneshot {
        while runner.outstanding_requests() > 0 {
            match client.next_event()? {
                ClientEvent::Device(result) => {
                    client.send_commands(runner.device_result(result))?;
                }
                ClientEvent::Task { task, outcome } => {
                    client.send_commands(runner.task_outcome(task, outcome))?;
                }
                ClientEvent::Store(result) => {
                    client.send_commands(runner.store_result(result))?;
                }
                ClientEvent::Lifecycle(state) => {
                    client.send_commands(runner.lifecycle(state))?;
                }
                ClientEvent::Shell(event) => {
                    client.send_commands(runner.shell_event(event))?;
                }
                ClientEvent::Action(_) | ClientEvent::Exit => break,
            }
        }
        client.send_commands([Command::Exit])?;
        return Ok(());
    }

    loop {
        let commands = match client.next_event()? {
            ClientEvent::Action(action) => runner.action(action),
            ClientEvent::Device(result) => runner.device_result(result),
            ClientEvent::Task { task, outcome } => runner.task_outcome(task, outcome),
            ClientEvent::Store(result) => runner.store_result(result),
            ClientEvent::Lifecycle(state) => runner.lifecycle(state),
            ClientEvent::Shell(event) => runner.shell_event(event),
            ClientEvent::Exit => {
                // The runtime is taking the screen back. Give the application
                // its exit callback, then go, rather than arguing about it.
                let _ = client.send_commands(runner.exit());
                return Ok(());
            }
        };
        let leaving = commands
            .iter()
            .any(|command| matches!(command, Command::Exit));
        client.send_commands(commands)?;
        if leaving {
            return Ok(());
        }
    }
}
