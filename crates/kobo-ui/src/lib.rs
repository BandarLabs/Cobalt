#![forbid(unsafe_code)]
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::only_used_in_recursion,
    clippy::too_many_lines
)]

//! A small retained UI tree and grayscale rasterizer for the Kobo display.

use std::cmp::{max, min};
use std::sync::OnceLock;

pub const DISPLAY_WIDTH: i32 = 1072;
pub const DISPLAY_HEIGHT: i32 = 1448;
const MAX_LAYOUT_NODES: usize = 512;
const MAX_LAYOUT_DEPTH: usize = 16;
/// Beyond a handful of options a list stops being a choice and becomes a menu,
/// which is what [`Node::PagedList`] is for.
pub const MAX_CHOICE_OPTIONS: usize = 6;

/// The deepest a [`Node::Quote`] is drawn.
///
/// Measured rather than picked: one indent step is one small space, and this
/// panel's text column is 91 mm wide. Past three steps a reply has lost a
/// quarter of its measure, and a discussion that nests forty deep — which real
/// ones do — would otherwise end up one word per line. Deeper replies keep
/// their true depth in their byline and share the deepest indent.
pub const MAX_QUOTE_DEPTH: u8 = 3;

/// The most rows one list may declare.
///
/// A bound exists so a screen cannot become unboundedly tall from data; a list
/// longer than this wants paging, which is a different primitive.
pub const MAX_ROWS: usize = 32;

/// The most rows a [`Node::Terminal`] may carry.
///
/// Sized from the panel this was built for rather than chosen round: the
/// smallest text on a 1448-pixel-tall screen gives about 37 rows, so 64 leaves
/// room for a taller panel without letting a screen become unboundedly large
/// from data.
pub const MAX_TERMINAL_ROWS: usize = 64;

/// The most characters one terminal row may carry.
///
/// 53 columns fit across this panel; 160 is the widest terminal anyone
/// conventionally uses, and anything past the grid is dropped, never wrapped.
pub const MAX_TERMINAL_COLUMNS: usize = 160;

/// The character grid that fits in a region of the given pixel size.
///
/// Both the layout engine and the application that is feeding a terminal have
/// to agree on this exactly, or the pseudo-terminal is told one width and the
/// panel shows another, and every line wraps in the wrong place. Deriving both
/// from one function is what makes that impossible rather than unlikely.
#[must_use]
pub fn terminal_grid(width: i32, height: i32) -> (u16, u16) {
    let (cell_width, cell_height) = mono_cell(TERMINAL_SIZE);
    let columns = (max(0, width) / max(1, cell_width)).clamp(0, MAX_TERMINAL_COLUMNS as i32);
    let rows = (max(0, height) / max(1, cell_height)).clamp(0, MAX_TERMINAL_ROWS as i32);
    (columns as u16, rows as u16)
}

/// Terminal text is set at the smallest size, because a terminal's value is in
/// how much of it can be seen at once and a shell's output is read in glances
/// rather than at length.
const TERMINAL_SIZE: FontSize = FontSize::Caption;

/// The physical characteristics of a panel the UI is being laid out for.
///
/// Sizes throughout this crate are derived from millimetre measurements rather
/// than pixel counts, because a pixel constant silently means a different
/// physical size on every panel. Kobo resolutions and densities vary widely:
/// roughly 212 to 300 pixels per inch, and 758x1024 up to 1440x1920. A number
/// that is correct on one is wrong on the rest.
///
/// This is a rendering concern only. It does not loosen device support: the
/// hardware profile gate stays exact, and unknown hardware is still rejected
/// rather than mapped onto a similar model.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DisplayMetrics {
    pub width: i32,
    pub height: i32,
    pub pixels_per_inch: i32,
    /// The reader's preferred text size, supplied by the runtime.
    pub text_scale: TextScale,
}

/// A small, deliberate accessibility scale rather than an arbitrary zoom.
///
/// Applications continue to ask for semantic sizes such as [`FontSize::Body`].
/// The runtime applies this preference to every face, so pagination performed
/// in an application and rendering performed on the device remain identical.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum TextScale {
    #[default]
    Default = 0,
    Large = 1,
    ExtraLarge = 2,
}

impl TextScale {
    /// Percentage applied to the physical type size.
    #[must_use]
    pub const fn percent(self) -> i32 {
        match self {
            Self::Default => 100,
            Self::Large => 120,
            Self::ExtraLarge => 140,
        }
    }

    /// A stable wire representation used by `kobo-protocol`.
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    /// Decodes the stable wire representation.
    #[must_use]
    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Default),
            1 => Some(Self::Large),
            2 => Some(Self::ExtraLarge),
            _ => None,
        }
    }

    /// Parses values accepted by the runtime's `KOBO_TEXT_SCALE` setting.
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "100" | "100%" => Some(Self::Default),
            "large" | "120" | "120%" => Some(Self::Large),
            "extra-large" | "extra_large" | "xl" | "140" | "140%" => Some(Self::ExtraLarge),
            _ => None,
        }
    }
}

/// The only panel with hardware support today.
pub const CLARA_BW_METRICS: DisplayMetrics = DisplayMetrics {
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
    text_scale: TextScale::Default,
};

/// Returns the supported panel with the process-level accessibility setting.
#[must_use]
pub fn display_metrics_from_env() -> DisplayMetrics {
    let mut metrics = CLARA_BW_METRICS;
    if let Ok(value) = std::env::var("KOBO_TEXT_SCALE") {
        if let Some(scale) = TextScale::from_name(&value) {
            metrics.text_scale = scale;
        }
    }
    metrics
}

impl Default for DisplayMetrics {
    fn default() -> Self {
        CLARA_BW_METRICS
    }
}

impl DisplayMetrics {
    /// Converts a tenth of a millimetre to whole pixels, rounding to nearest.
    ///
    /// Tenths because whole millimetres are too coarse for a type scale, and
    /// integers because layout has to produce identical results on the host
    /// and the device.
    #[must_use]
    pub const fn tenth_mm(&self, tenths: i32) -> i32 {
        // pixels = tenths / 10 / 25.4 * dpi, rearranged to stay in integers.
        (tenths * self.pixels_per_inch + 127) / 254
    }

    /// Converts a semantic type measurement to pixels with the user's text
    /// preference applied. Spacing and touch targets intentionally do not use
    /// this path: larger type needs more room, not larger fingers.
    #[must_use]
    pub const fn scaled_type_tenth_mm(&self, tenths: i32) -> i32 {
        self.tenth_mm((tenths * self.text_scale.percent() + 50) / 100)
    }

    /// Physical width in tenths of a millimetre.
    #[must_use]
    pub const fn width_tenth_mm(&self) -> i32 {
        (self.width * 254) / self.pixels_per_inch
    }

    /// The smallest target a finger can reliably hit, seven millimetres.
    ///
    /// A widely copied guideline is 44 units, which is seven millimetres only
    /// at the 163 pixels per inch it was written for. Taken as 44 pixels on a
    /// 300 pixel per inch panel it is 3.7 millimetres, about half the intended
    /// size, and on a 212 pixel per inch panel it is 5.3.
    #[must_use]
    pub const fn touch_target_minimum(&self) -> i32 {
        self.tenth_mm(70)
    }

    /// The comfortable default for a control, ten millimetres.
    #[must_use]
    pub const fn touch_target_default(&self) -> i32 {
        self.tenth_mm(100)
    }

    /// The margin between screen content and the bezel.
    #[must_use]
    pub const fn screen_margin(&self) -> i32 {
        self.tenth_mm(40)
    }

    /// A rule has to be thick enough to read as a line. One pixel is about
    /// 0.08 millimetres at 300 pixels per inch, which disappears.
    #[must_use]
    pub const fn rule_thickness(&self) -> i32 {
        self.tenth_mm(3)
    }

    /// The height of the fixed bar that carries the title and the way back.
    ///
    /// Eleven millimetres to start with, which was a millimetre more than the
    /// comfortable control default for no reason beyond looking settled, and
    /// on a 122 millimetre panel that is nine per cent of everything the
    /// reader has. Eight and a half is a quarter off and still one and a half
    /// millimetres above [`Self::touch_target_minimum`], which matters because
    /// this bar carries Back — the one control that is guaranteed to work, and
    /// so the one that must never be the size of a guess.
    #[must_use]
    pub const fn top_bar_height(&self) -> i32 {
        self.tenth_mm(85)
    }

    #[must_use]
    pub const fn nav_bar_height(&self) -> i32 {
        self.tenth_mm(120)
    }

    /// How many columns this panel can carry without the text becoming
    /// unreadable, derived from physical width rather than assumed.
    ///
    /// A column narrower than about 45 millimetres cannot hold a sensible line
    /// of text, so a 91 millimetre six inch panel gets two and a 157 millimetre
    /// ten inch one gets three.
    #[must_use]
    pub const fn max_grid_columns(&self) -> usize {
        let columns = (self.width_tenth_mm() / 450) as usize;
        if columns < 1 {
            1
        } else if columns > 4 {
            4
        } else {
            columns
        }
    }

    /// How many columns a grid of this tile shape gets.
    ///
    /// Deliberately not [`Self::max_grid_columns`], which answers a different
    /// question. That one asks how narrow a column of *text* may be, and 45
    /// millimetres is the honest answer. A tile carries a mark and a one-line
    /// label, so the binding constraint is a finger and a recognisable icon,
    /// not a line of prose — holding tiles to the text figure gave a Clara two
    /// 41 millimetre squares per row, which is a grid of four enormous buttons
    /// where a phone would show nine.
    ///
    /// Portrait cells are held wider because they carry artwork someone has to
    /// recognise, and a 25 millimetre book cover is a postage stamp.
    #[must_use]
    pub const fn grid_columns(&self, shape: TileShape) -> usize {
        let minimum = shape.minimum_cell_tenth_mm();
        let usable = self.width_tenth_mm() - 80;
        let columns = (usable / minimum) as usize;
        if columns < 1 {
            1
        } else if columns > 5 {
            5
        } else {
            columns
        }
    }

    /// A bar with one destination is not navigation, and targets narrower than
    /// a finger are not usable, so the ceiling follows physical width too.
    #[must_use]
    pub const fn max_nav_destinations(&self) -> usize {
        let usable = self.width - 2 * self.screen_margin();
        let fits = (usable / self.touch_target_minimum()) as usize;
        if fits < MIN_NAV_DESTINATIONS {
            MIN_NAV_DESTINATIONS
        } else if fits > 5 {
            5
        } else {
            fits
        }
    }

    /// The spacing scale in pixels. The base step is one millimetre.
    #[must_use]
    pub const fn space(&self, space: Space) -> i32 {
        self.tenth_mm(match space {
            Space::Tight => 10,
            Space::Small => 20,
            Space::Medium => 40,
            Space::Large => 60,
        })
    }
}

/// A bar with fewer destinations than this is not navigation.
pub const MIN_NAV_DESTINATIONS: usize = 2;

/// Grayscale values used by the built-in monochrome design system.
///
/// A GC16 refresh resolves sixteen levels and the middle ones ghost, so the
/// palette is deliberately tiny. Paper is pure white because that is the
/// panel's rest state.
pub mod vector;

pub mod tone {
    pub const PAPER: u8 = 255;
    pub const SURFACE: u8 = 232;
    pub const INK: u8 = 0;
    pub const MUTED: u8 = 96;
    pub const RULE: u8 = 160;
    pub const FOCUS: u8 = 0;
}

/// A whole percentage, clamped to a possible value on construction.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Percent(u8);

impl Percent {
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(if value > 100 { 100 } else { value })
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Snaps to the nearest five percent.
    ///
    /// Progress that updates every percent would ask the panel for a hundred
    /// refreshes over the life of one download. At five percent steps the bar
    /// still reads as moving, costs twenty refreshes, and each step is a
    /// visible change rather than a sub-pixel one.
    #[must_use]
    pub const fn coarse(self) -> Self {
        Self((self.0 + 2) / 5 * 5)
    }
}

impl std::fmt::Display for Percent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The spacing a screen may ask for.
///
/// Deliberately an enum rather than a number. A free integer lets an author
/// invent spacing that does not belong to the scale, and a signed one lets them
/// write a negative gap that overlaps the nodes around it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Space {
    Tight,
    #[default]
    Small,
    Medium,
    Large,
}

impl Space {
    /// Pixels on the only panel with hardware support.
    ///
    /// Prefer [`DisplayMetrics::space`] wherever the target panel is known.
    #[must_use]
    pub const fn pixels(self) -> i32 {
        CLARA_BW_METRICS.space(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(pub u32);

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActionId(pub u32);

impl ActionId {
    /// The reserved identifier for going back.
    ///
    /// Back is owned by the runtime's navigation stack rather than by the
    /// application, so no application can decline to offer it or wire it to
    /// something else. Nickel's own browser has exactly that defect, and
    /// `NickelMenu` ships a workaround for it.
    pub const BACK: Self = Self(u32::MAX);

    /// Whether this identifier belongs to the runtime rather than an app.
    #[must_use]
    pub const fn is_reserved(self) -> bool {
        self.0 == Self::BACK.0
    }
}

/// Runtime-owned decoration that an application cannot describe for itself.
///
/// This is deliberately not part of [`Screen`], is not carried on the wire, and
/// has no builder method. The runtime supplies it at render time from the
/// navigation stack, which is the only way to guarantee that an application
/// cannot trap the reader on a screen with no way out.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Chrome {
    pub back: bool,
}

impl Chrome {
    #[must_use]
    pub const fn with_back(back: bool) -> Self {
        Self { back }
    }
}

/// Gives a screen a top bar to put the way back in, when it has none.
///
/// The way back is drawn in the top bar, so an application that did not ask
/// for one would otherwise trap the reader. The runtime supplies the bar
/// rather than trusting every application to remember, titled with the
/// application's own name so nothing is invented.
///
/// Here rather than in the daemon because the daemon has two renderers — the
/// panel and the host simulation — and only one of them was doing this. A
/// preview drawn without the way back is a preview of a screen that will never
/// exist, and it hides the one defect that leaves somebody stuck.
#[must_use]
pub fn ensure_way_back(mut screen: Screen, chrome: Chrome, name: &str) -> Screen {
    if chrome.back && screen.top_bar.is_none() {
        screen = screen.with_top_bar(TopBar::new(NodeId(0), name));
    }
    screen
}

/// A single tappable label in a bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BarAction {
    pub action: ActionId,
    pub label: String,
}

/// Whether a control can currently be activated.
///
/// Disabled is semantic state rather than a colour chosen by the application:
/// the renderer gives it a quiet, outlined treatment, it yields no action, and
/// it still absorbs the tap that lands on it rather than letting the page turn
/// underneath answer instead.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ControlState {
    #[default]
    Enabled,
    Disabled,
}

impl ControlState {
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

impl BarAction {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
        }
    }
}

/// The fixed bar at the top of a screen.
///
/// Carries a title and at most one action. The cap is the point: a bar that
/// accepts a list of actions becomes a toolbar, and a toolbar on a panel this
/// size produces targets too small to hit. Back is not a field here because it
/// belongs to the runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopBar {
    pub id: NodeId,
    pub title: String,
    pub action: Option<BarAction>,
}

impl TopBar {
    #[must_use]
    pub fn new(id: NodeId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            action: None,
        }
    }

    #[must_use]
    pub fn action(mut self, action: ActionId, label: impl Into<String>) -> Self {
        self.action = Some(BarAction::new(action, label));
        self
    }
}

/// A single control pinned to the bottom band, in place of navigation.
///
/// Structurally separate from [`NavBar`] rather than a bar of one destination,
/// because they are different things: a bar says where you are among places
/// you could be, and this says there is one way off this screen. A bar of one
/// is refused everywhere else in this layer for exactly that reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BottomAction {
    pub id: NodeId,
    pub action: BarAction,
}

impl BottomAction {
    #[must_use]
    pub const fn new(id: NodeId, action: BarAction) -> Self {
        Self { id, action }
    }
}

/// The fixed bar at the bottom of a screen, equivalent to the reader's own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavBar {
    pub id: NodeId,
    pub destinations: Vec<BarAction>,
    /// Which destination the reader is currently looking at, if any.
    ///
    /// `None` is a real answer rather than a missing one. A bar whose entries
    /// are *actions* — previous page, next page, the way out — has no current
    /// destination to mark, and marking one anyway tells the reader they are
    /// somewhere they are not. This used to be a plain `usize`, and the two
    /// screens that meant "none" said `usize::MAX`, which survived exactly as
    /// far as the wire: the byte saturated to 255 and the decoder clamped it
    /// to the last destination, so both screens shipped with the wrong entry
    /// underlined on the panel.
    pub selected: Option<usize>,
}

impl NavBar {
    #[must_use]
    pub fn new(id: NodeId, destinations: Vec<BarAction>, selected: Option<usize>) -> Self {
        Self {
            id,
            destinations,
            selected,
        }
    }

    /// The destinations that will actually be shown on a given panel.
    ///
    /// A bar with one destination is not navigation, and destinations narrower
    /// than a finger cannot be tapped, so the count is clamped to what the
    /// panel can physically carry rather than honoured blindly.
    #[must_use]
    pub fn visible(&self, metrics: &DisplayMetrics) -> &[BarAction] {
        let limit = min(self.destinations.len(), metrics.max_nav_destinations());
        &self.destinations[..limit]
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Screen {
    pub id: u32,
    /// Optional fixed top bar. Structurally outside the node list so a screen
    /// cannot carry two of them, bury one inside a card, or place one halfway
    /// down the page.
    pub top_bar: Option<TopBar>,
    pub nodes: Vec<Node>,
    /// Optional fixed bottom bar, pinned to the panel rather than the flow.
    pub nav_bar: Option<NavBar>,
    /// A single control pinned to the bottom band, in place of a bar.
    ///
    /// A screen with one way off it — a launcher's way back to the reader, a
    /// dialogue's way out — has no navigation to draw, and a bar of one
    /// destination is refused precisely because it is not navigation. Placed
    /// in the flow instead, that control is the first thing a long page pushes
    /// off the bottom: the launcher shipped with its way back to the Kobo
    /// reader hanging five pixels past the edge of the panel, because the
    /// space reserved for a bar and the space a trailing rule and button
    /// actually need are not the same number and nothing was comparing them.
    /// Pinned, it occupies exactly the band that was already reserved, so the
    /// two cannot disagree.
    pub bottom_action: Option<BottomAction>,
    /// Turning the page by tapping the side of the panel.
    ///
    /// This is how every Kobo has always worked, and it is muscle memory for
    /// anyone holding one: the left edge goes back, the rest goes forward. A
    /// paged screen that made the reader find a small control at the bottom
    /// instead would be a worse reader than the one it replaced.
    ///
    /// It is deliberately a property of the screen rather than a node. The
    /// zones are whatever is left of the content area once every real control
    /// has been hit-tested, so a button, a row or a keyboard key always wins:
    /// a tap can never turn the page *and* press something.
    pub page_turns: Option<PageTurns>,
    /// Whether the application has somewhere of its own to go back to.
    ///
    /// The runtime still owns the control and still decides. This only says
    /// that the application would like first refusal on it: when set, the tap
    /// arrives as [`ActionId::BACK`] instead of leaving for the launcher, so a
    /// screen reached from inside an application returns to the screen it was
    /// reached from rather than out of the application altogether.
    ///
    /// It cannot be used to trap the reader. An application offered Back that
    /// does not then draw something new is left behind and the launcher shown
    /// anyway, so the worst this can do is delay the way out once.
    pub owns_back: bool,
    /// Whether this screen's text is a book rather than an interface.
    ///
    /// Sets prose in a serif drawn for continuous reading and opens the lines
    /// up to the measure books have always used. Off everywhere else, because
    /// the interface face is chosen so that a label glanced at once cannot be
    /// misread, which is a different job and a different answer.
    pub reading: bool,

    /// A text size this screen asks for, overriding the reader's own setting.
    ///
    /// `None` means inherit, which is what almost every screen should do: the
    /// scale is an accessibility preference and an application that overrides
    /// it is overruling someone who has already said how big they need type to
    /// be. The exception this exists for is a reader, where the size of the
    /// body text *is* the thing being adjusted and the adjustment belongs to
    /// the book rather than to the device.
    ///
    /// An application that sets this must paginate at the same scale, or the
    /// page it measured is not the page that gets drawn.
    pub text_scale: Option<TextScale>,
}

/// The actions a tap on the left or right of the content area sends.
///
/// Some Kobo models also have physical page buttons. When those are wired up
/// they will send the same two actions, which is the reason this is a pair of
/// intents rather than a pair of touch zones.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageTurns {
    pub previous: ActionId,
    pub next: ActionId,
}

impl PageTurns {
    #[must_use]
    pub const fn new(previous: ActionId, next: ActionId) -> Self {
        Self { previous, next }
    }
}

/// The share of the width that goes back rather than forward.
///
/// A third, matching the stock reader. Forward is the common direction, so it
/// gets the larger share and the thumb that is already resting on the right
/// edge of the panel.
const BACK_ZONE: i32 = 3;

impl Screen {
    #[must_use]
    pub fn new(id: u32, nodes: Vec<Node>) -> Self {
        Self {
            id,
            top_bar: None,
            nodes,
            nav_bar: None,
            bottom_action: None,
            page_turns: None,
            owns_back: false,
            reading: false,
            text_scale: None,
        }
    }

    /// Asks for first refusal on the runtime's Back control.
    ///
    /// Pass the application's own answer to "is there anywhere to go back to",
    /// so the last screen of an application's own stack still leaves for the
    /// launcher rather than swallowing the tap and appearing to do nothing.
    #[must_use]
    pub const fn with_reading(mut self, reading: bool) -> Self {
        self.reading = reading;
        self
    }

    #[must_use]
    pub const fn with_text_scale(mut self, text_scale: Option<TextScale>) -> Self {
        self.text_scale = text_scale;
        self
    }

    #[must_use]
    pub const fn with_own_back(mut self, owns_back: bool) -> Self {
        self.owns_back = owns_back;
        self
    }

    /// Turns the sides of the content area into page turns.
    #[must_use]
    pub const fn with_page_turns(mut self, previous: ActionId, next: ActionId) -> Self {
        self.page_turns = Some(PageTurns::new(previous, next));
        self
    }

    #[must_use]
    pub fn with_top_bar(mut self, top_bar: TopBar) -> Self {
        self.top_bar = Some(top_bar);
        self
    }

    #[must_use]
    pub fn with_nav_bar(mut self, nav_bar: NavBar) -> Self {
        self.nav_bar = Some(nav_bar);
        self.bottom_action = None;
        self
    }

    /// Pins one control to the bottom band instead of a row of destinations.
    ///
    /// The two are mutually exclusive because they are the same band: a screen
    /// carrying both would draw one over the other. Setting either clears the
    /// other rather than leaving that to be discovered on a panel.
    #[must_use]
    pub fn with_bottom_action(mut self, action: BottomAction) -> Self {
        self.bottom_action = Some(action);
        self.nav_bar = None;
        self
    }

    /// Lays the screen out for the only panel with hardware support.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout_for(&CLARA_BW_METRICS)
    }

    /// Lays the screen out for a specific panel.
    #[must_use]
    pub fn layout_for(&self, metrics: &DisplayMetrics) -> Layout {
        self.layout_with(metrics, Chrome::default())
    }

    /// Lays the screen out for a panel, including runtime-owned decoration.
    #[must_use]
    pub fn layout_with(&self, metrics: &DisplayMetrics, chrome: Chrome) -> Layout {
        let margin = metrics.screen_margin();
        let gap = metrics.space(Space::Tight);
        let prose = if self.reading {
            Face::Reading
        } else {
            Face::Text
        };
        let mut layout = Layout {
            prose_face: prose,
            ..Layout::default()
        };

        let mut cursor = margin;
        if let Some(top_bar) = &self.top_bar {
            cursor = layout_top_bar(top_bar, chrome, metrics, &mut layout);
            cursor = cursor.saturating_add(gap);
        }
        let content_top = cursor;

        // The bottom bar is pinned to the panel, so content is bounded by it
        // rather than flowing underneath. Reserving the band up front is what
        // lets a tab switch repaint the content area and two bars instead of
        // the whole screen, which is the difference between one refresh and
        // one refresh plus visible chrome flicker.
        let content_bottom = if self.nav_bar.is_some() || self.bottom_action.is_some() {
            metrics.height - metrics.nav_bar_height()
        } else {
            metrics.height
        };

        for node in &self.nodes {
            if layout.nodes.len() >= MAX_LAYOUT_NODES || cursor >= content_bottom {
                break;
            }
            cursor = layout_node(
                node,
                margin,
                cursor,
                metrics.width - 2 * margin,
                0,
                metrics,
                prose,
                &mut layout,
            );
            cursor = cursor.saturating_add(gap);
        }

        if let Some(nav_bar) = &self.nav_bar {
            layout_nav_bar(nav_bar, metrics, &mut layout);
        }
        if let Some(action) = &self.bottom_action {
            layout_bottom_action(action, metrics, &mut layout);
        }
        // The page-turn zones are the content area, which starts below the top
        // bar and stops above the nav bar. Never the bars themselves: Back and
        // the navigation are the two things a reader must be able to hit
        // without thinking, and a mistimed page turn there would be maddening.
        layout.page_turns = self.page_turns;
        layout.content = Rect {
            x: 0,
            y: content_top,
            width: metrics.width,
            height: max(0, content_bottom - content_top),
        };
        layout
    }

    #[must_use]
    pub fn hit_test(&self, x: i32, y: i32) -> Option<ActionId> {
        self.layout().hit_test(x, y)
    }
}

fn layout_top_bar(
    top_bar: &TopBar,
    chrome: Chrome,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    let margin = metrics.screen_margin();
    let height = metrics.top_bar_height();
    let width = metrics.width - 2 * margin;
    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: 0,
            y: 0,
            width: metrics.width,
            height,
        },
        kind: LayoutKind::TopBar,
        text_lines: Vec::new(),
    });

    // Never taller than the bar it sits in. The comfortable control default is
    // ten millimetres and the bar is eight and a half, so taken literally this
    // put a control that overhangs its own bar at a negative offset — the back
    // chevron was drawn larger than the bar, sticking out above it. Clamped
    // here rather than at each control, because every one of them is centred
    // against the same height.
    let control = min(metrics.touch_target_default(), height);
    let mut title_x = margin;
    let mut title_width = width;
    if chrome.back {
        layout.nodes.push(LayoutNode {
            id: top_bar.id,
            rect: Rect {
                x: margin,
                y: (height - control) / 2,
                width: control,
                height: control,
            },
            kind: LayoutKind::Back,
            text_lines: Vec::new(),
        });
        let taken = control.saturating_add(metrics.space(Space::Small));
        title_x = title_x.saturating_add(taken);
        title_width = title_width.saturating_sub(taken);
    }
    if let Some(action) = &top_bar.action {
        let (text_width, _) = measure_text(&action.label, FontSize::Body);
        let action_width = max(
            control,
            text_width.saturating_add(metrics.space(Space::Medium)),
        );
        layout.nodes.push(LayoutNode {
            id: top_bar.id,
            rect: Rect {
                x: metrics.width - margin - action_width,
                y: (height - control) / 2,
                width: action_width,
                height: control,
            },
            kind: LayoutKind::BarAction(action.action),
            text_lines: vec![action.label.clone()],
        });
        title_width =
            title_width.saturating_sub(action_width.saturating_add(metrics.space(Space::Small)));
    }

    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: title_x,
            y: (height - FontSize::Title.line_height()) / 2,
            width: max(0, title_width),
            height: FontSize::Title.line_height(),
        },
        kind: LayoutKind::TopBarTitle,
        // One line only. A title that wraps is a title that is too long, and
        // growing the bar to fit it would move every screen's content.
        //
        // Ellipsised rather than simply cut. Keeping the first wrapped line
        // and dropping the rest silently reads as the whole title: a Hacker
        // News thread titled "US citizen charged after GrapheneOS phone wipes
        // during airport search" appeared on the panel as "US citizen charged
        // after", which is a different and much worse sentence.
        text_lines: vec![one_line(&top_bar.title, title_width, FontSize::Title)],
    });

    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: 0,
            y: height,
            width: metrics.width,
            height: metrics.rule_thickness(),
        },
        kind: LayoutKind::Divider,
        text_lines: Vec::new(),
    });
    height.saturating_add(metrics.rule_thickness())
}

/// Draws one control in the band a bottom bar would have occupied.
///
/// Deliberately the same reserved height as a nav bar and the same rule above
/// it, so the two are interchangeable from the content's point of view and a
/// screen that swaps one for the other does not reflow. The control is a
/// [`LayoutKind::Button`] like any other, which is what makes it hit-tested,
/// drawn and repainted by the code that already does all three.
fn layout_bottom_action(bottom: &BottomAction, metrics: &DisplayMetrics, layout: &mut Layout) {
    let band = metrics.nav_bar_height();
    let top = metrics.height - band;
    let rule = metrics.rule_thickness();
    layout.nodes.push(LayoutNode {
        id: bottom.id,
        rect: Rect {
            x: 0,
            y: top,
            width: metrics.width,
            height: band,
        },
        kind: LayoutKind::Spacer,
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id: bottom.id,
        rect: Rect {
            x: 0,
            y: top,
            width: metrics.width,
            height: rule,
        },
        kind: LayoutKind::Divider,
        text_lines: Vec::new(),
    });
    let margin = metrics.screen_margin();
    let width = max(1, metrics.width - margin * 2);
    // Never taller than the band it was given, and centred in what is left of
    // it below the rule, so the control has the same air above and below
    // instead of sitting on the bottom edge of the panel.
    let height = min(
        band.saturating_sub(rule),
        max(
            metrics.touch_target_minimum(),
            metrics.touch_target_default(),
        ),
    );
    let y = top
        .saturating_add(rule)
        .saturating_add((band - rule - height) / 2);
    layout.nodes.push(LayoutNode {
        id: bottom.id,
        rect: Rect {
            x: margin,
            y,
            width,
            height,
        },
        kind: LayoutKind::Button(bottom.action.action, ControlState::Enabled),
        text_lines: vec![one_line(&bottom.action.label, width - 32, FontSize::Body)],
    });
}

fn layout_nav_bar(nav_bar: &NavBar, metrics: &DisplayMetrics, layout: &mut Layout) {
    let visible = nav_bar.visible(metrics);
    if visible.len() < MIN_NAV_DESTINATIONS {
        return;
    }
    let height = metrics.nav_bar_height();
    let top = metrics.height - height;
    layout.nodes.push(LayoutNode {
        id: nav_bar.id,
        rect: Rect {
            x: 0,
            y: top,
            width: metrics.width,
            height,
        },
        kind: LayoutKind::NavBar,
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id: nav_bar.id,
        rect: Rect {
            x: 0,
            y: top,
            width: metrics.width,
            height: metrics.rule_thickness(),
        },
        kind: LayoutKind::Divider,
        text_lines: Vec::new(),
    });

    let count = visible.len() as i32;
    let slot = metrics.width / count;
    for (index, destination) in visible.iter().enumerate() {
        let x = slot * index as i32;
        // The last slot absorbs the division remainder so the bar always spans
        // the full panel and never leaves a dead strip on the right edge.
        let width = if index + 1 == visible.len() {
            metrics.width - x
        } else {
            slot
        };
        layout.nodes.push(LayoutNode {
            id: nav_bar.id,
            rect: Rect {
                x,
                y: top,
                width,
                height,
            },
            kind: if nav_bar.selected == Some(index) {
                LayoutKind::NavDestinationSelected(destination.action)
            } else {
                LayoutKind::NavDestination(destination.action)
            },
            text_lines: vec![destination.label.clone()],
        });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    Heading {
        id: NodeId,
        text: String,
    },
    Text {
        id: NodeId,
        text: String,
    },
    /// A paragraph set in from the left, with a rule marking what it answers.
    ///
    /// Threaded discussion is not a niche: replies, quoted mail, nested
    /// comments and annotated diffs are all the same shape, and every one of
    /// them was previously reduced to drawing arrows in the text because no
    /// node could express depth. Depth is a small number rather than a
    /// measurement, so an application still cannot choose pixels, and it is
    /// capped at [`MAX_QUOTE_DEPTH`] because an indent that keeps growing
    /// leaves a nine-deep reply one word wide on a panel this narrow.
    Quote {
        id: NodeId,
        /// How many levels in. Clamped on construction.
        depth: u8,
        text: String,
    },
    Button {
        id: NodeId,
        action: ActionId,
        label: String,
        state: ControlState,
    },
    Card {
        id: NodeId,
        children: Vec<Node>,
    },
    Divider {
        id: NodeId,
    },
    Spacer {
        id: NodeId,
        space: Space,
    },
    Progress {
        id: NodeId,
        /// Percentage complete. Clamped on construction so a screen can never
        /// describe a bar that is more than full.
        value: Percent,
    },
    PagedList {
        id: NodeId,
        page: u16,
        items: Vec<String>,
    },
    /// A vertical list of tappable entries, each explaining itself.
    ///
    /// This is the right shape whenever entries need describing rather than
    /// just naming: a square tile forces a one-word label and wastes most of
    /// its area, while a row gives a sentence for the price of one line.
    /// A grid of equally sized, individually tappable cells.
    ///
    /// This is the general one. A tile grid is opinionated — it picks its own
    /// column count for readability and expects an icon and a word — and that
    /// is right for a launcher and wrong for everything else. A board, a
    /// keyboard, a calculator and a colour picker are all the same shape, and
    /// none of them should need a new primitive in the protocol. So the caller
    /// chooses the columns, and whether cells are square or a single row high.
    Grid {
        id: NodeId,
        columns: u8,
        square: bool,
        cells: Vec<Cell>,
    },
    Rows {
        id: NodeId,
        rows: Vec<Row>,
    },
    /// A grid of large tappable tiles, the launcher's primary surface.
    TileGrid {
        id: NodeId,
        tiles: Vec<Tile>,
        shape: TileShape,
    },
    /// The tap-first question primitive.
    ///
    /// Typing on a device with no keyboard and a refresh measured in tens of
    /// milliseconds is markedly worse than tapping, so the shape of the node
    /// pushes authors toward offering answers rather than demanding prose.
    Choice {
        id: NodeId,
        prompt: String,
        options: Vec<BarAction>,
        /// Which option is already the answer, if one is.
        ///
        /// Carried as state rather than drawn into a label, for the same
        /// reason a finished row in a [`Node::Rows`] list is: an application
        /// that marks its own choice with a character picks one the installed
        /// face may not have, and gets a missing-glyph box on the panel. The
        /// renderer marks it with an icon from the atlas instead.
        selected: Option<u8>,
        /// Optional escape hatch, shown last, for when none of the options fit.
        /// The keyboard is only summoned if this row is actually tapped.
        freeform: Option<Freeform>,
    },
    /// An attention strip. This is the supported alternative to flashing the
    /// frontlight, which is a photosensitivity hazard and a battery cost.
    Banner {
        id: NodeId,
        level: BannerLevel,
        text: String,
    },
    /// A placeholder occupying the exact space real content will occupy.
    ///
    /// Static by construction. There is no spinner and no animation anywhere in
    /// this system, because every frame of an animation is a panel refresh.
    Skeleton {
        id: NodeId,
        lines: u8,
    },
    /// A picture the runtime is already holding, drawn as large as the space
    /// allows without changing its shape.
    ///
    /// The pixels are deliberately not here. A screen is re-sent on every
    /// change — that is what makes the model simple — and a book cover is
    /// eighty thousand bytes, so carrying them inline would put a cover on the
    /// wire for every tap. Instead the application hands the picture over once
    /// and refers to it afterwards by `handle`.
    ///
    /// The natural size travels with the reference so that layout stays a pure
    /// function of the screen. A renderer that had to look the picture up
    /// before it could measure anything would give a different answer
    /// depending on what the runtime happened to be holding, which is exactly
    /// the class of bug that makes a preview stop matching the panel.
    Picture {
        id: NodeId,
        handle: PictureHandle,
        /// The picture's own size, in pixels, as handed over.
        source: (u32, u32),
        /// The tallest this may be drawn, in tenths of a millimetre, so a
        /// portrait picture cannot take a whole panel on one device and a
        /// third of it on another.
        max_height_tenths_mm: u16,
    },
    /// Work in flight, typically a network request.
    ///
    /// This is the supported answer to "show a spinner". A spinner redraws
    /// roughly ten times a second, and on this panel every redraw is a refresh,
    /// so a three second request would cost thirty refreshes and more power
    /// than the request itself. Instead the row states what is happening, in
    /// words, and stays put.
    ///
    /// Carrying cancel here rather than leaving it to the author is deliberate:
    /// a request with no way to abandon it is the most common way an
    /// application ends up feeling stuck on a slow connection.
    Activity {
        id: NodeId,
        label: String,
        /// Present only when the work has a genuine denominator. Progress is
        /// snapped to coarse steps, so a download cannot drive one refresh per
        /// percent.
        progress: Option<Percent>,
        cancel: Option<BarAction>,
    },
    /// A grid of characters, drawn in the fixed-pitch face.
    ///
    /// This is the one node whose text is already laid out when it arrives.
    /// Everywhere else an application says what a thing *is* and the renderer
    /// decides how it looks, because that is what keeps a badly proportioned
    /// screen unexpressible. A character grid is different in kind: column
    /// alignment carries the meaning, so wrapping, truncating or re-flowing it
    /// would destroy the content rather than present it differently.
    ///
    /// The application is still not choosing a font or a colour. It supplies
    /// rows; the renderer owns the face, the size, the ink and the cursor.
    Terminal {
        id: NodeId,
        /// One string per row of the grid, longest first line at the top.
        /// Rows past [`MAX_TERMINAL_ROWS`] and characters past
        /// [`MAX_TERMINAL_COLUMNS`] are dropped rather than wrapped.
        rows: Vec<String>,
        /// Where the block cursor sits, when it is showing.
        cursor: Option<Caret>,
    },
}

/// The position of a terminal's block cursor, in grid cells.
///
/// Cells rather than pixels, for the same reason the grid exists at all: the
/// runtime can then repaint exactly one cell when the cursor moves, which is a
/// refresh of about four square millimetres instead of the whole panel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Caret {
    pub row: u16,
    pub column: u16,
}

impl Caret {
    #[must_use]
    pub const fn new(row: u16, column: u16) -> Self {
        Self { row, column }
    }
}

/// A picture the runtime is holding on the application's behalf.
///
/// Handles are chosen by the application and are private to it, so two
/// applications may use the same number without colliding.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PictureHandle(pub u32);

/// A picture on a tile, together with the size it was handed over at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TilePicture {
    pub handle: PictureHandle,
    pub source: (u32, u32),
}

impl TilePicture {
    #[must_use]
    pub const fn new(handle: PictureHandle, width: u32, height: u32) -> Self {
        Self {
            handle,
            source: (width, height),
        }
    }
}

/// The proportion of a tile's body.
///
/// This is a token rather than a number because a grid whose cells may be any
/// shape is a grid that can be made to look wrong. Square is the destination
/// shape; portrait is the shape of a book, a poster or a cover, and exists so
/// that a shelf of covers reads as a shelf.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TileShape {
    #[default]
    Square,
    Portrait,
}

impl TileShape {
    /// The body's height as a fraction of its width, in eighths.
    #[must_use]
    pub const fn eighths(self) -> i32 {
        match self {
            Self::Square => 8,
            Self::Portrait => 12,
        }
    }

    /// The narrowest a cell of this shape may be, in tenths of a millimetre.
    ///
    /// A physical measurement rather than a pixel count, for the same reason
    /// every other size here is: 25 millimetres is a comfortable icon on any
    /// panel, and 25 pixels is a different thing on each one.
    ///
    /// Portrait was 40 millimetres, which on a six inch panel is two columns,
    /// and two columns of a shape half again as tall as it is wide is a row
    /// and a half of shelf between the bars — the third row was cut in half by
    /// the nav bar, so a shelf of six read as four books and a mistake. Three
    /// columns of 26 millimetres puts two whole rows on the panel. It is a
    /// smaller cover and it is 310 by 465 pixels at 300 pixels per inch, which
    /// is a larger thumbnail than a phone bookshelf shows, so nothing about
    /// recognising a cover is lost by it.
    #[must_use]
    pub const fn minimum_cell_tenth_mm(self) -> i32 {
        match self {
            Self::Square => 250,
            Self::Portrait => 260,
        }
    }
}

/// One tile in a [`Node::TileGrid`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tile {
    pub action: ActionId,
    pub label: String,
    pub glyph: Glyph,
    /// Drawn instead of the glyph when the runtime is holding it. The glyph
    /// stays because a cover that has not arrived yet, or that failed to
    /// decode, must still leave a usable tile rather than a hole.
    pub picture: Option<TilePicture>,
}

impl Tile {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>, glyph: Glyph) -> Self {
        Self {
            action,
            label: label.into(),
            glyph,
            picture: None,
        }
    }

    #[must_use]
    pub fn with_picture(mut self, picture: TilePicture) -> Self {
        self.picture = Some(picture);
        self
    }
}

/// One square of a [`Node::Grid`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub action: ActionId,
    pub label: String,
}

impl Cell {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
        }
    }
}

/// The most cells one grid will lay out.
pub const MAX_CELLS: usize = 64;
/// The most columns a grid may ask for.
pub const MAX_COLUMNS: u8 = 12;

/// One entry in a [`Node::Rows`] list.
///
/// A title identifies, a summary explains and a glyph makes the row findable
/// without reading. The summary is optional because forcing authors to invent
/// one produces filler, and filler is worse than nothing on a small screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub action: ActionId,
    pub title: String,
    pub summary: String,
    pub lead: RowLead,
    pub state: RowState,
}

impl Row {
    #[must_use]
    pub fn new(
        action: ActionId,
        title: impl Into<String>,
        summary: impl Into<String>,
        lead: impl Into<RowLead>,
    ) -> Self {
        Self {
            action,
            title: title.into(),
            summary: summary.into(),
            lead: lead.into(),
            state: RowState::Open,
        }
    }

    /// The same row, marked as finished.
    #[must_use]
    pub fn done(mut self, done: bool) -> Self {
        self.state = if done { RowState::Done } else { RowState::Open };
        self
    }
}

/// What stands at the head of a row.
///
/// An icon makes a row findable without reading it, which is why rows have one
/// at all. But a list where every entry carries the *same* icon has spent a
/// whole touch target's width on decoration: the Hacker News client drew a
/// newspaper beside all thirty stories, which told the eye nothing it did not
/// already know from the fact that it was looking at a list of stories.
///
/// The alternative is not a smaller icon, it is a different fact. Where the
/// entries are ordered, the position *is* the distinguishing information, so
/// the well holds a number instead — the same thing Hacker News itself puts
/// there. `From<Glyph>` exists so that the icon case, which is still the right
/// answer for a menu of unlike things, stays the shortest thing to write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowLead {
    Icon(Glyph),
    /// The row's position in an ordered list, drawn as digits.
    Number(u16),
}

impl From<Glyph> for RowLead {
    fn from(glyph: Glyph) -> Self {
        Self::Icon(glyph)
    }
}

impl From<u16> for RowLead {
    fn from(number: u16) -> Self {
        Self::Number(number)
    }
}

/// Whether what a row names is still outstanding.
///
/// This is a state, not a style. An application says the thing is finished and
/// the renderer decides what finished looks like, which on this panel is muted
/// ink and a line through the title. A crossed-out line is the one case where
/// a line through text carries meaning rather than decoration, which is why it
/// exists here and nowhere else.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RowState {
    #[default]
    Open,
    Done,
}

/// The free-text row that may follow a [`Node::Choice`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Freeform {
    pub action: ActionId,
    pub placeholder: String,
}

impl Freeform {
    #[must_use]
    pub fn new(action: ActionId, placeholder: impl Into<String>) -> Self {
        Self {
            action,
            placeholder: placeholder.into(),
        }
    }
}

/// How loudly a [`Node::Banner`] speaks.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BannerLevel {
    #[default]
    Info,
    /// Drawn inverted. On a panel with two usable tones, inversion is the
    /// loudest signal available, and it costs one small refresh.
    Attention,
}

/// The built-in icon set.
///
/// A closed enum rather than an image, for three reasons: an application cannot
/// ship a low-contrast icon that vanishes on a grayscale panel, icons stay
/// legible at every supported density because they are drawn from geometry
/// rather than scaled from a bitmap, and no decoding of untrusted image data
/// ever happens inside the runtime.
///
/// The artwork lives in [`vector`], in a 1000 unit box, and is rasterised at
/// whatever size the layout asks for.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Glyph {
    #[default]
    App,
    Book,
    Note,
    Clock,
    Settings,
    Folder,
    Chart,
    Search,
    Wifi,
    Battery,
    Reader,
    Power,
    /// Three by three with a nought and a cross: a board, a game, a grid.
    Grid,
    /// An empty ring: something outstanding.
    Circle,
    /// A ring with a tick in it: something finished.
    Check,
    /// A prompt: a chevron and a line waiting to be typed on.
    Terminal,
    /// A speech bubble: a conversation, or a thread of them.
    Chat,
    /// A folded newspaper: stories, a feed, a front page.
    News,
    /// A dot with two arcs radiating from it: the feed mark, as everyone
    /// already reads it.
    Rss,
}

impl Node {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        match self {
            Self::Heading { id, .. }
            | Self::Text { id, .. }
            | Self::Quote { id, .. }
            | Self::Button { id, .. }
            | Self::Card { id, .. }
            | Self::Divider { id }
            | Self::Spacer { id, .. }
            | Self::Progress { id, .. }
            | Self::PagedList { id, .. }
            | Self::Grid { id, .. }
            | Self::Rows { id, .. }
            | Self::TileGrid { id, .. }
            | Self::Choice { id, .. }
            | Self::Banner { id, .. }
            | Self::Skeleton { id, .. }
            | Self::Picture { id, .. }
            | Self::Activity { id, .. }
            | Self::Terminal { id, .. } => *id,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    #[must_use]
    pub const fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }

    #[must_use]
    pub const fn intersection(self, other: Self) -> Option<Self> {
        let left = max_i32(self.x, other.x);
        let top = max_i32(self.y, other.y);
        let right = min_i32(
            self.x.saturating_add(self.width),
            other.x.saturating_add(other.width),
        );
        let bottom = min_i32(
            self.y.saturating_add(self.height),
            other.y.saturating_add(other.height),
        );
        if right > left && bottom > top {
            Some(Self {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            })
        } else {
            None
        }
    }
}

const fn max_i32(a: i32, b: i32) -> i32 {
    if a > b {
        a
    } else {
        b
    }
}
const fn min_i32(a: i32, b: i32) -> i32 {
    if a < b {
        a
    } else {
        b
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutKind {
    Heading,
    Text,
    /// An indented paragraph. The value is the clamped depth, so the renderer
    /// can draw the gutter rule without consulting the tree.
    Quote(u8),
    Button(ActionId, ControlState),
    Card,
    Divider,
    Spacer,
    Progress,
    PagedList,
    TopBar,
    TopBarTitle,
    /// Emitted only by the runtime, never by an application.
    Back,
    BarAction(ActionId),
    NavBar,
    NavDestination(ActionId),
    NavDestinationSelected(ActionId),
    Row(ActionId),
    Cell(ActionId),
    CellLabel,
    RowTitle,
    /// A title whose work is finished: muted and struck through.
    RowTitleDone,
    RowSummary,
    RowLead(RowLead),
    Tile(ActionId),
    TileLabel,
    TileGlyph(Glyph),
    /// A picture, already placed. `rect` is where it goes; the renderer scales
    /// it to fit only if the application handed over something larger.
    Picture(PictureHandle),
    ChoicePrompt,
    ChoiceOption(ActionId, bool),
    ChoiceFreeform(ActionId),
    Banner(BannerLevel),
    Skeleton,
    ActivityLabel,
    ActivityProgress,
    /// A grid of characters. `text_lines` holds one entry per row, already
    /// clipped to the grid.
    TerminalGrid,
    /// One inverted cell. `text_lines` holds the single character underneath
    /// it, so the cursor can be repainted alone without the row it sits in.
    TerminalCursor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutNode {
    pub id: NodeId,
    pub rect: Rect,
    pub kind: LayoutKind,
    pub text_lines: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Layout {
    pub nodes: Vec<LayoutNode>,
    /// The band between the bars, which is what the page-turn zones cover.
    pub content: Rect,
    /// Set when the screen asked for tap-to-turn.
    pub page_turns: Option<PageTurns>,
    /// The face this screen's prose was wrapped in, and must be drawn in.
    ///
    /// Kept on the layout rather than on each node because it is a property of
    /// the screen, and kept at all because measuring and drawing have to agree:
    /// text wrapped in one face and drawn in another does not end where the
    /// wrapping said it would.
    pub prose_face: Face,
}

/// How urgently a screen diagnostic should be treated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

/// A concrete reason a screen will not render as its author intended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayoutIssueKind {
    ContentOverflow {
        hidden_nodes: usize,
    },
    Clipped,
    TouchTargetTooSmall {
        minimum: i32,
    },
    TextOverflow,
    MissingPicture(PictureHandle),
    UnsupportedCharacter {
        character: char,
        face: Face,
    },
    DuplicateNodeId,
    CollectionTruncated {
        collection: &'static str,
        provided: usize,
        visible: usize,
    },
    EmptyChoice,
    InvalidPictureSource,
}

/// One actionable screen diagnostic, optionally tied to a drawn rectangle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutIssue {
    pub severity: DiagnosticSeverity,
    pub node: Option<NodeId>,
    pub kind: LayoutIssueKind,
    pub rect: Option<Rect>,
}

impl std::fmt::Display for LayoutIssue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let node = self
            .node
            .map_or_else(|| "screen".to_owned(), |node| format!("node {}", node.0));
        match &self.kind {
            LayoutIssueKind::ContentOverflow { hidden_nodes } => {
                write!(
                    formatter,
                    "{node}: {hidden_nodes} node(s) are below the content area"
                )
            }
            LayoutIssueKind::Clipped => {
                write!(formatter, "{node}: content is clipped by a panel edge")
            }
            LayoutIssueKind::TouchTargetTooSmall { minimum } => write!(
                formatter,
                "{node}: touch target is smaller than the {minimum}px minimum"
            ),
            LayoutIssueKind::TextOverflow => {
                write!(formatter, "{node}: rendered text exceeds its rectangle")
            }
            LayoutIssueKind::MissingPicture(handle) => {
                write!(
                    formatter,
                    "{node}: picture {} is not in the runtime cache",
                    handle.0
                )
            }
            LayoutIssueKind::UnsupportedCharacter { character, face } => {
                write!(formatter, "{node}: {face:?} face cannot draw {character:?}")
            }
            LayoutIssueKind::DuplicateNodeId => {
                write!(formatter, "{node}: node identifier is used more than once")
            }
            LayoutIssueKind::CollectionTruncated {
                collection,
                provided,
                visible,
            } => write!(
                formatter,
                "{node}: {collection} contains {provided} items but only {visible} are visible"
            ),
            LayoutIssueKind::EmptyChoice => {
                write!(formatter, "{node}: choice has no tappable answers")
            }
            LayoutIssueKind::InvalidPictureSource => {
                write!(formatter, "{node}: picture source has no area")
            }
        }
    }
}

/// Layout plus diagnostics from the same measurement pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LayoutDiagnostics {
    pub layout: Layout,
    pub issues: Vec<LayoutIssue>,
}

impl LayoutDiagnostics {
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == DiagnosticSeverity::Error)
    }
}

impl Layout {
    /// The control a finger is resting on, for drawing it pressed.
    ///
    /// Separate from [`Self::hit_test`] because the two answer different
    /// questions. Hit testing asks what a completed tap should *do*, and
    /// deliberately includes the page-turn zones, which cover half the panel
    /// and have nothing to invert. This asks what the reader is touching, which
    /// must be something with edges they can see change.
    ///
    /// The smallest containing control wins, so a button inside a card inverts
    /// the button.
    #[must_use]
    pub fn pressed_control(&self, x: i32, y: i32) -> Option<Rect> {
        self.nodes
            .iter()
            .filter(|node| node.rect.contains(x, y))
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::Button(_, ControlState::Enabled)
                        | LayoutKind::Back
                        | LayoutKind::BarAction(_)
                        | LayoutKind::NavDestination(_)
                        | LayoutKind::NavDestinationSelected(_)
                        | LayoutKind::Row(_)
                        | LayoutKind::Cell(_)
                        | LayoutKind::Tile(_)
                        | LayoutKind::ChoiceOption(_, _)
                        | LayoutKind::ChoiceFreeform(_)
                )
            })
            .min_by_key(|node| {
                i64::from(node.rect.width.max(0)) * i64::from(node.rect.height.max(0))
            })
            .map(|node| node.rect)
    }

    #[must_use]
    pub fn hit_test(&self, x: i32, y: i32) -> Option<ActionId> {
        // Controls first, always. A page turn is what a tap means when it
        // means nothing else, so a button, a row or a keyboard key can never
        // be shadowed by a zone sitting underneath it.
        if let Some(action) = self.hit_control(x, y) {
            return Some(action);
        }
        // A disabled control is still a control. Falling through here would
        // turn the page under a greyed-out button, which is worse than doing
        // nothing: the reader taps something that cannot act and the screen
        // answers with a different action entirely.
        if self.hit_inert_control(x, y) {
            return None;
        }
        self.hit_page_turn(x, y)
    }

    /// The page turn a tap on empty content means, if any.
    #[must_use]
    pub fn hit_page_turn(&self, x: i32, y: i32) -> Option<ActionId> {
        let turns = self.page_turns?;
        if !self.content.contains(x, y) {
            return None;
        }
        if x < self.content.x + self.content.width / BACK_ZONE {
            Some(turns.previous)
        } else {
            Some(turns.next)
        }
    }

    fn hit_control(&self, x: i32, y: i32) -> Option<ActionId> {
        self.nodes.iter().rev().find_map(|node| match node.kind {
            LayoutKind::Button(action, ControlState::Enabled)
            | LayoutKind::BarAction(action)
            | LayoutKind::NavDestination(action)
            | LayoutKind::NavDestinationSelected(action)
            | LayoutKind::Tile(action)
            | LayoutKind::Row(action)
            | LayoutKind::Cell(action)
            | LayoutKind::ChoiceOption(action, _)
            | LayoutKind::ChoiceFreeform(action)
                if node.rect.contains(x, y) =>
            {
                Some(action)
            }
            LayoutKind::Back if node.rect.contains(x, y) => Some(ActionId::BACK),
            _ => None,
        })
    }

    /// Whether the tap landed on a control that exists but cannot act.
    fn hit_inert_control(&self, x: i32, y: i32) -> bool {
        self.nodes.iter().rev().any(|node| {
            matches!(node.kind, LayoutKind::Button(_, ControlState::Disabled))
                && node.rect.contains(x, y)
        })
    }

    /// The smallest rectangle covering every node, for a targeted refresh.
    #[must_use]
    pub fn bounds(&self) -> Option<Rect> {
        let mut bounds: Option<Rect> = None;
        for node in &self.nodes {
            bounds = Some(match bounds {
                None => node.rect,
                Some(current) => {
                    let x = min(current.x, node.rect.x);
                    let y = min(current.y, node.rect.y);
                    let right = max(current.x + current.width, node.rect.x + node.rect.width);
                    let bottom = max(current.y + current.height, node.rect.y + node.rect.height);
                    Rect {
                        x,
                        y,
                        width: right - x,
                        height: bottom - y,
                    }
                }
            });
        }
        bounds
    }

    /// The rectangle covering a single node, for patching one row rather than
    /// repainting the screen. Selecting an option should cost one small
    /// refresh, not a full flash of the panel.
    #[must_use]
    pub fn rect_of_action(&self, action: ActionId) -> Option<Rect> {
        self.nodes
            .iter()
            .find(|node| match node.kind {
                LayoutKind::Button(candidate, ControlState::Enabled)
                | LayoutKind::BarAction(candidate)
                | LayoutKind::NavDestination(candidate)
                | LayoutKind::NavDestinationSelected(candidate)
                | LayoutKind::Tile(candidate)
                | LayoutKind::ChoiceOption(candidate, _)
                | LayoutKind::Cell(candidate)
                | LayoutKind::ChoiceFreeform(candidate) => candidate == action,
                _ => false,
            })
            .map(|node| node.rect)
    }
}

/// The largest `source` can be drawn inside `max_width` by `max_height` without
/// changing its proportions.
///
/// A picture is never enlarged. Upscaling a small cover to fill a tile turns a
/// sharp thumbnail into a soft one, and on a panel with sixteen greys softness
/// is the one thing that reads as broken.
fn fit_within(source: (u32, u32), max_width: i32, max_height: i32) -> (i32, i32) {
    let max_width = max(0, max_width);
    let max_height = max(0, max_height);
    let width = i32::try_from(source.0).unwrap_or(i32::MAX);
    let height = i32::try_from(source.1).unwrap_or(i32::MAX);
    if width <= 0 || height <= 0 || max_width == 0 || max_height == 0 {
        return (0, 0);
    }
    if width <= max_width && height <= max_height {
        return (width, height);
    }
    let by_width = (
        max_width,
        max(
            1,
            (i64::from(max_width) * i64::from(height) / i64::from(width)) as i32,
        ),
    );
    if by_width.1 <= max_height {
        return by_width;
    }
    (
        max(
            1,
            (i64::from(max_height) * i64::from(width) / i64::from(height)) as i32,
        ),
        max_height,
    )
}

/// The first line of `text`, marked with an ellipsis when there was more.
///
/// A label cut at a word boundary with nothing to show for it reads as a
/// rendering fault rather than as an abbreviation, and under a book cover
/// almost every title is longer than the tile is wide.
///
/// Public because a list of headlines wants it as much as a tile does: a story
/// title that wraps makes its row a different height from the one above, and a
/// list whose rows all differ is one the eye has to re-measure on every line.
#[must_use]
pub fn one_line(text: &str, width: i32, size: FontSize) -> String {
    let lines = wrap_text(text, width, size);
    let mut first = lines.first().cloned().unwrap_or_default();
    // `wrap_text` breaks on an average advance, which is the right trade for
    // paragraphs and the wrong one for a single label: a line of wide letters
    // measures over the estimate and runs out of its tile, which is how "AI
    // Command Center" reached both borders of a cell it was supposed to sit
    // inside. One line can afford to be measured properly.
    if lines.len() <= 1 && measure_text(&first, size).0 <= width {
        return first;
    }
    // Room has to be made for the ellipsis, or the mark itself wraps and is
    // never seen.
    while !first.is_empty() && measure_text(&format!("{first}\u{2026}"), size).0 > width {
        first.pop();
    }
    format!("{}\u{2026}", first.trim_end())
}

/// `text` cut to at most `lines` wrapped lines, ellipsised if it did not fit.
///
/// One line is the tidiest a list can look and, for anything written by
/// somebody else, the least useful: a Hacker News headline averages well over
/// a line on this panel, so a one-line list is a column of sentences that all
/// stop before they have said anything. Two lines carry almost every real
/// headline whole, at the cost of a list whose rows differ in height — which
/// is the right trade, because a row's height is not information and its title
/// is.
///
/// Returns a plain string rather than lines, because the layout engine wraps
/// the title itself and would only have to join them back together.
#[must_use]
pub fn clamp_lines(text: &str, width: i32, size: FontSize, lines: usize) -> String {
    let lines = lines.max(1);
    if lines == 1 {
        return one_line(text, width, size);
    }
    let wrapped = wrap_text(text, width, size);
    if wrapped.len() <= lines {
        return text.trim().to_string();
    }
    let mut kept = wrapped[..lines].join(" ");
    // Take words off the end until the ellipsis fits inside the allowance too,
    // or the mark lands on a line nobody will see.
    while !kept.is_empty()
        && wrap_text(&format!("{}\u{2026}", kept.trim_end()), width, size).len() > lines
    {
        kept.pop();
    }
    format!("{}\u{2026}", kept.trim_end())
}

#[allow(clippy::too_many_arguments)]
fn layout_node(
    node: &Node,
    x: i32,
    y: i32,
    width: i32,
    depth: usize,
    metrics: &DisplayMetrics,
    prose: Face,
    layout: &mut Layout,
) -> i32 {
    if depth > MAX_LAYOUT_DEPTH || layout.nodes.len() >= MAX_LAYOUT_NODES {
        return y;
    }
    let width = max(0, width);
    match node {
        Node::Heading { id, text } => {
            let lines = wrap_text(text, width, FontSize::Heading);
            let height = max(36, lines.len() as i32 * FontSize::Heading.line_height());
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Heading,
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Text { id, text } => {
            let lines = wrap_text_in(text, width, FontSize::Body, prose);
            let height = max(
                MIN_TEXT_HEIGHT,
                lines.len() as i32 * FontSize::Body.line_height_in(prose),
            );
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Text,
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Quote { id, depth, text } => {
            let depth = (*depth).min(MAX_QUOTE_DEPTH);
            let (offset, text_width) = quote_offsets(metrics, width, depth);
            let text_x = x.saturating_add(offset);
            let lines = wrap_text_in(text, text_width, FontSize::Body, prose);
            let height = max(
                MIN_TEXT_HEIGHT,
                lines.len() as i32 * FontSize::Body.line_height_in(prose),
            );
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: text_x,
                    y,
                    width: text_width,
                    height,
                },
                kind: LayoutKind::Quote(depth),
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Button {
            id,
            action,
            label,
            state,
        } => {
            // A control is never smaller than a finger, by construction. The
            // author never gets to choose a height at all.
            let height = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Button(*action, *state),
                text_lines: wrap_text(label, width - 32, FontSize::Body),
            });
            y.saturating_add(height)
        }
        Node::Card { id, children } => {
            let index = layout.nodes.len();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: 0,
                },
                kind: LayoutKind::Card,
                text_lines: Vec::new(),
            });
            let padding = metrics.space(Space::Small);
            let inner_gap = metrics.space(Space::Tight);
            let mut cursor = y.saturating_add(padding);
            for child in children {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                cursor = layout_node(
                    child,
                    x.saturating_add(padding),
                    cursor,
                    width.saturating_sub(2 * padding),
                    depth + 1,
                    metrics,
                    prose,
                    layout,
                )
                .saturating_add(inner_gap);
            }
            let height = max(
                2 * padding,
                cursor.saturating_sub(y).saturating_add(inner_gap),
            );
            layout.nodes[index].rect.height = height;
            y.saturating_add(height)
        }
        Node::Divider { id } => {
            let thickness = metrics.rule_thickness();
            let inset = metrics.space(Space::Tight);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y: y.saturating_add(inset),
                    width,
                    height: thickness,
                },
                kind: LayoutKind::Divider,
                text_lines: Vec::new(),
            });
            y.saturating_add(2 * inset + thickness)
        }
        Node::Spacer { id, space } => {
            let height = metrics.space(*space);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Spacer,
                text_lines: Vec::new(),
            });
            y.saturating_add(height)
        }
        Node::Progress { id, value } => {
            let height = metrics.tenth_mm(20);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Progress,
                text_lines: vec![value.to_string()],
            });
            y.saturating_add(height)
        }
        Node::PagedList { id, page, items } => {
            let per_page = 8_usize;
            let start = usize::from(*page).saturating_mul(per_page);
            let lines = items
                .iter()
                .skip(start)
                .take(per_page)
                .flat_map(|item| wrap_text(item, width, FontSize::Body))
                .collect::<Vec<_>>();
            let height = max(
                MIN_TEXT_HEIGHT,
                lines.len() as i32 * FontSize::Body.line_height(),
            );
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::PagedList,
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Grid {
            id,
            columns,
            square,
            cells,
        } => {
            let columns = i32::from((*columns).clamp(1, MAX_COLUMNS));
            let gutter = metrics.space(Space::Tight);
            let cell_width = (width - gutter * (columns - 1)) / columns;
            // A square cell is what makes a board read as a board. A grid that
            // is not square is a keyboard, and there one row of touch target
            // is exactly right and anything taller wastes the panel.
            let cell_height = if *square {
                cell_width
            } else {
                metrics.touch_target_default()
            };
            let index = layout.nodes.len();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: 0,
                },
                kind: LayoutKind::Spacer,
                text_lines: Vec::new(),
            });
            let mut rows = 0;
            for (position, cell) in cells.iter().take(MAX_CELLS).enumerate() {
                if layout.nodes.len() + 2 > MAX_LAYOUT_NODES {
                    break;
                }
                let position = i32::try_from(position).unwrap_or(0);
                let column = position % columns;
                let row = position / columns;
                rows = row + 1;
                let rect = Rect {
                    x: x.saturating_add(column * (cell_width + gutter)),
                    y: y.saturating_add(row * (cell_height + gutter)),
                    width: cell_width,
                    height: cell_height,
                };
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect,
                    kind: LayoutKind::Cell(cell.action),
                    text_lines: Vec::new(),
                });
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect,
                    kind: LayoutKind::CellLabel,
                    text_lines: vec![cell.label.clone()],
                });
            }
            let height = if rows == 0 {
                0
            } else {
                rows * cell_height + (rows - 1) * gutter
            };
            layout.nodes[index].rect.height = height;
            y.saturating_add(height)
        }
        Node::Rows { id, rows } => {
            let padding = metrics.space(Space::Small);
            let gap = metrics.space(Space::Tight);
            // The glyph column is a touch target's width so that rows line up
            // with every other tappable thing in the system.
            let icon = metrics.touch_target_default();
            let text_x = x.saturating_add(icon).saturating_add(padding);
            let text_width = max(1, width - icon - padding * 2);
            let mut cursor = y;
            for (position, row) in rows.iter().take(MAX_ROWS).enumerate() {
                if layout.nodes.len() + 5 > MAX_LAYOUT_NODES {
                    break;
                }
                // Separators go between rows, never after the last one. A
                // trailing rule collides with whatever the screen puts next and
                // reads as a mistake, which it was.
                if position > 0 {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x,
                            y: cursor,
                            width,
                            height: metrics.rule_thickness(),
                        },
                        kind: LayoutKind::Divider,
                        text_lines: Vec::new(),
                    });
                    cursor = cursor.saturating_add(gap);
                }
                let title_lines = wrap_text(&row.title, text_width, FontSize::Body);
                let summary_lines = if row.summary.is_empty() {
                    Vec::new()
                } else {
                    wrap_text(&row.summary, text_width, FontSize::Caption)
                };
                let title_height = title_lines.len() as i32 * FontSize::Body.line_height();
                let summary_height = summary_lines.len() as i32 * FontSize::Caption.line_height();
                let content = title_height.saturating_add(summary_height);
                // Never shorter than a finger, however terse the entry is.
                let height = max(
                    metrics.touch_target_default(),
                    content.saturating_add(padding * 2),
                );
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::Row(row.action),
                    text_lines: Vec::new(),
                });
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor.saturating_add((height - icon) / 2),
                        width: icon,
                        height: icon,
                    },
                    kind: LayoutKind::RowLead(row.lead),
                    text_lines: Vec::new(),
                });
                let text_y = cursor.saturating_add((height - content) / 2);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: text_x,
                        y: text_y,
                        width: text_width,
                        height: title_height,
                    },
                    kind: match row.state {
                        RowState::Open => LayoutKind::RowTitle,
                        RowState::Done => LayoutKind::RowTitleDone,
                    },
                    text_lines: title_lines,
                });
                if summary_height > 0 {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: text_x,
                            y: text_y.saturating_add(title_height),
                            width: text_width,
                            height: summary_height,
                        },
                        kind: LayoutKind::RowSummary,
                        text_lines: summary_lines,
                    });
                }
                cursor = cursor.saturating_add(height).saturating_add(gap);
            }
            cursor.saturating_sub(gap)
        }
        Node::Picture {
            id,
            handle,
            source,
            max_height_tenths_mm,
        } => {
            let ceiling = metrics.tenth_mm(i32::from(*max_height_tenths_mm));
            let (drawn_width, drawn_height) = fit_within(*source, width, ceiling);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: x + (width - drawn_width) / 2,
                    y,
                    width: drawn_width,
                    height: drawn_height,
                },
                kind: LayoutKind::Picture(*handle),
                text_lines: Vec::new(),
            });
            y.saturating_add(drawn_height)
        }
        Node::TileGrid { id, tiles, shape } => {
            let columns = metrics.grid_columns(*shape) as i32;
            let gutter = metrics.space(Space::Small);
            let cell = (width - gutter * (columns - 1)) / columns;
            // The body, plus a band beneath for the label. A tile shorter than
            // it is wide reads as a button, not a destination.
            let body = cell * shape.eighths() / 8;
            let label_band = FontSize::Caption.line_height() + metrics.space(Space::Tight);
            let cell_height = body.saturating_add(label_band);
            let index = layout.nodes.len();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: 0,
                },
                kind: LayoutKind::Spacer,
                text_lines: Vec::new(),
            });
            let mut rows = 0;
            for (position, tile) in tiles.iter().enumerate() {
                if layout.nodes.len() + 3 > MAX_LAYOUT_NODES {
                    break;
                }
                let column = position as i32 % columns;
                let row = position as i32 / columns;
                rows = row + 1;
                let cell_x = x.saturating_add(column * (cell + gutter));
                let cell_y = y.saturating_add(row * (cell_height + gutter));
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x,
                        y: cell_y,
                        width: cell,
                        height: cell_height,
                    },
                    kind: LayoutKind::Tile(tile.action),
                    text_lines: Vec::new(),
                });
                // Fitted inside the body and centred, so a cover that is not
                // exactly the tile's proportion is letterboxed rather than
                // stretched. A stretched face is worse than a smaller one.
                let (mark, mark_width, mark_height) = if let Some(picture) = tile.picture {
                    let (width, height) = fit_within(picture.source, cell, body);
                    (LayoutKind::Picture(picture.handle), width, height)
                } else {
                    let size = metrics.tenth_mm(110);
                    (LayoutKind::TileGlyph(tile.glyph), size, size)
                };
                let inset = metrics.space(Space::Tight);
                let caption = FontSize::Caption.line_height();
                // Mark and name are one object, centred together, rather than
                // a mark centred in the body with the name pinned to the cell's
                // bottom edge. Those are the same thing only when the mark
                // fills the body: a glyph is barely a third of it, so the name
                // ended up stranded a finger's width below its own icon and
                // hard against the tile's rule. Every phone home screen sets an
                // icon and its label as a pair for the same reason.
                let group = mark_height
                    .saturating_add(inset)
                    .saturating_add(caption)
                    .min(cell_height);
                let group_y = cell_y.saturating_add((cell_height - group) / 2);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x + (cell - mark_width) / 2,
                        y: group_y,
                        width: mark_width,
                        height: mark_height,
                    },
                    kind: mark,
                    text_lines: Vec::new(),
                });
                // Inset by the same tight step the label sits below the mark,
                // so a name that fills its tile is ellipsised with a margin
                // rather than run flush into the cell border.
                let label_width = max_i32(1, cell - inset * 2);
                let label = one_line(&tile.label, label_width, FontSize::Caption);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x + inset,
                        y: group_y.saturating_add(mark_height).saturating_add(inset),
                        width: label_width,
                        height: caption,
                    },
                    kind: LayoutKind::TileLabel,
                    text_lines: vec![label],
                });
            }
            let height = if rows == 0 {
                0
            } else {
                rows * cell_height + (rows - 1) * gutter
            };
            layout.nodes[index].rect.height = height;
            y.saturating_add(height)
        }
        Node::Choice {
            id,
            prompt,
            options,
            selected,
            freeform,
        } => {
            let gap = metrics.space(Space::Tight);
            let row_height = metrics.touch_target_default();
            let mut cursor = y;
            if !prompt.is_empty() {
                let lines = wrap_text(prompt, width, FontSize::Body);
                let height = lines.len() as i32 * FontSize::Body.line_height();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::ChoicePrompt,
                    text_lines: lines,
                });
                cursor = cursor
                    .saturating_add(height)
                    .saturating_add(metrics.space(Space::Small));
            }
            for (index, option) in options.iter().take(MAX_CHOICE_OPTIONS).enumerate() {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                let chosen = selected.is_some_and(|selected| usize::from(selected) == index);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height: row_height,
                    },
                    kind: LayoutKind::ChoiceOption(option.action, chosen),
                    text_lines: vec![option.label.clone()],
                });
                cursor = cursor.saturating_add(row_height).saturating_add(gap);
            }
            if let Some(freeform) = freeform {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height: row_height,
                    },
                    kind: LayoutKind::ChoiceFreeform(freeform.action),
                    text_lines: vec![freeform.placeholder.clone()],
                });
                cursor = cursor.saturating_add(row_height).saturating_add(gap);
            }
            cursor.saturating_sub(gap)
        }
        Node::Banner { id, level, text } => {
            let padding = metrics.space(Space::Small);
            let lines = wrap_text(text, width - 2 * padding, FontSize::Body);
            let height = (lines.len() as i32 * FontSize::Body.line_height())
                .saturating_add(2 * padding)
                .max(metrics.touch_target_minimum());
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Banner(*level),
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Skeleton { id, lines } => {
            let count = i32::from((*lines).clamp(1, 12));
            let height = count * FontSize::Body.line_height();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Skeleton,
                text_lines: vec![count.to_string()],
            });
            y.saturating_add(height)
        }
        Node::Activity {
            id,
            label,
            progress,
            cancel,
        } => {
            let gap = metrics.space(Space::Small);
            let mut cursor = y;
            let lines = wrap_text(label, width, FontSize::Body);
            let label_height = lines.len() as i32 * FontSize::Body.line_height();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y: cursor,
                    width,
                    height: label_height,
                },
                kind: LayoutKind::ActivityLabel,
                text_lines: lines,
            });
            cursor = cursor.saturating_add(label_height).saturating_add(gap);
            if let Some(progress) = progress {
                let height = metrics.tenth_mm(20);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::ActivityProgress,
                    text_lines: vec![progress.coarse().to_string()],
                });
                cursor = cursor.saturating_add(height).saturating_add(gap);
            }
            if let Some(cancel) = cancel {
                let height = metrics.touch_target_default();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::ChoiceFreeform(cancel.action),
                    text_lines: vec![cancel.label.clone()],
                });
                cursor = cursor.saturating_add(height).saturating_add(gap);
            }
            cursor.saturating_sub(gap)
        }
        Node::Terminal { id, rows, cursor } => {
            let (cell_width, cell_height) = mono_cell(TERMINAL_SIZE);
            let columns = (width / max(1, cell_width)).clamp(0, MAX_TERMINAL_COLUMNS as i32);
            let lines: Vec<String> = rows
                .iter()
                .take(MAX_TERMINAL_ROWS)
                // Clipped, never wrapped. A row that overflowed onto the next
                // one would shift every row below it and the grid would stop
                // being a grid.
                .map(|row| row.chars().take(columns as usize).collect())
                .collect();
            let height = lines.len() as i32 * cell_height;
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width: columns * cell_width,
                    height,
                },
                kind: LayoutKind::TerminalGrid,
                text_lines: lines.clone(),
            });
            if let Some(caret) = cursor {
                let row = i32::from(caret.row);
                let column = i32::from(caret.column);
                if row < lines.len() as i32 && column < columns {
                    // The character underneath travels with the cursor so the
                    // cell can be repainted on its own: a cursor that needed
                    // its row redrawn would cost a refresh the width of the
                    // panel every time it moved one place.
                    let under = lines
                        .get(row as usize)
                        .and_then(|line| line.chars().nth(column as usize))
                        .unwrap_or(' ');
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(column * cell_width),
                            y: y.saturating_add(row * cell_height),
                            width: cell_width,
                            height: cell_height,
                        },
                        kind: LayoutKind::TerminalCursor,
                        text_lines: vec![under.to_string()],
                    });
                }
            }
            y.saturating_add(height)
        }
    }
}

/// The character grid a terminal on `screen` will actually be given.
///
/// An application feeding a pseudo-terminal has to know the grid *before* it
/// has any output to put in it, and it must be the same grid the panel will
/// show, or the shell wraps its lines in one place and the reader sees them
/// wrap in another. So the screen is laid out with an empty terminal and the
/// space left over is measured: the answer comes from the layout engine itself
/// rather than from an application's arithmetic about bars and keyboards.
///
/// Returns `(0, 0)` for a screen with no terminal on it.
#[must_use]
pub fn terminal_grid_for(screen: &Screen, metrics: &DisplayMetrics) -> (u16, u16) {
    let layout = screen.layout_for(metrics);
    let content = layout.content;
    let Some(terminal) = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::TerminalGrid)
    else {
        return (0, 0);
    };
    let bottom = content.y.saturating_add(content.height);
    let used = layout
        .nodes
        .iter()
        .filter(|node| node.rect.y >= content.y && node.rect.y < bottom)
        .map(|node| node.rect.y.saturating_add(node.rect.height))
        .max()
        .unwrap_or(content.y);
    terminal_grid(terminal.rect.width, bottom.saturating_sub(used))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontSize {
    Caption,
    Body,
    Title,
    Heading,
}

impl FontSize {
    /// The em size in tenths of a millimetre.
    ///
    /// Type is specified physically for the same reason every other measurement
    /// in this layer is: Kobo panels run from about 212 to 300 pixels per inch,
    /// so a pixel size would be a different physical size on every model. Body
    /// text is 3.6 mm, which is close to a printed paperback.
    #[must_use]
    pub const fn tenth_mm(self) -> i32 {
        match self {
            Self::Caption => 28,
            Self::Body => 36,
            Self::Title => 52,
            Self::Heading => 68,
        }
    }

    /// The legacy bitmap scale factor, used only by the built-in fallback.
    #[must_use]
    pub const fn scale(self) -> i32 {
        match self {
            Self::Caption => 2,
            Self::Body => 3,
            Self::Title => 4,
            Self::Heading => 5,
        }
    }

    /// The baseline-to-baseline distance in pixels.
    ///
    /// Layout and rendering must agree on this or text overlaps, so both go
    /// through here rather than each deciding for itself. It follows the
    /// installed typeface, which is why it cannot be `const`.
    #[must_use]
    pub fn line_height(self) -> i32 {
        self.line_height_in(Face::Text)
    }

    /// The baseline-to-baseline distance in pixels for one face.
    ///
    /// The two faces do not share a line height. A monospace face is typically
    /// taller for the same em, and a terminal that used the proportional line
    /// height would overlap its own rows.
    #[must_use]
    pub fn line_height_in(self, face: Face) -> i32 {
        TYPESETTER.get().map_or_else(
            || self.fallback_line_height(),
            |t| t.line_height(self, face),
        )
    }

    /// The built-in bitmap's line height.
    #[must_use]
    pub const fn fallback_line_height(self) -> i32 {
        match self {
            Self::Caption => 18,
            Self::Body => 27,
            Self::Title => 36,
            Self::Heading => 45,
        }
    }
}

/// Which of the two system faces a run of text is set in.
///
/// This is an axis, not a font name. An application says what a piece of text
/// *is*, never which file to open, for the same reason it names a [`FontSize`]
/// rather than a pixel count: the runtime owns the answer and can change it for
/// a different panel without touching a line of application code.
///
/// A weight axis is still deliberately absent. It would multiply the faces the
/// runtime has to find, and on a panel with two usable tones bold buys far less
/// separation than size and space already do.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Face {
    /// Proportional. Everything a reader reads.
    #[default]
    Text,
    /// Fixed pitch, where column alignment carries meaning: a terminal, a hash,
    /// a file size. Every glyph has the same advance, so `n` characters are
    /// always exactly `n` cells wide.
    Mono,
    /// A book, read for an hour at a time.
    ///
    /// Distinct from [`Self::Text`] because the two jobs genuinely differ. The
    /// interface face is chosen so that a label glanced at once cannot be
    /// misread — that is why it is Atkinson Hyperlegible, drawn by the Braille
    /// Institute to keep similar letterforms apart. Prose is the opposite
    /// problem: nothing is glanced at, everything is read in sequence, and what
    /// matters is that the eye is carried along the line without noticing the
    /// type at all. Every dedicated reader answers that with a serif, and this
    /// one does too.
    Reading,
}

/// Supplies real type to the layout and the renderer.
///
/// This layer knows what a heading is; it does not know what a font file is.
/// The runtime installs one implementation at startup, which is why the
/// application-facing size is a semantic name rather than a pixel count and
/// why replacing the typeface changes no application code at all.
pub trait Typesetter: Send + Sync {
    /// The width and height in pixels that `text` will occupy.
    fn measure(&self, text: &str, size: FontSize, face: Face) -> (i32, i32);
    /// The baseline-to-baseline distance for a size.
    fn line_height(&self, size: FontSize, face: Face) -> i32;
    /// Draws `text` with its top-left corner at `x`, `y`.
    ///
    /// Coverage runs from 0 for untouched to 255 for solid, so a renderer can
    /// antialias against whatever it is drawing onto.
    fn draw(
        &self,
        text: &str,
        x: i32,
        y: i32,
        size: FontSize,
        face: Face,
        plot: &mut dyn FnMut(i32, i32, u8),
    );

    /// Whether this face can actually draw `character`.
    ///
    /// The default is `true`, meaning "cannot tell": a typesetter that does not
    /// know its own coverage must not cause working text to be reported as
    /// undrawable.
    fn has_glyph(&self, _character: char, _face: Face) -> bool {
        true
    }

    /// Unicode byte offsets after which a line may or must end.
    fn line_breaks(&self, text: &str) -> Vec<(usize, BreakOpportunity)> {
        fallback_line_breaks(text)
    }

    /// Byte offsets at the end of each user-perceived character.
    ///
    /// Used when one unbroken token is wider than the line, so combining
    /// sequences and emoji remain intact while wrapping still makes progress.
    fn grapheme_boundaries(&self, text: &str) -> Vec<usize> {
        text.char_indices()
            .map(|(offset, character)| offset + character.len_utf8())
            .collect()
    }

    /// The advance of a single [`Face::Mono`] cell.
    ///
    /// A grid of characters cannot be laid out by measuring strings: a terminal
    /// has to know the cell before it knows what will be in it, and every cell
    /// must land on the same column whatever it holds. Asking the face once is
    /// also what lets a partial repaint address one cell rather than one line.
    fn cell_width(&self, size: FontSize) -> i32 {
        let (width, _) = self.measure("0", size, Face::Mono);
        max(1, width)
    }
}

/// Whether a Unicode line-break position is optional or compulsory.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BreakOpportunity {
    Allowed,
    Mandatory,
}

/// The one typeface for the device, installed once by the runtime.
///
/// A global is the honest model here: there is exactly one UI typeface, chosen
/// by the runtime and never by an application, in the same way a phone has one
/// system font. Keeping it out of every layout signature is what allows real
/// type to arrive without touching a single application or example.
static TYPESETTER: OnceLock<Box<dyn Typesetter>> = OnceLock::new();

/// Installs the typeface the runtime has chosen.
///
/// # Errors
///
/// Returns the argument back if a typeface was already installed. Swapping one
/// mid-run would change the size of text that has already been laid out.
pub fn install_typesetter(typesetter: Box<dyn Typesetter>) -> Result<(), Box<dyn Typesetter>> {
    TYPESETTER.set(typesetter)
}

/// Returns whether real type is in use rather than the built-in fallback.
#[must_use]
pub fn has_typesetter() -> bool {
    TYPESETTER.get().is_some()
}

/// Returns integer pixel dimensions for the installed typeface.
///
/// Falls back to the built-in bitmap when no typeface is installed, so a
/// simulator or a test still renders something legible-shaped and layout stays
/// deterministic.
#[must_use]
pub fn measure_text(text: &str, size: FontSize) -> (i32, i32) {
    measure_text_in(text, size, Face::Text)
}

/// Returns integer pixel dimensions for one face of the installed typeface.
#[must_use]
pub fn measure_text_in(text: &str, size: FontSize, face: Face) -> (i32, i32) {
    if let Some(typesetter) = TYPESETTER.get() {
        return typesetter.measure(text, size, face);
    }
    let scale = size.scale();
    let glyphs = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
    (glyphs.saturating_mul(6).saturating_mul(scale), 7 * scale)
}

/// The first character of `text` the installed face cannot draw, if any.
///
/// A character with no glyph is drawn as an empty box, which on a panel reads
/// as a rendering fault rather than as a missing character. This is what lets
/// an application's own tests refuse a label carrying a symbol the face never
/// had, instead of finding out by looking at hardware.
///
/// Returns `None` when no typeface is installed, because the built-in bitmap
/// fallback is not what anything is ultimately drawn with and answering from it
/// would condemn perfectly good text.
#[must_use]
pub fn undrawable_in(text: &str, face: Face) -> Option<char> {
    let typesetter = TYPESETTER.get()?;
    text.chars()
        .find(|character| !character.is_whitespace() && !typesetter.has_glyph(*character, face))
}

/// The size of one monospace cell: what a character grid is built from.
///
/// Returns width and height together because a caller that needs one always
/// needs the other, and taking them from a single call means a grid can never
/// be sized from two different answers.
#[must_use]
pub fn mono_cell(size: FontSize) -> (i32, i32) {
    TYPESETTER.get().map_or_else(
        || (6 * size.scale(), size.fallback_line_height()),
        |typesetter| {
            (
                max(1, typesetter.cell_width(size)),
                max(1, typesetter.line_height(size, Face::Mono)),
            )
        },
    )
}

fn fallback_line_breaks(text: &str) -> Vec<(usize, BreakOpportunity)> {
    let mut breaks = Vec::new();
    let mut previous = None;
    for (offset, character) in text.char_indices() {
        let end = offset + character.len_utf8();
        let opportunity = if is_line_separator(character) {
            // A carriage return and the line feed after it are one separator,
            // not two. Breaking after both leaves an empty line between every
            // pair of lines, and text arriving over a network is full of them.
            if character == '\n' && previous == Some('\r') {
                breaks.pop();
            }
            Some(BreakOpportunity::Mandatory)
        } else if character.is_whitespace() || is_cjk(character) {
            Some(BreakOpportunity::Allowed)
        } else {
            None
        };
        if let Some(opportunity) = opportunity {
            breaks.push((end, opportunity));
        }
        previous = Some(character);
    }
    if breaks.last().map(|entry| entry.0) != Some(text.len()) {
        breaks.push((text.len(), BreakOpportunity::Mandatory));
    } else if let Some(last) = breaks.last_mut() {
        last.1 = BreakOpportunity::Mandatory;
    }
    breaks
}

const fn is_cjk(character: char) -> bool {
    matches!(
        character as u32,
        0x3040..=0x30ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7af
            | 0xf900..=0xfaff
    )
}

/// The least vertical space a block of body text occupies.
///
/// Pagination and layout must agree on this or a page that measured as full
/// draws past the bottom of the panel, so both read it from here.
pub const MIN_TEXT_HEIGHT: i32 = 24;

/// The panel area a screen has left for prose.
///
/// Layout stops at the bottom of the content area and silently drops whatever
/// does not fit, which is the right behaviour for a screen that is slightly
/// too long and the wrong one for a book: a page that overflows loses its last
/// paragraph with nothing on the panel to say so. Measuring the area first is
/// how a reader breaks pages where the panel actually ends.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProseArea {
    pub width: i32,
    pub height: i32,
    /// The space the layout leaves between two adjacent nodes.
    pub gap: i32,
    /// The face this prose will be set in, which decides both how wide the
    /// words come out and how far apart the lines sit.
    pub face: Face,
}

impl DisplayMetrics {
    /// The pixel size of one tile's body on this panel.
    ///
    /// An application needs this to prepare a picture at the size it will
    /// actually be drawn. Guessing is the alternative, and a guess is wrong on
    /// every other model: the column count comes from the panel's physical
    /// width, so the same shelf has different cells on a Clara and an Elipsa.
    #[must_use]
    pub fn tile_body(&self, shape: TileShape) -> (i32, i32) {
        let columns = self.grid_columns(shape) as i32;
        let gutter = self.space(Space::Small);
        let width = max_i32(
            0,
            (self.width - 2 * self.screen_margin() - gutter * (columns - 1)) / columns,
        );
        (width, width * shape.eighths() / 8)
    }

    /// The area between the bars that body text may occupy.
    ///
    /// `top_bar` and `nav_bar` describe the screen the text will be shown on,
    /// because both are chrome the layout reserves rather than content the
    /// text can flow through.
    #[must_use]
    pub fn prose_area(&self, top_bar: bool, nav_bar: bool) -> ProseArea {
        let margin = self.screen_margin();
        let gap = self.space(Space::Tight);
        let mut top = margin;
        if top_bar {
            top = self.top_bar_height() + self.rule_thickness() + gap;
        }
        let bottom = if nav_bar {
            self.height - self.nav_bar_height()
        } else {
            self.height - margin
        };
        ProseArea {
            width: max_i32(0, self.width - 2 * margin),
            height: max_i32(0, bottom - top),
            gap,
            face: Face::Text,
        }
    }

    /// The same area, to be set in a named face.
    ///
    /// A serif sets the same words wider and on more generous lines, so a page
    /// measured in the interface face and drawn in the reading one loses its
    /// last lines off the bottom.
    #[must_use]
    pub fn prose_area_in(&self, top_bar: bool, nav_bar: bool, face: Face) -> ProseArea {
        ProseArea {
            face,
            ..self.prose_area(top_bar, nav_bar)
        }
    }
}

/// Breaks prose into pages that fit, keeping paragraphs whole where it can.
///
/// Each page is a list of paragraphs, in the order they should be emitted as
/// separate text nodes: wrapping works on words and cannot see where one
/// paragraph ended and the next began, so a book emitted as a single node
/// loses every blank line in it.
///
/// Heights come from the same wrapping and line height the layout engine uses,
/// so this agrees with what will be drawn rather than estimating it. A
/// character budget cannot: a page of dialogue is mostly short paragraphs and
/// their gaps, and holds barely half the text of a page of description.
#[must_use]
pub fn paginate(text: &str, area: ProseArea) -> Vec<Vec<String>> {
    // Line endings are normalised first. Project Gutenberg serves CRLF, so a
    // split on "\n\n" alone never matched and an entire novel arrived as one
    // paragraph: a solid wall of text with no space anywhere in it. A lone CR
    // is folded too, because some of the older files use it.
    let text = normalise_breaks(text);
    let paragraphs = text
        .split("\n\n")
        .map(|paragraph| (0, paragraph))
        .collect::<Vec<_>>();
    // The metrics only ever reach `quote_offsets`, and at depth zero that
    // returns the full width whatever panel this is, so an unindented page is
    // measured identically on every device.
    paginate_quoted(&paragraphs, &DisplayMetrics::default(), area)
        .into_iter()
        .map(|page| page.into_iter().map(|(_, text)| text).collect())
        .collect()
}

/// The fewest lines of a paragraph worth leaving alone on a page.
///
/// Two, which is the ordinary typesetting rule for widows and orphans. One
/// line of a paragraph by itself at the foot or the head of a page reads as
/// something having gone wrong rather than as prose continuing.
const MIN_KEEP_LINES: usize = 2;

/// Breaks indented prose into pages that fit, keeping each paragraph's depth.
///
/// The companion to [`paginate`] for threaded discussion. Depth cannot be
/// applied afterwards: an indented paragraph has a narrower measure, so it
/// wraps to more lines and takes more of the page. A thread paginated flat and
/// then drawn indented loses the bottom of every page.
#[must_use]
pub fn paginate_quoted(
    paragraphs: &[(u8, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<(u8, String)>> {
    let line_height = FontSize::Body.line_height_in(area.face);
    let mut pages: Vec<Vec<(u8, String)>> = Vec::new();
    let mut page: Vec<(u8, String)> = Vec::new();
    let mut used = 0;
    if area.width <= 0 || area.height < line_height {
        return pages;
    }

    for (depth, paragraph) in paragraphs {
        let depth = (*depth).min(MAX_QUOTE_DEPTH);
        let (_, width) = quote_offsets(metrics, area.width, depth);
        // Line breaks inside a paragraph are the source file's, not the
        // author's; Gutenberg's plain text is hard wrapped at seventy columns
        // and honouring that would give a column of ragged short lines.
        let paragraph = paragraph.replace('\n', " ");
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        let mut lines = wrap_text_in(paragraph, width, FontSize::Body, area.face);
        while !lines.is_empty() {
            let spacing = if page.is_empty() { 0 } else { area.gap };
            let room = area.height - used - spacing;
            let fits = max_i32(0, room / line_height) as usize;
            if fits == 0 {
                // Nothing more will fit, so start a page rather than draw off
                // the bottom of the panel.
                pages.push(std::mem::take(&mut page));
                used = 0;
                continue;
            }
            if fits >= lines.len() {
                // The layout engine gives every text node a floor, so a
                // one-line paragraph can occupy more than one line's height.
                used += spacing + max_i32(MIN_TEXT_HEIGHT, lines.len() as i32 * line_height);
                page.push((depth, lines.join(" ")));
                break;
            }
            // The paragraph does not fit in what is left. Splitting it at a
            // line boundary is what a book does; moving it whole to the next
            // page is what this used to do, and on a threaded discussion —
            // where a single comment is a single paragraph and often a long
            // one — it left page after page half empty, with the reader
            // turning twice as often to read the same words.
            //
            // The one thing worth protecting is the orphan: a lone line
            // stranded at the foot of a page, or carried alone to the top of
            // the next, reads as a mistake. So the split has to leave at least
            // `MIN_KEEP_LINES` on both sides, and where it cannot the whole
            // paragraph moves on as before.
            let keep = fits.min(lines.len().saturating_sub(MIN_KEEP_LINES));
            // A paragraph longer than an entire page cannot be kept whole at
            // any cost: a book whose preface is one enormous block would
            // otherwise open at chapter two.
            let keep = if page.is_empty() { keep.max(1) } else { keep };
            if keep >= MIN_KEEP_LINES || (page.is_empty() && keep > 0) {
                let rest = lines.split_off(keep);
                page.push((depth, lines.join(" ")));
                pages.push(std::mem::take(&mut page));
                used = 0;
                lines = rest;
            } else {
                pages.push(std::mem::take(&mut page));
                used = 0;
            }
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// Folds every line-ending convention onto `\n`.
///
/// Text arrives from servers, not from this repository, and a paragraph break
/// that only matches one of the three conventions is a paragraph break that
/// usually does not match. Project Gutenberg serves CRLF: without this, an
/// entire novel paginated as a single paragraph and rendered as a solid wall
/// of words with no space anywhere in it.
#[must_use]
pub fn normalise_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Where a quote at `depth` starts and how wide its words are, given the
/// column it sits in.
///
/// One function, used by the layout engine and by the paginator, because a
/// paginator that measured a different width from the one that gets drawn is
/// a paginator that silently drops the last line of a page.
#[must_use]
pub fn quote_offsets(metrics: &DisplayMetrics, width: i32, depth: u8) -> (i32, i32) {
    let depth = depth.min(MAX_QUOTE_DEPTH);
    let step = metrics.space(Space::Small);
    let indent = i32::from(depth) * step;
    // The gutter holds the rule that says "this answers something".
    let gutter = if depth == 0 { 0 } else { step };
    (indent + gutter, max(1, width - indent - gutter))
}

/// How wide the words in a list row actually are, once the icon is paid for.
///
/// Exposed because an application that wants a uniform row height has to
/// ellipsise its titles itself, and it can only do that against the same
/// measure the layout engine uses. Deriving it a second time by hand is how a
/// list ends up with one row in ten wrapping anyway.
#[must_use]
pub fn row_text_width(metrics: &DisplayMetrics, area: ProseArea) -> i32 {
    let padding = metrics.space(Space::Small);
    let icon = metrics.touch_target_default();
    max(1, area.width - icon - padding * 2)
}

/// Breaks a list of rows into pages that fit, returning the row indices on each.
///
/// There is no scrolling anywhere in this UI and there should not be: a panel
/// that takes the better part of a second to repaint cannot follow a finger,
/// and a partial refresh chasing a moving list is exactly the operation that
/// leaves ghosting behind. A page turn is one refresh with a stable result,
/// which is also what a book does.
///
/// So a list longer than the panel is paged rather than cut off, and this is
/// how an application finds out where the folds are. Heights come from the
/// same wrapping and spacing the layout engine uses, so a page that fits here
/// is a page that will be drawn whole.
#[must_use]
pub fn paginate_rows(
    rows: &[(&str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    let padding = metrics.space(Space::Small);
    let icon = metrics.touch_target_default();
    let text_width = row_text_width(metrics, area);
    let separator = metrics.rule_thickness() + area.gap;
    let mut pages: Vec<Vec<usize>> = Vec::new();
    let mut page: Vec<usize> = Vec::new();
    let mut used = 0;

    for (index, (title, summary)) in rows.iter().enumerate() {
        let title_height = wrap_text(title, text_width, FontSize::Body).len() as i32
            * FontSize::Body.line_height();
        let summary_height = if summary.is_empty() {
            0
        } else {
            wrap_text(summary, text_width, FontSize::Caption).len() as i32
                * FontSize::Caption.line_height()
        };
        // Never shorter than a finger, however terse the entry is: the same
        // floor the layout engine applies.
        let height = max(icon, title_height + summary_height + padding * 2);
        let spacing = if page.is_empty() { 0 } else { separator };
        if !page.is_empty() && used + spacing + height > area.height {
            pages.push(std::mem::take(&mut page));
            used = 0;
        }
        let spacing = if page.is_empty() { 0 } else { separator };
        used += spacing + height;
        page.push(index);
        // A single row taller than the whole area still gets a page of its
        // own, because the alternative is an entry that can never be reached.
        if used > area.height && page.len() == 1 {
            pages.push(std::mem::take(&mut page));
            used = 0;
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// Breaks a grid of tiles into pages that fit, returning the tile indices on each.
///
/// The companion to [`paginate_rows`] for the tile shape. The arithmetic is
/// the layout engine's own: an application that guessed a tile count would be
/// right on one panel and wrong on every other, and being wrong here does not
/// look like a layout bug — the layout engine drops what does not fit in
/// silence, so the last few entries simply cease to exist.
#[must_use]
pub fn paginate_tiles(
    count: usize,
    metrics: &DisplayMetrics,
    shape: TileShape,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    let columns = max(1, metrics.grid_columns(shape));
    let gutter = metrics.space(Space::Small);
    let cell = (area.width - gutter * (columns as i32 - 1)) / columns as i32;
    let body = cell * shape.eighths() / 8;
    let label_band = FontSize::Caption.line_height() + metrics.space(Space::Tight);
    let cell_height = max(1, body.saturating_add(label_band));
    // At least one row, however short the area is. A page that holds nothing
    // is a catalogue that can never be read.
    let rows = max(1, (area.height + gutter) / (cell_height + gutter));
    let per_page = columns * rows as usize;
    if count == 0 {
        return vec![Vec::new()];
    }
    (0..count)
        .collect::<Vec<_>>()
        .chunks(per_page)
        .map(<[usize]>::to_vec)
        .collect()
}

/// Wraps at Unicode line-break opportunities using the typeface's exact width.
///
/// Exceptionally long tokens are split at grapheme boundaries. A proportional
/// run of `W`s therefore cannot overflow while a run of `i`s wastes half a
/// line, and combining marks are never detached from their base character.
#[must_use]
pub fn wrap_text(text: &str, max_width: i32, size: FontSize) -> Vec<String> {
    wrap_text_in(text, max_width, size, Face::Text)
}

/// Breaks `text` to `max_width`, measured in the face it will be drawn in.
///
/// The face is not decoration here. A serif and a sans of the same size set the
/// same words to different widths, so wrapping against one and drawing in the
/// other puts lines past the margin and loses the end of a page.
#[must_use]
pub fn wrap_text_in(text: &str, max_width: i32, size: FontSize, face: Face) -> Vec<String> {
    if text.is_empty() || max_width <= 0 {
        return vec![String::new()];
    }
    let opportunities = TYPESETTER
        .get()
        .map_or_else(|| fallback_line_breaks(text), |face| face.line_breaks(text));
    let graphemes = TYPESETTER.get().map_or_else(
        || {
            text.char_indices()
                .map(|(offset, character)| offset + character.len_utf8())
                .collect()
        },
        |face| face.grapheme_boundaries(text),
    );
    let mut lines = Vec::new();
    let mut start = 0;
    while start < text.len() {
        start = skip_soft_whitespace(text, start);
        if start == text.len() {
            break;
        }

        let mut best = None;
        let mut emitted = false;
        for &(end, opportunity) in opportunities.iter().filter(|entry| entry.0 > start) {
            let visible_end = if opportunity == BreakOpportunity::Mandatory {
                trim_line_separator(text, start, end)
            } else {
                end
            };
            let candidate = text[start..visible_end].trim_end_matches(char::is_whitespace);
            if measure_text_in(candidate, size, face).0 <= max_width {
                best = Some((end, candidate.to_owned()));
                if opportunity == BreakOpportunity::Mandatory {
                    lines.push(candidate.to_owned());
                    start = end;
                    emitted = true;
                    break;
                }
                continue;
            }

            if let Some((best_end, line)) = best.take() {
                lines.push(line);
                start = best_end;
            } else {
                let forced_end =
                    force_grapheme_break(text, start, visible_end, max_width, size, &graphemes);
                lines.push(text[start..forced_end].trim_end().to_owned());
                start = forced_end;
            }
            emitted = true;
            break;
        }

        if !emitted {
            let forced_end =
                force_grapheme_break(text, start, text.len(), max_width, size, &graphemes);
            lines.push(text[start..forced_end].trim_end().to_owned());
            start = forced_end;
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn skip_soft_whitespace(text: &str, mut offset: usize) -> usize {
    while let Some(character) = text[offset..].chars().next() {
        if !character.is_whitespace() || is_line_separator(character) {
            break;
        }
        offset += character.len_utf8();
        if offset == text.len() {
            break;
        }
    }
    offset
}

fn trim_line_separator(text: &str, start: usize, mut end: usize) -> usize {
    while end > start {
        let Some((offset, character)) = text[start..end].char_indices().last() else {
            break;
        };
        if !is_line_separator(character) {
            break;
        }
        end = start + offset;
    }
    end
}

const fn is_line_separator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn force_grapheme_break(
    text: &str,
    start: usize,
    limit: usize,
    max_width: i32,
    size: FontSize,
    graphemes: &[usize],
) -> usize {
    let mut first = None;
    let mut best = None;
    for &end in graphemes.iter().filter(|&&end| end > start && end <= limit) {
        first.get_or_insert(end);
        if measure_text(&text[start..end], size).0 <= max_width {
            best = Some(end);
        } else {
            break;
        }
    }
    best.or(first).unwrap_or_else(|| {
        text[start..]
            .chars()
            .next()
            .map_or(text.len(), |character| start + character.len_utf8())
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Surface {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Surface {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![tone::PAPER; width.saturating_mul(height)],
        }
    }

    pub fn clear(&mut self, value: u8) {
        self.pixels.fill(value);
    }

    pub fn fill_rect(&mut self, rect: Rect, value: u8) {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: i32::try_from(self.width).unwrap_or(i32::MAX),
            height: i32::try_from(self.height).unwrap_or(i32::MAX),
        };
        if let Some(clipped) = rect.intersection(bounds) {
            for y in clipped.y..clipped.y + clipped.height {
                let row = usize::try_from(y).unwrap_or(0).saturating_mul(self.width);
                for x in clipped.x..clipped.x + clipped.width {
                    let index = row.saturating_add(usize::try_from(x).unwrap_or(0));
                    if let Some(pixel) = self.pixels.get_mut(index) {
                        *pixel = value;
                    }
                }
            }
        }
    }

    /// Turns every pixel in `rect` to its opposite tone.
    ///
    /// This is what a control being touched looks like. It is done to the
    /// finished surface rather than by drawing the control differently, so it
    /// costs nothing to lay out, applies to every kind of control including the
    /// ones drawn from vectors, and reverses exactly by being done again.
    pub fn invert_rect(&mut self, rect: Rect) {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: i32::try_from(self.width).unwrap_or(i32::MAX),
            height: i32::try_from(self.height).unwrap_or(i32::MAX),
        };
        if let Some(clipped) = rect.intersection(bounds) {
            for y in clipped.y..clipped.y + clipped.height {
                let row = usize::try_from(y).unwrap_or(0).saturating_mul(self.width);
                for x in clipped.x..clipped.x + clipped.width {
                    let index = row.saturating_add(usize::try_from(x).unwrap_or(0));
                    if let Some(pixel) = self.pixels.get_mut(index) {
                        *pixel = u8::MAX - *pixel;
                    }
                }
            }
        }
    }

    /// Mixes `value` into one pixel by `coverage`, where 0 leaves the pixel
    /// untouched and 255 replaces it.
    ///
    /// Antialiased glyph edges are the reason this exists: a 300 pixel-per-inch
    /// panel resolves sixteen grey levels, so stair-stepped text is visibly
    /// worse than blended text at no extra refresh cost.
    pub fn blend(&mut self, x: i32, y: i32, value: u8, coverage: u8) {
        if x < 0 || y < 0 {
            return;
        }
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return;
        };
        if x >= self.width {
            return;
        }
        let Some(index) = y.checked_mul(self.width).and_then(|row| row.checked_add(x)) else {
            return;
        };
        if let Some(pixel) = self.pixels.get_mut(index) {
            let destination = i32::from(*pixel);
            let ink = i32::from(value);
            let mixed = destination + (ink - destination) * i32::from(coverage) / 255;
            *pixel = u8::try_from(mixed.clamp(0, 255)).unwrap_or(*pixel);
        }
    }

    pub fn stroke_rect(&mut self, rect: Rect, value: u8) {
        self.fill_rect(
            Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: 1,
            },
            value,
        );
        self.fill_rect(
            Rect {
                x: rect.x,
                y: rect.y.saturating_add(rect.height).saturating_sub(1),
                width: rect.width,
                height: 1,
            },
            value,
        );
        self.fill_rect(
            Rect {
                x: rect.x,
                y: rect.y,
                width: 1,
                height: rect.height,
            },
            value,
        );
        self.fill_rect(
            Rect {
                x: rect.x.saturating_add(rect.width).saturating_sub(1),
                y: rect.y,
                width: 1,
                height: rect.height,
            },
            value,
        );
    }
}

/// Non-flashing updates permitted before the panel gets a cleaning refresh.
pub const PANEL_CLEAN_INTERVAL: u32 = 8;

/// The physical update strategy selected from a frame's changed pixels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PanelWaveform {
    /// Fast, two-level feedback for changes containing only black and white.
    Du,
    /// Sixteen-level partial refresh for text and images containing grey.
    Gl16,
    /// Full sixteen-level refresh that clears accumulated residue.
    Gc16,
}

impl PanelWaveform {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Du => "DU",
            Self::Gl16 => "GL16",
            Self::Gc16 => "GC16",
        }
    }
}

/// One refresh the runtime will ask the panel controller to perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameTransition {
    pub region: Rect,
    pub waveform: PanelWaveform,
    pub full: bool,
    /// One-based number of the refresh in this session.
    pub refresh: u64,
    /// Partial refreshes that will have accumulated after this transition.
    pub partials_since_clean: u32,
}

/// Shared state machine for choosing Kobo panel transitions.
///
/// The device runtime and simulator both use this exact planner. Physics such
/// as visible residue remains a simulator approximation, but the changed
/// rectangle, waveform and cleaning cadence cannot drift from the device.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FramePlanner {
    width: usize,
    height: usize,
    previous: Vec<u8>,
    partials_since_clean: u32,
    refreshes: u64,
    started: bool,
}

impl FramePlanner {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            previous: vec![tone::INK; width.saturating_mul(height)],
            partials_since_clean: 0,
            refreshes: 0,
            started: false,
        }
    }

    /// Plans the next update without changing planner state.
    ///
    /// Returning `None` means the surface is the wrong size or no pixel has
    /// changed. Call [`Self::commit`] only after the update succeeds.
    #[must_use]
    pub fn plan(&self, surface: &Surface) -> Option<FrameTransition> {
        if surface.width != self.width
            || surface.height != self.height
            || surface.pixels.len() != self.previous.len()
        {
            return None;
        }
        let whole = Rect {
            x: 0,
            y: 0,
            width: i32::try_from(self.width).ok()?,
            height: i32::try_from(self.height).ok()?,
        };
        let (region, waveform) = if self.started {
            let changed = self.changed(surface)?;
            if self.partials_since_clean >= PANEL_CLEAN_INTERVAL {
                (whole, PanelWaveform::Gc16)
            } else if Self::has_grey(surface, changed) {
                (changed, PanelWaveform::Gl16)
            } else {
                (changed, PanelWaveform::Du)
            }
        } else {
            (whole, PanelWaveform::Gc16)
        };
        let full = waveform == PanelWaveform::Gc16;
        Some(FrameTransition {
            region,
            waveform,
            full,
            refresh: self.refreshes.saturating_add(1),
            partials_since_clean: if full {
                0
            } else {
                self.partials_since_clean.saturating_add(1)
            },
        })
    }

    /// Records a successfully applied transition.
    pub fn commit(&mut self, surface: &Surface, transition: FrameTransition) -> bool {
        if surface.width != self.width
            || surface.height != self.height
            || surface.pixels.len() != self.previous.len()
            || transition.refresh != self.refreshes.saturating_add(1)
        {
            return false;
        }
        self.previous.copy_from_slice(&surface.pixels);
        self.partials_since_clean = transition.partials_since_clean;
        self.refreshes = transition.refresh;
        self.started = true;
        true
    }

    #[must_use]
    pub const fn refreshes(&self) -> u64 {
        self.refreshes
    }

    #[must_use]
    pub const fn partials_since_clean(&self) -> u32 {
        self.partials_since_clean
    }

    fn changed(&self, surface: &Surface) -> Option<Rect> {
        let (mut left, mut right) = (usize::MAX, 0usize);
        let (mut top, mut bottom) = (usize::MAX, 0usize);
        for (index, _) in surface
            .pixels
            .iter()
            .zip(self.previous.iter())
            .enumerate()
            .filter(|(_, (current, previous))| current != previous)
        {
            let (x, y) = (index % self.width, index / self.width);
            left = left.min(x);
            right = right.max(x);
            top = top.min(y);
            bottom = bottom.max(y);
        }
        (left <= right).then(|| Rect {
            x: i32::try_from(left).unwrap_or(i32::MAX),
            y: i32::try_from(top).unwrap_or(i32::MAX),
            width: i32::try_from(right - left + 1).unwrap_or(i32::MAX),
            height: i32::try_from(bottom - top + 1).unwrap_or(i32::MAX),
        })
    }

    fn has_grey(surface: &Surface, region: Rect) -> bool {
        let Ok(left) = usize::try_from(region.x) else {
            return false;
        };
        let Ok(top) = usize::try_from(region.y) else {
            return false;
        };
        let Ok(width) = usize::try_from(region.width) else {
            return false;
        };
        let Ok(height) = usize::try_from(region.height) else {
            return false;
        };
        (top..top.saturating_add(height)).any(|y| {
            let start = y.saturating_mul(surface.width).saturating_add(left);
            let end = start.saturating_add(width);
            surface
                .pixels
                .get(start..end)
                .unwrap_or(&[])
                .iter()
                .any(|tone| *tone != tone::INK && *tone != tone::PAPER)
        })
    }
}

/// Eight-bit grey pixels, row major, `width * height` of them.
#[derive(Clone, Copy, Debug)]
pub struct PicturePixels<'a> {
    pub width: u32,
    pub height: u32,
    pub grey: &'a [u8],
}

/// Where the renderer finds the pictures an application handed over.
///
/// Pictures are looked up at paint time rather than travelling with the screen,
/// so a source that has lost one — evicted, never delivered, or refused — is a
/// normal condition and answers `None`. Nothing is drawn in that case, which is
/// why a tile keeps its glyph as well as its picture.
pub trait Pictures {
    fn get(&self, handle: PictureHandle) -> Option<PicturePixels<'_>>;

    /// Checks availability without marking the picture recently drawn.
    fn contains(&self, handle: PictureHandle) -> bool {
        self.get(handle).is_some()
    }
}

/// A source holding nothing, for the many callers that draw no pictures.
impl Pictures for () {
    fn get(&self, _handle: PictureHandle) -> Option<PicturePixels<'_>> {
        None
    }
}

impl Screen {
    /// Validates layout, text coverage, limits, and touch targets without
    /// assuming that asynchronous pictures have arrived yet.
    ///
    /// Measured with the back chrome the runtime gives every application other
    /// than the home screen, because that is the smaller content area and the
    /// one that decides what is cut off. Validating without it reported a
    /// clean screen for content the panel would go on to clip.
    #[must_use]
    pub fn validate(&self, metrics: &DisplayMetrics) -> Vec<LayoutIssue> {
        self.diagnostics(metrics, Chrome::with_back(true)).issues
    }

    /// Produces layout and diagnostics from one consistent set of metrics.
    #[must_use]
    pub fn diagnostics(&self, metrics: &DisplayMetrics, chrome: Chrome) -> LayoutDiagnostics {
        diagnose_screen(self, metrics, chrome, None)
    }

    /// Also reports picture handles absent from the runtime cache.
    #[must_use]
    pub fn diagnostics_with_pictures(
        &self,
        metrics: &DisplayMetrics,
        chrome: Chrome,
        pictures: &dyn Pictures,
    ) -> LayoutDiagnostics {
        diagnose_screen(self, metrics, chrome, Some(pictures))
    }
}

fn diagnose_screen(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: Chrome,
    pictures: Option<&dyn Pictures>,
) -> LayoutDiagnostics {
    let layout = screen.layout_with(metrics, chrome);
    let mut issues = Vec::new();
    let mut nodes = Vec::new();
    collect_nodes(&screen.nodes, 0, &mut nodes, &mut issues);

    let mut identifiers = Vec::new();
    if let Some(top) = &screen.top_bar {
        check_identifier(top.id, &mut identifiers, &mut issues);
        check_text_coverage(top.id, &top.title, Face::Text, &mut issues);
        if let Some(action) = &top.action {
            check_text_coverage(top.id, &action.label, Face::Text, &mut issues);
        }
    }
    if let Some(nav) = &screen.nav_bar {
        check_identifier(nav.id, &mut identifiers, &mut issues);
        if nav.destinations.len() > nav.visible(metrics).len() {
            issues.push(limit_issue(
                nav.id,
                "navigation destinations",
                nav.destinations.len(),
                nav.visible(metrics).len(),
            ));
        }
        for destination in &nav.destinations {
            check_text_coverage(nav.id, &destination.label, Face::Text, &mut issues);
        }
    }
    if let Some(bottom) = &screen.bottom_action {
        check_identifier(bottom.id, &mut identifiers, &mut issues);
        check_text_coverage(bottom.id, &bottom.action.label, Face::Text, &mut issues);
    }
    for node in &nodes {
        check_identifier(node.id(), &mut identifiers, &mut issues);
        validate_node(node, metrics, pictures, &mut issues);
    }

    validate_content_bounds(&nodes, &layout, metrics, &mut issues);
    validate_layout_nodes(&layout, metrics, &mut issues);
    LayoutDiagnostics { layout, issues }
}

fn collect_nodes<'a>(
    nodes: &'a [Node],
    depth: usize,
    collected: &mut Vec<&'a Node>,
    issues: &mut Vec<LayoutIssue>,
) {
    if depth > MAX_LAYOUT_DEPTH {
        if let Some(node) = nodes.first() {
            issues.push(limit_issue(
                node.id(),
                "layout depth",
                depth,
                MAX_LAYOUT_DEPTH,
            ));
        }
        return;
    }
    for node in nodes {
        collected.push(node);
        if let Node::Card { children, .. } = node {
            collect_nodes(children, depth + 1, collected, issues);
        }
    }
}

fn check_identifier(id: NodeId, identifiers: &mut Vec<NodeId>, issues: &mut Vec<LayoutIssue>) {
    if identifiers.contains(&id) {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Error,
            node: Some(id),
            kind: LayoutIssueKind::DuplicateNodeId,
            rect: None,
        });
    } else {
        identifiers.push(id);
    }
}

fn limit_issue(
    node: NodeId,
    collection: &'static str,
    provided: usize,
    visible: usize,
) -> LayoutIssue {
    LayoutIssue {
        severity: DiagnosticSeverity::Warning,
        node: Some(node),
        kind: LayoutIssueKind::CollectionTruncated {
            collection,
            provided,
            visible,
        },
        rect: None,
    }
}

fn validate_node(
    node: &Node,
    metrics: &DisplayMetrics,
    pictures: Option<&dyn Pictures>,
    issues: &mut Vec<LayoutIssue>,
) {
    let id = node.id();
    match node {
        Node::Heading { text, .. }
        | Node::Text { text, .. }
        | Node::Quote { text, .. }
        | Node::Banner { text, .. } => check_text_coverage(id, text, Face::Text, issues),
        Node::Button { label, .. } => check_text_coverage(id, label, Face::Text, issues),
        Node::Card { .. }
        | Node::Divider { .. }
        | Node::Spacer { .. }
        | Node::Progress { .. }
        | Node::Skeleton { .. } => {}
        Node::PagedList { items, .. } => {
            for item in items {
                check_text_coverage(id, item, Face::Text, issues);
            }
        }
        Node::Grid { cells, .. } => {
            if cells.len() > MAX_CELLS {
                issues.push(limit_issue(id, "grid cells", cells.len(), MAX_CELLS));
            }
            for cell in cells {
                check_text_coverage(id, &cell.label, Face::Text, issues);
            }
        }
        Node::Rows { rows, .. } => {
            if rows.len() > MAX_ROWS {
                issues.push(limit_issue(id, "rows", rows.len(), MAX_ROWS));
            }
            for row in rows {
                check_text_coverage(id, &row.title, Face::Text, issues);
                check_text_coverage(id, &row.summary, Face::Text, issues);
            }
        }
        Node::TileGrid { tiles, .. } => {
            for tile in tiles {
                check_text_coverage(id, &tile.label, Face::Text, issues);
                if let (Some(pictures), Some(picture)) = (pictures, tile.picture) {
                    check_picture(id, picture.handle, picture.source, pictures, issues);
                }
            }
        }
        Node::Choice {
            prompt,
            options,
            freeform,
            ..
        } => {
            check_text_coverage(id, prompt, Face::Text, issues);
            if options.is_empty() && freeform.is_none() {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::EmptyChoice,
                    rect: None,
                });
            }
            if options.len() > MAX_CHOICE_OPTIONS {
                issues.push(limit_issue(
                    id,
                    "choice options",
                    options.len(),
                    MAX_CHOICE_OPTIONS,
                ));
            }
            for option in options {
                check_text_coverage(id, &option.label, Face::Text, issues);
            }
            if let Some(freeform) = freeform {
                check_text_coverage(id, &freeform.placeholder, Face::Text, issues);
            }
        }
        Node::Picture { handle, source, .. } => match pictures {
            Some(pictures) => check_picture(id, *handle, *source, pictures, issues),
            None if source.0 == 0 || source.1 == 0 => issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(id),
                kind: LayoutIssueKind::InvalidPictureSource,
                rect: None,
            }),
            None => {}
        },
        Node::Activity { label, cancel, .. } => {
            check_text_coverage(id, label, Face::Text, issues);
            if let Some(cancel) = cancel {
                check_text_coverage(id, &cancel.label, Face::Text, issues);
            }
        }
        Node::Terminal { rows, .. } => {
            if rows.len() > MAX_TERMINAL_ROWS {
                issues.push(limit_issue(
                    id,
                    "terminal rows",
                    rows.len(),
                    MAX_TERMINAL_ROWS,
                ));
            }
            for row in rows {
                check_text_coverage(id, row, Face::Mono, issues);
                let columns = row.chars().count();
                if columns > MAX_TERMINAL_COLUMNS {
                    issues.push(limit_issue(
                        id,
                        "terminal columns",
                        columns,
                        MAX_TERMINAL_COLUMNS,
                    ));
                    break;
                }
            }
        }
    }

    // Keep this parameter part of the validation contract: very narrow panels
    // can expose limit failures even if the current supported one does not.
    let _ = metrics;
}

fn check_picture(
    id: NodeId,
    handle: PictureHandle,
    source: (u32, u32),
    pictures: &dyn Pictures,
    issues: &mut Vec<LayoutIssue>,
) {
    if source.0 == 0 || source.1 == 0 {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Error,
            node: Some(id),
            kind: LayoutIssueKind::InvalidPictureSource,
            rect: None,
        });
    } else if !pictures.contains(handle) {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::MissingPicture(handle),
            rect: None,
        });
    }
}

fn check_text_coverage(id: NodeId, text: &str, face: Face, issues: &mut Vec<LayoutIssue>) {
    if let Some(character) = undrawable_in(text, face) {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Error,
            node: Some(id),
            kind: LayoutIssueKind::UnsupportedCharacter { character, face },
            rect: None,
        });
    }
}

fn validate_content_bounds(
    nodes: &[&Node],
    layout: &Layout,
    metrics: &DisplayMetrics,
    issues: &mut Vec<LayoutIssue>,
) {
    let mut hidden = Vec::new();
    let mut clipped = Vec::new();
    for node in nodes {
        let id = node.id();
        let laid_out = layout.nodes.iter().filter(|laid_out| laid_out.id == id);
        let rects = laid_out.map(|laid_out| laid_out.rect).collect::<Vec<_>>();
        let expects_rect = !matches!(node, Node::Rows { rows, .. } if rows.is_empty());
        if expects_rect
            && (rects.is_empty()
                || rects
                    .iter()
                    .all(|rect| rect.intersection(layout.content).is_none()))
        {
            hidden.push(id);
        } else if rects
            .iter()
            .any(|rect| !rect_is_inside(*rect, layout.content))
            && !clipped.contains(&id)
        {
            clipped.push(id);
            let rect = rects
                .iter()
                .copied()
                .find(|rect| !rect_is_inside(*rect, layout.content));
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(id),
                kind: LayoutIssueKind::Clipped,
                rect,
            });
        }
    }
    if let Some(first) = hidden.first().copied() {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Error,
            node: Some(first),
            kind: LayoutIssueKind::ContentOverflow {
                hidden_nodes: hidden.len(),
            },
            rect: None,
        });
    }
    if layout.nodes.len() >= MAX_LAYOUT_NODES {
        issues.push(limit_issue(
            NodeId(0),
            "layout nodes",
            layout.nodes.len(),
            MAX_LAYOUT_NODES,
        ));
    }
    let _ = metrics;
}

fn validate_layout_nodes(layout: &Layout, metrics: &DisplayMetrics, issues: &mut Vec<LayoutIssue>) {
    let minimum = metrics.touch_target_minimum();
    for node in &layout.nodes {
        if is_tappable(node.kind) && (node.rect.width < minimum || node.rect.height < minimum) {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(node.id),
                kind: LayoutIssueKind::TouchTargetTooSmall { minimum },
                rect: Some(node.rect),
            });
        }
        let Some((size, face)) = layout_text_style(node) else {
            continue;
        };
        let too_wide = node
            .text_lines
            .iter()
            .any(|line| measure_text_in(line, size, face).0 > node.rect.width);
        let too_tall = i32::try_from(node.text_lines.len())
            .unwrap_or(i32::MAX)
            .saturating_mul(size.line_height_in(face))
            > node.rect.height;
        if too_wide || too_tall {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(node.id),
                kind: LayoutIssueKind::TextOverflow,
                rect: Some(node.rect),
            });
        }
    }
}

const fn is_tappable(kind: LayoutKind) -> bool {
    matches!(
        kind,
        LayoutKind::Button(_, ControlState::Enabled)
            | LayoutKind::Back
            | LayoutKind::BarAction(_)
            | LayoutKind::NavDestination(_)
            | LayoutKind::NavDestinationSelected(_)
            | LayoutKind::Row(_)
            | LayoutKind::Cell(_)
            | LayoutKind::Tile(_)
            | LayoutKind::ChoiceOption(_, _)
            | LayoutKind::ChoiceFreeform(_)
    )
}

fn layout_text_style(node: &LayoutNode) -> Option<(FontSize, Face)> {
    let size = match node.kind {
        LayoutKind::Heading => FontSize::Heading,
        LayoutKind::CellLabel
            if node
                .text_lines
                .first()
                .is_some_and(|text| text.chars().count() <= 2) =>
        {
            FontSize::Heading
        }
        LayoutKind::TopBarTitle => FontSize::Title,
        LayoutKind::RowSummary
        | LayoutKind::TileLabel
        | LayoutKind::NavDestination(_)
        | LayoutKind::NavDestinationSelected(_) => FontSize::Caption,
        LayoutKind::Text
        | LayoutKind::Quote(_)
        | LayoutKind::Button(_, _)
        | LayoutKind::PagedList
        | LayoutKind::BarAction(_)
        | LayoutKind::RowTitle
        | LayoutKind::RowTitleDone
        | LayoutKind::CellLabel
        | LayoutKind::ChoicePrompt
        | LayoutKind::ChoiceOption(_, _)
        | LayoutKind::ChoiceFreeform(_)
        | LayoutKind::Banner(_)
        | LayoutKind::ActivityLabel => FontSize::Body,
        LayoutKind::TerminalGrid | LayoutKind::TerminalCursor => {
            return Some((TERMINAL_SIZE, Face::Mono));
        }
        _ => return None,
    };
    Some((size, Face::Text))
}

const fn rect_is_inside(rect: Rect, bounds: Rect) -> bool {
    rect.x >= bounds.x
        && rect.y >= bounds.y
        && rect.x.saturating_add(rect.width) <= bounds.x.saturating_add(bounds.width)
        && rect.y.saturating_add(rect.height) <= bounds.y.saturating_add(bounds.height)
}

/// Eight megabytes, which is a shelf of about seventy covers.
///
/// The bound is on bytes rather than on a count, because a count would let one
/// application holding a few large pictures use far more memory than another
/// holding many small ones. This device has 512 MB and no swap, so an unbounded
/// cache is a way to have the kernel kill the runtime.
pub const DEFAULT_PICTURE_BUDGET: usize = 8 * 1024 * 1024;

struct HeldPicture {
    handle: PictureHandle,
    width: u32,
    height: u32,
    grey: Vec<u8>,
    used: std::cell::Cell<u64>,
}

struct PendingPicture {
    handle: PictureHandle,
    width: u32,
    height: u32,
    expected: usize,
    grey: Vec<u8>,
}

/// The pictures one application has handed over, bounded by total size.
///
/// Eviction is least-recently-drawn. A picture that falls out is not an error:
/// the screen still names it, the renderer finds nothing, and a tile falls back
/// to its glyph. That is why nothing in the UI treats a missing picture as a
/// failure.
pub struct PictureCache {
    budget: usize,
    held: usize,
    entries: Vec<HeldPicture>,
    clock: std::cell::Cell<u64>,
    pending: Option<PendingPicture>,
}

impl Default for PictureCache {
    fn default() -> Self {
        Self::new(DEFAULT_PICTURE_BUDGET)
    }
}

impl std::fmt::Debug for PictureCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureCache")
            .field("held", &self.held)
            .field("budget", &self.budget)
            .field("pictures", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl PictureCache {
    #[must_use]
    pub const fn new(budget: usize) -> Self {
        Self {
            budget,
            held: 0,
            entries: Vec::new(),
            clock: std::cell::Cell::new(0),
            pending: None,
        }
    }

    /// Accepts a picture, replacing any picture already under that handle.
    ///
    /// Returns `false` when the declared size does not match the bytes, or when
    /// one picture alone exceeds the whole budget. Both are refusals rather
    /// than truncations: a half-stored picture would draw as garbage.
    pub fn put(&mut self, handle: PictureHandle, width: u32, height: u32, grey: Vec<u8>) -> bool {
        self.put_report(handle, width, height, grey).is_some()
    }

    /// Stores a complete picture and reports any handles evicted to make room.
    ///
    /// `None` means the picture was refused. An empty vector means it fitted
    /// without eviction. This gives runtimes and simulator diagnostics a way
    /// to explain a missing image instead of silently falling back forever.
    pub fn put_report(
        &mut self,
        handle: PictureHandle,
        width: u32,
        height: u32,
        grey: Vec<u8>,
    ) -> Option<Vec<PictureHandle>> {
        let expected = usize::try_from(width).ok().and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|h| width.checked_mul(h))
        });
        let expected = expected?;
        if expected == 0 || expected != grey.len() || grey.len() > self.budget {
            return None;
        }
        self.remove(handle);
        let mut evicted = Vec::new();
        while self.held + grey.len() > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.used.get())
                .map(|(index, _)| index)
            else {
                break;
            };
            evicted.push(self.entries[oldest].handle);
            self.held -= self.entries[oldest].grey.len();
            self.entries.remove(oldest);
        }
        self.held += grey.len();
        self.clock.set(self.clock.get() + 1);
        self.entries.push(HeldPicture {
            handle,
            width,
            height,
            grey,
            used: std::cell::Cell::new(self.clock.get()),
        });
        Some(evicted)
    }

    /// Starts a bounded, in-order upload without replacing the live picture.
    ///
    /// Starting another upload cancels the incomplete one. The previous live
    /// value under `handle` remains drawable until [`Self::commit_upload`].
    pub fn begin_upload(&mut self, handle: PictureHandle, width: u32, height: u32) -> bool {
        let expected = usize::try_from(width).ok().and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        });
        let Some(expected) = expected else {
            self.pending = None;
            return false;
        };
        if expected == 0 || expected > self.budget {
            self.pending = None;
            return false;
        }
        self.pending = Some(PendingPicture {
            handle,
            width,
            height,
            expected,
            grey: Vec::with_capacity(expected),
        });
        true
    }

    /// Appends one chunk at its exact expected offset.
    pub fn upload_chunk(&mut self, handle: PictureHandle, offset: usize, bytes: &[u8]) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        if pending.handle != handle
            || offset != pending.grey.len()
            || pending.grey.len().saturating_add(bytes.len()) > pending.expected
        {
            self.pending = None;
            return false;
        }
        pending.grey.extend_from_slice(bytes);
        true
    }

    /// Atomically replaces the live picture after every byte has arrived.
    ///
    /// Returns evicted handles on success and `None` for an incomplete or
    /// mismatched upload.
    pub fn commit_upload(&mut self, handle: PictureHandle) -> Option<Vec<PictureHandle>> {
        let pending = self.pending.take()?;
        if pending.handle != handle || pending.grey.len() != pending.expected {
            return None;
        }
        self.put_report(pending.handle, pending.width, pending.height, pending.grey)
    }

    pub fn remove(&mut self, handle: PictureHandle) {
        if let Some(index) = self.entries.iter().position(|entry| entry.handle == handle) {
            self.held -= self.entries[index].grey.len();
            self.entries.remove(index);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.held = 0;
        self.pending = None;
    }

    #[must_use]
    pub const fn bytes_held(&self) -> usize {
        self.held
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Pictures for PictureCache {
    fn get(&self, handle: PictureHandle) -> Option<PicturePixels<'_>> {
        let entry = self.entries.iter().find(|entry| entry.handle == handle)?;
        // Drawing counts as use, so a cover on the current screen outlives one
        // that was loaded later and never shown.
        self.clock.set(self.clock.get() + 1);
        entry.used.set(self.clock.get());
        Some(PicturePixels {
            width: entry.width,
            height: entry.height,
            grey: &entry.grey,
        })
    }

    fn contains(&self, handle: PictureHandle) -> bool {
        self.entries.iter().any(|entry| entry.handle == handle)
    }
}

/// Rasterizes a retained screen. `dirty` limits writes to a changed rectangle when supplied.
pub fn render(screen: &Screen, surface: &mut Surface, dirty: Option<Rect>) {
    render_with(screen, &CLARA_BW_METRICS, Chrome::default(), surface, dirty);
}

/// Draws `pixels` into `rect`, shrinking by averaging when the picture is
/// larger than the space it was given.
///
/// Averaging rather than sampling matters here: dropping pixels from a
/// halftoned image produces moire, which on a sixteen-grey panel looks like
/// damage. An application that fitted the picture before handing it over lands
/// in the exact-size path and pays nothing.
fn draw_picture(surface: &mut Surface, rect: Rect, pixels: PicturePixels<'_>, clip: Rect) {
    let Some(visible) = rect.intersection(clip) else {
        return;
    };
    let source_width = pixels.width as usize;
    let source_height = pixels.height as usize;
    if rect.width <= 0 || rect.height <= 0 || source_width == 0 || source_height == 0 {
        return;
    }
    if pixels.grey.len() < source_width * source_height {
        return;
    }
    let target_width = rect.width as usize;
    let target_height = rect.height as usize;
    for y in visible.y..visible.y + visible.height {
        let row = (y - rect.y) as usize;
        let from_y = row * source_height / target_height;
        let to_y = max(from_y + 1, (row + 1) * source_height / target_height);
        for x in visible.x..visible.x + visible.width {
            let column = (x - rect.x) as usize;
            let from_x = column * source_width / target_width;
            let to_x = max(from_x + 1, (column + 1) * source_width / target_width);
            let mut total = 0u32;
            let mut counted = 0u32;
            for sample_y in from_y..to_y.min(source_height) {
                let base = sample_y * source_width;
                for sample_x in from_x..to_x.min(source_width) {
                    total += u32::from(pixels.grey[base + sample_x]);
                    counted += 1;
                }
            }
            if let Some(mean) = total.checked_div(counted) {
                surface.blend(x, y, u8::try_from(mean).unwrap_or(u8::MAX), 255);
            }
        }
    }
}

/// Rasterizes a retained screen for a specific panel and runtime chrome.
///
/// The arms stay in layout-kind order rather than being merged whenever two
/// happen to draw the same way today. Merging them would couple unrelated node
/// kinds, so changing how one draws would silently change the other.
#[allow(clippy::match_same_arms)]
pub fn render_with(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: Chrome,
    surface: &mut Surface,
    dirty: Option<Rect>,
) {
    render_all(screen, metrics, chrome, &(), surface, dirty);
}

/// Rasterizes a retained screen, drawing pictures from `pictures`.
///
/// This is the whole renderer; [`render_with`] is this with an empty picture
/// source. Keeping one implementation is what stops the simulator and the panel
/// from drifting apart, which has already happened once with typefaces.
#[allow(clippy::match_same_arms)]
pub fn render_all(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: Chrome,
    pictures: &dyn Pictures,
    surface: &mut Surface,
    dirty: Option<Rect>,
) {
    let clip = dirty.unwrap_or(Rect {
        x: 0,
        y: 0,
        width: i32::try_from(surface.width).unwrap_or(i32::MAX),
        height: i32::try_from(surface.height).unwrap_or(i32::MAX),
    });
    surface.fill_rect(clip, tone::PAPER);
    let layout = screen.layout_with(metrics, chrome);
    let prose = layout.prose_face;
    for node in layout.nodes {
        if node.rect.intersection(clip).is_none() {
            continue;
        }
        match node.kind {
            LayoutKind::Card => {
                fill_clipped(surface, node.rect, tone::SURFACE, clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
            }
            LayoutKind::Button(_, ControlState::Enabled) => {
                fill_clipped(surface, node.rect, tone::INK, clip);
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Body,
                    tone::PAPER,
                    clip,
                );
            }
            LayoutKind::Button(_, ControlState::Disabled) => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Body,
                    tone::MUTED,
                    clip,
                );
            }
            // The cell is outlined rather than filled, so a board reads as
            // ruled squares and an empty cell stays paper white. Filling would
            // make every move a full-cell change, which is slow on E Ink and
            // looks like a mistake.
            LayoutKind::Cell(_) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            LayoutKind::CellLabel => {
                // Short labels are marks, not words: an X, an O or a letter is
                // the content of the cell and should fill it.
                let size = if node
                    .text_lines
                    .first()
                    .is_some_and(|label| label.chars().count() <= 2)
                {
                    FontSize::Heading
                } else {
                    FontSize::Body
                };
                draw_centered(surface, &node.text_lines, node.rect, size, tone::INK, clip);
            }
            LayoutKind::Divider => fill_clipped(surface, node.rect, tone::RULE, clip),
            LayoutKind::Progress => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness(),
                    clip,
                );
                let value = node
                    .text_lines
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                fill_clipped(
                    surface,
                    Rect {
                        x: node.rect.x + 2,
                        y: node.rect.y + 2,
                        width: node
                            .rect
                            .width
                            .saturating_sub(4)
                            .saturating_mul(min(100, value))
                            / 100,
                        height: max(0, node.rect.height - 4),
                    },
                    tone::INK,
                    clip,
                );
            }

            LayoutKind::Heading => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Heading,
                tone::INK,
                clip,
            ),
            LayoutKind::Quote(depth) => {
                if depth > 0 {
                    // The rule sits in the gutter to the left of the text,
                    // which is where the layout reserved room for it.
                    let step = metrics.space(Space::Small);
                    let thickness = metrics.rule_thickness();
                    fill_clipped(
                        surface,
                        Rect {
                            x: node.rect.x - step,
                            y: node.rect.y,
                            width: thickness,
                            height: node.rect.height,
                        },
                        tone::RULE,
                        clip,
                    );
                }
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x,
                    node.rect.y,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            // The face the layout wrapped these lines in, never a default.
            // Measuring in one face and drawing in another is what puts a line
            // past the margin it was fitted to.
            LayoutKind::Text | LayoutKind::PagedList => draw_lines_in(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                prose,
                tone::INK,
                clip,
            ),
            // The bars themselves are only a background. Drawing them as
            // separate nodes is what lets a tab switch repaint the content
            // area and two small bands instead of the entire panel.
            LayoutKind::TopBar | LayoutKind::NavBar => {
                fill_clipped(surface, node.rect, tone::PAPER, clip);
            }
            LayoutKind::TopBarTitle => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Title,
                tone::INK,
                clip,
            ),
            LayoutKind::Back => draw_back_arrow(surface, node.rect, clip),
            LayoutKind::BarAction(_) => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::NavDestination(_) => {
                draw_nav_label(surface, &node.text_lines, node.rect, metrics, false, clip);
            }
            LayoutKind::NavDestinationSelected(_) => {
                draw_nav_label(surface, &node.text_lines, node.rect, metrics, true, clip);
            }
            LayoutKind::Tile(_) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            // The tap target itself draws nothing. A hairline between rows is
            // enough separation, and a box around each one would add weight
            // that a list of several entries cannot carry.
            LayoutKind::Row(_) => {}
            LayoutKind::RowTitle => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::RowTitleDone => draw_struck_lines(
                surface,
                &node.text_lines,
                node.rect,
                metrics,
                FontSize::Body,
                clip,
            ),
            LayoutKind::RowSummary => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::RowLead(lead) => draw_row_lead(surface, lead, node.rect, clip),
            LayoutKind::TileGlyph(glyph) => draw_glyph_icon(surface, glyph, node.rect, clip),
            // Outlined, because a cover with pale edges on white paper has no
            // boundary at all and reads as text floating in space.
            LayoutKind::Picture(handle) => {
                if let Some(pixels) = pictures.get(handle) {
                    draw_picture(surface, node.rect, pixels, clip);
                }
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
            }
            LayoutKind::TileLabel => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Caption,
                tone::INK,
                clip,
            ),
            LayoutKind::ChoicePrompt => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::ChoiceOption(_, chosen) => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness(),
                    clip,
                );
                let inset = metrics.space(Space::Small);
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x + inset,
                    node.rect.y + (node.rect.height - FontSize::Body.line_height()) / 2,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
                // The answer already given is marked at the far end, drawn
                // from the icon atlas so it exists on every device whatever
                // the installed face happens to contain.
                if chosen {
                    let size = FontSize::Body.line_height();
                    draw_glyph_icon(
                        surface,
                        Glyph::Check,
                        Rect {
                            x: node.rect.x + node.rect.width - inset - size,
                            y: node.rect.y + (node.rect.height - size) / 2,
                            width: size,
                            height: size,
                        },
                        clip,
                    );
                }
            }
            // Outlined in a lighter tone and set in muted ink, so the escape
            // hatch reads as secondary to the options above it.
            LayoutKind::ChoiceFreeform(_) => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
                let inset = metrics.space(Space::Small);
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x + inset,
                    node.rect.y + (node.rect.height - FontSize::Body.line_height()) / 2,
                    FontSize::Body,
                    tone::MUTED,
                    clip,
                );
            }
            LayoutKind::Banner(level) => {
                let padding = metrics.space(Space::Small);
                let (background, ink) = match level {
                    BannerLevel::Info => (tone::SURFACE, tone::INK),
                    BannerLevel::Attention => (tone::INK, tone::PAPER),
                };
                fill_clipped(surface, node.rect, background, clip);
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x + padding,
                    node.rect.y + padding,
                    FontSize::Body,
                    ink,
                    clip,
                );
            }
            LayoutKind::Skeleton => {
                let count = node
                    .text_lines
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(1);
                let line_height = FontSize::Body.line_height();
                for index in 0..count {
                    // The last line is short, the way a real paragraph ends.
                    let width = if index + 1 == count {
                        node.rect.width * 3 / 5
                    } else {
                        node.rect.width
                    };
                    fill_clipped(
                        surface,
                        Rect {
                            x: node.rect.x,
                            y: node.rect.y + index * line_height,
                            width,
                            height: line_height - metrics.tenth_mm(20),
                        },
                        tone::SURFACE,
                        clip,
                    );
                }
            }
            LayoutKind::ActivityLabel => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::ActivityProgress => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
                let value = node
                    .text_lines
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                fill_clipped(
                    surface,
                    Rect {
                        x: node.rect.x + 2,
                        y: node.rect.y + 2,
                        width: node.rect.width.saturating_sub(4) * min(100, value) / 100,
                        height: max(0, node.rect.height - 4),
                    },
                    tone::INK,
                    clip,
                );
            }
            LayoutKind::Spacer => {}
            LayoutKind::TerminalGrid => {
                let (_, cell_height) = mono_cell(TERMINAL_SIZE);
                let mut line_y = node.rect.y;
                for line in &node.text_lines {
                    draw_text_in(
                        surface,
                        line,
                        node.rect.x,
                        line_y,
                        TERMINAL_SIZE,
                        Face::Mono,
                        tone::INK,
                        clip,
                    );
                    line_y = line_y.saturating_add(cell_height);
                }
            }
            LayoutKind::TerminalCursor => {
                // A block, not an underline or a bar. There is no blink to
                // draw attention with, so the cursor has to be found by shape
                // alone, and inversion is the only thing on this panel that is
                // unmistakable at a glance.
                fill_clipped(surface, node.rect, tone::INK, clip);
                if let Some(under) = node.text_lines.first() {
                    draw_text_in(
                        surface,
                        under,
                        node.rect.x,
                        node.rect.y,
                        TERMINAL_SIZE,
                        Face::Mono,
                        tone::PAPER,
                        clip,
                    );
                }
            }
        }
    }
}

fn draw_centered(
    surface: &mut Surface,
    lines: &[String],
    rect: Rect,
    size: FontSize,
    tone: u8,
    clip: Rect,
) {
    let text_height = size.line_height() * lines.len() as i32;
    let mut y = rect.y.saturating_add((rect.height - text_height) / 2);
    for line in lines {
        let (width, _) = measure_text(line, size);
        draw_text(
            surface,
            line,
            rect.x.saturating_add((rect.width - width) / 2),
            y,
            size,
            tone,
            clip,
        );
        y = y.saturating_add(size.line_height());
    }
}

fn draw_nav_label(
    surface: &mut Surface,
    lines: &[String],
    rect: Rect,
    metrics: &DisplayMetrics,
    selected: bool,
    clip: Rect,
) {
    draw_centered(surface, lines, rect, FontSize::Caption, tone::INK, clip);
    // Selection is marked with a bar rather than a fill. An inverted
    // destination would be the largest black area on the screen and would
    // dominate the content it is meant to be subordinate to.
    if selected {
        let thickness = metrics.rule_thickness() * 2;
        let inset = metrics.space(Space::Medium);
        fill_clipped(
            surface,
            Rect {
                x: rect.x + inset,
                y: rect.y + rect.height - thickness - metrics.space(Space::Small),
                width: max(0, rect.width - 2 * inset),
                height: thickness,
            },
            tone::INK,
            clip,
        );
    }
}

/// Draws the way back, inset inside its touch target.
///
/// The target is a finger and the mark is for an eye, and they are not the
/// same size. Drawn to fill the target the chevron was half again the height
/// of the title beside it, which reads as a mistake rather than as a control.
/// Four fifths keeps the mark in proportion to the words it sits next to while
/// the tappable area stays exactly as large as it was.
fn draw_back_arrow(surface: &mut Surface, rect: Rect, clip: Rect) {
    let inset = min(rect.width, rect.height) / 10;
    let mark = Rect {
        x: rect.x + inset,
        y: rect.y + inset,
        width: rect.width - 2 * inset,
        height: rect.height - 2 * inset,
    };
    draw_vector(surface, &vector::back_arrow(), mark, clip);
}

/// Draws whatever stands at the head of a row.
///
/// A number is set in caption size rather than body, because it is a label on
/// the row and not part of it, and centred in the same square the icon would
/// have occupied so that a list which numbers some rows and illustrates others
/// still lines up down its left edge.
fn draw_row_lead(surface: &mut Surface, lead: RowLead, rect: Rect, clip: Rect) {
    match lead {
        RowLead::Icon(glyph) => draw_glyph_icon(surface, glyph, rect, clip),
        RowLead::Number(number) => {
            let text = number.to_string();
            let size = FontSize::Caption;
            let (width, _) = measure_text(&text, size);
            let x = rect.x + (rect.width - width) / 2;
            let y = rect.y + (rect.height - size.line_height()) / 2;
            draw_text(surface, &text, x, y, size, tone::MUTED, clip);
        }
    }
}

fn draw_glyph_icon(surface: &mut Surface, glyph: Glyph, rect: Rect, clip: Rect) {
    draw_vector(surface, &vector::shapes(glyph), rect, clip);
}

/// Rasterises an icon into the largest square that fits `rect` and blends it.
///
/// Blended rather than thresholded: the panel resolves sixteen grey levels and
/// the renderer already picks a sixteen-level waveform when it sees grey, so a
/// stepped diagonal costs exactly as much to draw as a smooth one and looks
/// worse. This is the same reasoning that antialiases text.
fn draw_vector(surface: &mut Surface, shapes: &[vector::Shape], rect: Rect, clip: Rect) {
    let size = min(rect.width, rect.height);
    if size <= 0 {
        return;
    }
    let coverage = vector::render(shapes, size);
    let origin_x = rect.x + (rect.width - size) / 2;
    let origin_y = rect.y + (rect.height - size) / 2;
    for row in 0..size {
        for column in 0..size {
            let alpha = coverage.at(column, row);
            if alpha == 0 {
                continue;
            }
            let (x, y) = (origin_x + column, origin_y + row);
            if x < clip.x || y < clip.y || x >= clip.x + clip.width || y >= clip.y + clip.height {
                continue;
            }
            surface.blend(x, y, tone::INK, alpha);
        }
    }
}

fn fill_clipped(surface: &mut Surface, rect: Rect, tone: u8, clip: Rect) {
    if let Some(rect) = rect.intersection(clip) {
        surface.fill_rect(rect, tone);
    }
}

/// Outlines a rectangle with a border of the given thickness.
///
/// The thickness is not decoration. This used to draw a single pixel, which is
/// 0.08 millimetres at 300 pixels per inch: at the light tone an outline is
/// drawn in, that is close to invisible on the panel and it is the reason
/// every ruled box looked washed out while dividers, which have always used
/// the real rule thickness, looked correct. Both now come from the same
/// physical measurement.
fn stroke_clipped(surface: &mut Surface, rect: Rect, tone: u8, thickness: i32, clip: Rect) {
    // A border cannot be thicker than half the thing it surrounds, or the two
    // opposite edges overlap and the box fills in.
    let thickness = thickness
        .max(1)
        .min(rect.width.max(1))
        .min(rect.height.max(1));
    for edge in [
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: thickness,
        },
        Rect {
            x: rect.x,
            y: rect.y.saturating_add(rect.height).saturating_sub(thickness),
            width: rect.width,
            height: thickness,
        },
        Rect {
            x: rect.x,
            y: rect.y,
            width: thickness,
            height: rect.height,
        },
        Rect {
            x: rect.x.saturating_add(rect.width).saturating_sub(thickness),
            y: rect.y,
            width: thickness,
            height: rect.height,
        },
    ] {
        fill_clipped(surface, edge, tone, clip);
    }
}

fn draw_lines(
    surface: &mut Surface,
    lines: &[String],
    x: i32,
    y: i32,
    size: FontSize,
    tone: u8,
    clip: Rect,
) {
    draw_lines_in(surface, lines, x, y, size, Face::Text, tone, clip);
}

/// The same, in a named face.
#[allow(clippy::too_many_arguments)]
fn draw_lines_in(
    surface: &mut Surface,
    lines: &[String],
    x: i32,
    mut y: i32,
    size: FontSize,
    face: Face,
    tone: u8,
    clip: Rect,
) {
    for line in lines {
        draw_text_in(surface, line, x, y, size, face, tone, clip);
        y = y.saturating_add(size.line_height_in(face));
    }
}

/// Draws finished text: muted, with a rule through the middle of each line.
///
/// The strike is drawn only as wide as the text it crosses rather than the
/// whole column, because a line that runs past the last word looks like a
/// separator rather than a cancellation. It is a rule thickness, not one pixel:
/// a single pixel is under a tenth of a millimetre at this density and simply
/// is not there.
fn draw_struck_lines(
    surface: &mut Surface,
    lines: &[String],
    rect: Rect,
    metrics: &DisplayMetrics,
    size: FontSize,
    clip: Rect,
) {
    let mut y = rect.y;
    let thickness = metrics.rule_thickness();
    for line in lines {
        draw_text(surface, line, rect.x, y, size, tone::MUTED, clip);
        let width = min(measure_text(line, size).0, rect.width);
        // Through the middle of the letters rather than the middle of the line
        // box, which sits under the baseline and reads as an underline.
        let middle = y
            .saturating_add(size.line_height() / 2)
            .saturating_sub(thickness / 2);
        fill_clipped(
            surface,
            Rect {
                x: rect.x,
                y: middle,
                width,
                height: thickness,
            },
            tone::MUTED,
            clip,
        );
        y = y.saturating_add(size.line_height());
    }
}

fn draw_text(
    surface: &mut Surface,
    text: &str,
    x: i32,
    y: i32,
    size: FontSize,
    tone: u8,
    clip: Rect,
) {
    draw_text_in(surface, text, x, y, size, Face::Text, tone, clip);
}

/// Draws one run of text in a chosen face.
#[allow(clippy::too_many_arguments)]
fn draw_text_in(
    surface: &mut Surface,
    text: &str,
    x: i32,
    y: i32,
    size: FontSize,
    face: Face,
    tone: u8,
    clip: Rect,
) {
    if let Some(typesetter) = TYPESETTER.get() {
        typesetter.draw(text, x, y, size, face, &mut |pixel_x, pixel_y, coverage| {
            if coverage > 0 && clip.contains(pixel_x, pixel_y) {
                surface.blend(pixel_x, pixel_y, tone, coverage);
            }
        });
        return;
    }
    draw_fallback_text(surface, text, x, y, size, tone, clip);
}

/// Draws with the built-in bitmap when no typeface is installed.
///
/// This is uppercase-only and coarse on purpose: it exists so that a host test
/// or a bare simulator still produces a deterministic image, not because it is
/// fit to put in front of a reader.
fn draw_fallback_text(
    surface: &mut Surface,
    text: &str,
    mut x: i32,
    y: i32,
    size: FontSize,
    tone: u8,
    clip: Rect,
) {
    let scale = size.scale();
    for character in text.chars() {
        let glyph = glyph(character);
        for (row, pattern) in glyph.iter().copied().enumerate() {
            for column in 0..5 {
                if pattern & (1 << (4 - column)) != 0 {
                    let pixel = Rect {
                        x: x.saturating_add(column * scale),
                        y: y.saturating_add(
                            i32::try_from(row).unwrap_or(i32::MAX).saturating_mul(scale),
                        ),
                        width: scale,
                        height: scale,
                    };
                    if let Some(pixel) = pixel.intersection(clip) {
                        surface.fill_rect(pixel, tone);
                    }
                }
            }
        }
        x = x.saturating_add(6 * scale);
    }
}

fn glyph(character: char) -> [u8; 7] {
    match character.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110,
        ],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ':' => [0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_and_measurement_are_deterministic() {
        assert_eq!(measure_text("AB", FontSize::Body), (36, 21));
        assert_eq!(
            wrap_text("one two three", 90, FontSize::Body),
            vec!["one", "two", "three"]
        );
    }

    #[test]
    fn text_scale_has_stable_names_and_wire_values() {
        assert_eq!(TextScale::from_name("large"), Some(TextScale::Large));
        assert_eq!(TextScale::from_name("140%"), Some(TextScale::ExtraLarge));
        assert_eq!(TextScale::from_wire(1), Some(TextScale::Large));
        assert_eq!(TextScale::from_wire(9), None);
        assert_eq!(TextScale::ExtraLarge.percent(), 140);
    }

    #[test]
    fn validation_reports_content_that_layout_would_hide() {
        let nodes = (0..80)
            .map(|index| Node::Text {
                id: NodeId(index + 1),
                text: "A paragraph that occupies a real line.".into(),
            })
            .collect();
        let issues = Screen::new(1, nodes).validate(&CLARA_BW_METRICS);
        assert!(issues.iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::ContentOverflow { hidden_nodes } if hidden_nodes > 0
        )));
    }

    #[test]
    fn validation_reports_truncation_and_undersized_targets() {
        let screen = Screen::new(
            1,
            vec![
                Node::Choice {
                    id: NodeId(1),
                    prompt: "Choose".into(),
                    options: (0..=MAX_CHOICE_OPTIONS)
                        .map(|index| BarAction::new(ActionId(index as u32 + 1), "Option"))
                        .collect(),
                    selected: None,
                    freeform: None,
                },
                Node::Grid {
                    id: NodeId(2),
                    columns: MAX_COLUMNS,
                    square: false,
                    cells: vec![Cell::new(ActionId(20), "1")],
                },
            ],
        );
        let issues = screen.validate(&PANELS[1].1);
        assert!(issues.iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::CollectionTruncated {
                collection: "choice options",
                ..
            }
        )));
        assert!(issues
            .iter()
            .any(|issue| matches!(issue.kind, LayoutIssueKind::TouchTargetTooSmall { .. })));
    }

    #[test]
    fn validation_can_distinguish_a_missing_picture_from_layout() {
        let screen = Screen::new(
            1,
            vec![Node::Picture {
                id: NodeId(1),
                handle: PictureHandle(7),
                source: (10, 10),
                max_height_tenths_mm: 100,
            }],
        );
        let diagnostics = screen.diagnostics_with_pictures(
            &CLARA_BW_METRICS,
            Chrome::default(),
            &PictureCache::default(),
        );
        assert!(diagnostics.issues.iter().any(|issue| matches!(
            issue.kind,
            LayoutIssueKind::MissingPicture(PictureHandle(7))
        )));
    }

    /// Panels this SDK is expected to reach eventually. None of them is
    /// supported by the hardware gate yet; they exist here so the design
    /// system is exercised against real densities rather than one device.
    pub(super) const PANELS: [(&str, DisplayMetrics); 5] = [
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
            "libra-2",
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
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
        (
            "elipsa",
            DisplayMetrics {
                width: 1404,
                height: 1872,
                pixels_per_inch: 227,
                text_scale: TextScale::Default,
            },
        ),
    ];

    /// The whole point of measuring in millimetres: a touch target is the same
    /// physical size everywhere, even though the pixel count differs a lot.
    #[test]
    fn a_touch_target_is_seven_millimetres_on_every_panel() {
        for (name, metrics) in PANELS {
            let pixels = metrics.touch_target_minimum();
            let tenths = pixels * 254 / metrics.pixels_per_inch;
            assert!(
                (69..=71).contains(&tenths),
                "{name}: {pixels}px is {tenths} tenths of a millimetre, not 70"
            );
        }
        // Concretely: the same seven millimetres is 83 pixels on a 300 pixel
        // per inch panel and 58 on a 212 one. A shared pixel constant could
        // not be right for both.
        assert_eq!(CLARA_BW_METRICS.touch_target_minimum(), 83);
        assert_eq!(PANELS[1].1.touch_target_minimum(), 58);
    }

    #[test]
    fn column_counts_follow_physical_width_rather_than_resolution() {
        // The Nia has far fewer pixels than the Clara but is the same physical
        // width, so it must get the same layout.
        let clara = CLARA_BW_METRICS;
        let nia = PANELS[1].1;
        assert!((clara.width_tenth_mm() - nia.width_tenth_mm()).abs() <= 20);
        assert_eq!(clara.max_grid_columns(), nia.max_grid_columns());
        assert_eq!(clara.max_grid_columns(), 2);

        // A ten inch panel is wide enough for a third column.
        assert_eq!(PANELS[4].1.max_grid_columns(), 3);

        for (name, metrics) in PANELS {
            let columns = metrics.max_grid_columns();
            assert!((1..=4).contains(&columns), "{name} asked for {columns}");
            let column_width = metrics.width_tenth_mm() / columns as i32;
            assert!(
                column_width >= 450,
                "{name}: {column_width} tenths per column is too narrow to read"
            );
        }
    }

    #[test]
    fn every_panel_gets_a_usable_navigation_bar_and_visible_rules() {
        for (name, metrics) in PANELS {
            let destinations = metrics.max_nav_destinations();
            assert!(
                (MIN_NAV_DESTINATIONS..=5).contains(&destinations),
                "{name} allowed {destinations} destinations"
            );
            // Every destination has to remain at least a finger wide.
            let usable = metrics.width - 2 * metrics.screen_margin();
            assert!(usable / destinations as i32 >= metrics.touch_target_minimum());
            // A rule has to survive rounding to at least one whole pixel.
            assert!(metrics.rule_thickness() >= 1, "{name} rule vanished");
        }
    }

    #[test]
    fn the_spacing_scale_is_ordered_and_never_negative() {
        for (name, metrics) in PANELS {
            let steps = [Space::Tight, Space::Small, Space::Medium, Space::Large]
                .map(|space| metrics.space(space));
            assert!(steps[0] > 0, "{name} tight spacing vanished");
            assert!(
                steps.windows(2).all(|pair| pair[0] < pair[1]),
                "{name} spacing is not ordered: {steps:?}"
            );
        }
    }

    #[test]
    fn a_percentage_can_never_exceed_a_hundred() {
        assert_eq!(Percent::new(0).get(), 0);
        assert_eq!(Percent::new(100).get(), 100);
        assert_eq!(Percent::new(101).get(), 100);
        assert_eq!(Percent::new(u8::MAX).get(), 100);
    }

    #[test]
    fn button_hit_testing_respects_touch_target() {
        let screen = Screen::new(
            7,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(2),
                label: "Increment".into(),
                state: ControlState::Enabled,
            }],
        );
        let button = screen.layout().nodes[0].rect;
        assert!(button.height >= CLARA_BW_METRICS.touch_target_minimum());
        assert_eq!(
            screen.hit_test(button.x + 1, button.y + 1),
            Some(ActionId(2))
        );
        assert_eq!(screen.hit_test(0, 0), None);
    }

    #[test]
    fn a_disabled_button_is_visible_but_not_tappable() {
        let screen = Screen::new(
            7,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(2),
                label: "Unavailable".into(),
                state: ControlState::Disabled,
            }],
        );
        let layout = screen.layout();
        let button = &layout.nodes[0];
        assert_eq!(
            button.kind,
            LayoutKind::Button(ActionId(2), ControlState::Disabled)
        );
        assert_eq!(screen.hit_test(button.rect.x + 1, button.rect.y + 1), None);
    }

    #[test]
    fn a_disabled_button_absorbs_the_tap_rather_than_turning_the_page() {
        // A greyed-out control answering with somebody else's action is worse
        // than one that does nothing at all.
        let screen = Screen::new(
            8,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(2),
                label: "Unavailable".into(),
                state: ControlState::Disabled,
            }],
        )
        .with_page_turns(ActionId(10), ActionId(11));
        let layout = screen.layout();
        let button = layout.nodes[0].rect;
        assert_eq!(screen.hit_test(button.x + 1, button.y + 1), None);
        // Content the button does not cover still turns the page.
        assert_eq!(
            screen.hit_test(
                layout.content.x + layout.content.width - 1,
                button.y + button.height + 1
            ),
            Some(ActionId(11))
        );
    }

    #[test]
    fn renderer_writes_grayscale_pixels() {
        let screen = Screen::new(
            1,
            vec![Node::Heading {
                id: NodeId(1),
                text: "Hi".into(),
            }],
        );
        let mut surface = Surface::new(128, 128);
        render(&screen, &mut surface, None);
        assert!(surface.pixels.contains(&tone::INK));
    }

    #[test]
    fn dirty_render_leaves_other_pixels_untouched() {
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(1),
                label: "Go".into(),
                state: ControlState::Enabled,
            }],
        );
        let mut surface = Surface::new(128, 128);
        surface.clear(77);
        render(
            &screen,
            &mut surface,
            Some(Rect {
                x: 48,
                y: 48,
                width: 1,
                height: 1,
            }),
        );
        assert_eq!(surface.pixels[48 * 128 + 48], tone::INK);
        assert_eq!(surface.pixels[48 * 128 + 50], 77);
    }

    #[test]
    fn an_outlined_box_is_drawn_at_the_panel_rule_thickness() {
        // A one pixel outline is 0.08 millimetres on this panel, and at the
        // light tone an outline uses it is close to invisible. This is why
        // every ruled box looked washed out on the device while dividers,
        // which have always used the real rule thickness, looked right.
        let screen = Screen::new(
            1,
            vec![Node::Card {
                id: NodeId(1),
                children: vec![Node::Heading {
                    id: NodeId(2),
                    text: "Card".into(),
                }],
            }],
        );
        let card = screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| node.kind == LayoutKind::Card)
            .expect("the card was laid out");
        let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
        let mut surface = Surface::new(
            stride,
            usize::try_from(CLARA_BW_METRICS.height).expect("a positive height"),
        );
        surface.clear(tone::PAPER);
        render(&screen, &mut surface, None);

        let thickness = CLARA_BW_METRICS.rule_thickness();
        assert!(thickness > 1, "a rule thinner than this proves nothing");
        let column = usize::try_from(card.rect.x + card.rect.width / 2).expect("inside the panel");
        let mut drawn = 0;
        for offset in 0..thickness {
            let row = usize::try_from(card.rect.y + offset).expect("inside the panel");
            if surface.pixels[row * stride + column] == tone::RULE {
                drawn += 1;
            }
        }
        assert_eq!(
            drawn, thickness,
            "the top edge of a card is {drawn} pixels rather than {thickness}"
        );
        let below = usize::try_from(card.rect.y + thickness).expect("inside the panel");
        assert_eq!(
            surface.pixels[below * stride + column],
            tone::SURFACE,
            "the border ran past the rule thickness into the card itself"
        );
    }

    #[test]
    fn frame_planner_matches_the_panel_waveform_rules() {
        let mut planner = FramePlanner::new(8, 4);
        let mut frame = Surface::new(8, 4);
        let first = planner.plan(&frame).expect("first frame refreshes");
        assert_eq!(first.waveform, PanelWaveform::Gc16);
        assert!(first.full);
        assert!(planner.commit(&frame, first));
        assert!(planner.plan(&frame).is_none(), "unchanged frame refreshes");

        frame.pixels[2 * 8 + 3] = tone::INK;
        let black_and_white = planner.plan(&frame).expect("one changed pixel");
        assert_eq!(black_and_white.waveform, PanelWaveform::Du);
        assert_eq!(
            black_and_white.region,
            Rect {
                x: 3,
                y: 2,
                width: 1,
                height: 1,
            }
        );
        assert!(planner.commit(&frame, black_and_white));

        frame.pixels[2 * 8 + 3] = tone::MUTED;
        let grey = planner.plan(&frame).expect("grey changed");
        assert_eq!(grey.waveform, PanelWaveform::Gl16);
        assert!(planner.commit(&frame, grey));

        frame.pixels[0] = tone::INK;
        let grey_outside_change = planner.plan(&frame).expect("black pixel changed");
        assert_eq!(grey_outside_change.waveform, PanelWaveform::Du);
    }

    #[test]
    fn frame_planner_cleans_after_eight_partial_updates() {
        let mut planner = FramePlanner::new(2, 1);
        let mut frame = Surface::new(2, 1);
        let first = planner.plan(&frame).expect("first");
        assert!(planner.commit(&frame, first));
        for index in 0..PANEL_CLEAN_INTERVAL {
            frame.pixels[0] = if index % 2 == 0 {
                tone::INK
            } else {
                tone::PAPER
            };
            let partial = planner.plan(&frame).expect("partial");
            assert!(!partial.full);
            assert!(planner.commit(&frame, partial));
        }
        frame.pixels[1] = tone::INK;
        let cleaning = planner.plan(&frame).expect("cleaning refresh");
        assert_eq!(cleaning.waveform, PanelWaveform::Gc16);
        assert!(cleaning.full);
        assert_eq!(cleaning.region.width, 2);
    }

    #[test]
    fn a_box_smaller_than_its_own_border_is_still_a_box() {
        // Nothing lays out a two pixel card, but a clamped thickness is what
        // stops one being filled solid rather than outlined, and the clamp is
        // cheaper to test than to reason about.
        let mut surface = Surface::new(8, 8);
        surface.clear(tone::PAPER);
        let rect = Rect {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        stroke_clipped(
            &mut surface,
            rect,
            tone::INK,
            99,
            Rect {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
        );
        assert_eq!(surface.pixels[8 + 1], tone::INK);
        assert_eq!(
            surface.pixels[0],
            tone::PAPER,
            "the border escaped its rect"
        );
        assert_eq!(surface.pixels[4 * 8 + 4], tone::PAPER);
    }

    #[test]
    fn extreme_layout_values_are_bounded() {
        let mut node = Node::Spacer {
            id: NodeId(1),
            space: Space::Large,
        };
        for id in 2..40 {
            node = Node::Card {
                id: NodeId(id),
                children: vec![node],
            };
        }
        let screen = Screen::new(1, vec![node]);
        let layout = screen.layout();
        assert!(layout.nodes.len() <= MAX_LAYOUT_NODES);
        assert!(layout.nodes.len() <= MAX_LAYOUT_DEPTH + 2);
        let mut surface = Surface::new(128, 128);
        render(&screen, &mut surface, None);
        assert_eq!(surface.pixels.len(), 128 * 128);
    }
}

#[cfg(test)]
mod page_turn_tests {
    use super::*;

    fn paged() -> Screen {
        Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(2),
                text: "A page of a book.".to_owned(),
            }],
        )
        .with_top_bar(TopBar::new(NodeId(3), "Reading"))
        .with_page_turns(ActionId(10), ActionId(20))
    }

    #[test]
    fn the_left_of_the_page_goes_back_and_the_rest_goes_on() {
        // The gesture every Kobo has had since the first one.
        for (name, metrics) in super::tests::PANELS {
            let layout = paged().layout_for(&metrics);
            let middle = metrics.height / 2;
            assert_eq!(
                layout.hit_test(metrics.width / 8, middle),
                Some(ActionId(10)),
                "{name} did not go back"
            );
            assert_eq!(
                layout.hit_test(metrics.width * 7 / 8, middle),
                Some(ActionId(20)),
                "{name} did not go on"
            );
        }
    }

    #[test]
    fn a_control_always_beats_the_zone_underneath_it() {
        // The failure this covers is the worst one available: tapping a button
        // and turning the page instead.
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(2),
                action: ActionId(99),
                label: "Press me".to_owned(),
                state: ControlState::Enabled,
            }],
        )
        .with_page_turns(ActionId(10), ActionId(20));
        let layout = screen.layout();
        let button = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(..)))
            .expect("a button");
        let (x, y) = (
            button.rect.x + button.rect.width / 2,
            button.rect.y + button.rect.height / 2,
        );
        assert_eq!(layout.hit_test(x, y), Some(ActionId(99)));
    }

    #[test]
    fn the_bars_are_never_page_turns() {
        // Back and the navigation are the two controls a reader must be able
        // to hit without aiming.
        let screen = paged().with_nav_bar(NavBar::new(
            NodeId(4),
            vec![
                BarAction::new(ActionId(5), "One"),
                BarAction::new(ActionId(6), "Two"),
            ],
            Some(0),
        ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, Chrome::with_back(true));
        assert_eq!(layout.hit_page_turn(CLARA_BW_METRICS.width / 8, 10), None);
        assert_eq!(
            layout.hit_page_turn(
                CLARA_BW_METRICS.width / 8,
                CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height() / 2
            ),
            None
        );
    }

    #[test]
    fn a_screen_that_did_not_ask_for_them_has_none() {
        let layout = Screen::new(1, vec![]).layout();
        assert_eq!(layout.hit_test(10, 500), None);
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::tests::PANELS;
    use super::*;

    fn destinations(count: usize) -> Vec<BarAction> {
        (0..count)
            .map(|index| BarAction::new(ActionId(index as u32 + 1), format!("Tab {index}")))
            .collect()
    }

    fn kinds(layout: &Layout) -> Vec<LayoutKind> {
        layout.nodes.iter().map(|node| node.kind).collect()
    }

    #[test]
    fn back_is_absent_until_the_runtime_supplies_it() {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));

        let without = screen.layout_with(&CLARA_BW_METRICS, Chrome::with_back(false));
        assert!(!kinds(&without).contains(&LayoutKind::Back));

        let with = screen.layout_with(&CLARA_BW_METRICS, Chrome::with_back(true));
        assert!(kinds(&with).contains(&LayoutKind::Back));
    }

    #[test]
    fn back_is_reachable_by_touch_and_reports_the_reserved_action() {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));
        let layout = screen.layout_with(&CLARA_BW_METRICS, Chrome::with_back(true));
        let back = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Back)
            .expect("back control");
        assert_eq!(
            layout.hit_test(back.rect.x + 1, back.rect.y + 1),
            Some(ActionId::BACK)
        );
    }

    #[test]
    fn the_back_control_is_never_smaller_than_a_finger_on_any_panel() {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
        for (name, metrics) in PANELS {
            let layout = screen.layout_with(&metrics, Chrome::with_back(true));
            let back = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::Back)
                .expect("back control");
            assert!(
                back.rect.width >= metrics.touch_target_minimum()
                    && back.rect.height >= metrics.touch_target_minimum(),
                "{name}: back control is {}x{}, below the {} minimum",
                back.rect.width,
                back.rect.height,
                metrics.touch_target_minimum()
            );
        }
    }

    #[test]
    fn the_back_control_stays_inside_the_bar_it_belongs_to() {
        // It did not, once. The comfortable control size is ten millimetres
        // and the bar was narrowed to eight and a half, so the chevron was
        // laid out taller than the bar at a negative offset and drew above it,
        // over whatever the screen had put there.
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
        for (name, metrics) in PANELS {
            let layout = screen.layout_with(&metrics, Chrome::with_back(true));
            let bar = metrics.top_bar_height();
            for node in &layout.nodes {
                if !matches!(node.kind, LayoutKind::Back | LayoutKind::BarAction(_)) {
                    continue;
                }
                assert!(
                    node.rect.y >= 0 && node.rect.y + node.rect.height <= bar,
                    "{name}: {:?} spans {}..{} outside a bar {bar} tall",
                    node.kind,
                    node.rect.y,
                    node.rect.y + node.rect.height
                );
            }
        }
    }

    #[test]
    fn a_title_that_would_wrap_is_truncated_rather_than_growing_the_bar() {
        // A bar that grows to fit its title moves every screen's content, so
        // the title yields instead.
        let long = "An extremely long screen title that could never fit across one line";
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), long));
        for (name, metrics) in PANELS {
            let layout = screen.layout_for(&metrics);
            let title = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::TopBarTitle)
                .expect("title");
            assert_eq!(title.text_lines.len(), 1, "{name}: title wrapped");
            // And it says it was cut. Keeping the first wrapped line and
            // dropping the rest reads as the whole title, which on a news
            // headline is a different sentence rather than a shorter one.
            assert!(
                title.text_lines[0].ends_with('\u{2026}'),
                "{name}: a cut title did not say so: {:?}",
                title.text_lines[0]
            );
            assert!(
                long.starts_with(title.text_lines[0].trim_end_matches(['\u{2026}', ' '])),
                "{name}: the shown title is not the start of the real one: {:?}",
                title.text_lines[0]
            );
            let bar = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::TopBar)
                .expect("bar");
            assert_eq!(
                bar.rect.height,
                metrics.top_bar_height(),
                "{name}: bar grew to fit its title"
            );
        }
    }

    #[test]
    fn the_nav_bar_sits_on_the_bottom_edge_and_spans_the_panel() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            destinations(3),
            Some(0),
        ));
        for (name, metrics) in PANELS {
            let layout = screen.layout_for(&metrics);
            let slots = layout
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node.kind,
                        LayoutKind::NavDestination(_) | LayoutKind::NavDestinationSelected(_)
                    )
                })
                .collect::<Vec<_>>();
            assert!(!slots.is_empty(), "{name}: no destinations");
            let first = slots.first().expect("first");
            let last = slots.last().expect("last");
            assert_eq!(first.rect.x, 0, "{name}: bar does not reach the left edge");
            assert_eq!(
                last.rect.x + last.rect.width,
                metrics.width,
                "{name}: bar leaves a dead strip on the right"
            );
            assert_eq!(
                first.rect.y + first.rect.height,
                metrics.height,
                "{name}: bar is not on the bottom edge"
            );
        }
    }

    #[test]
    fn destinations_are_never_narrower_than_a_finger_on_any_panel() {
        for (name, metrics) in PANELS {
            // Ask for more than the panel can carry, on purpose.
            let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
                NodeId(1),
                destinations(5),
                Some(0),
            ));
            let layout = screen.layout_for(&metrics);
            for node in &layout.nodes {
                if matches!(
                    node.kind,
                    LayoutKind::NavDestination(_) | LayoutKind::NavDestinationSelected(_)
                ) {
                    assert!(
                        node.rect.width >= metrics.touch_target_minimum(),
                        "{name}: destination is {} wide, below the {} minimum",
                        node.rect.width,
                        metrics.touch_target_minimum()
                    );
                }
            }
        }
    }

    #[test]
    fn content_stops_above_the_nav_bar_rather_than_flowing_under_it() {
        let nodes = (0..40)
            .map(|index| Node::Text {
                id: NodeId(index),
                text: "A line of body copy that occupies a row".into(),
            })
            .collect();
        let screen =
            Screen::new(1, nodes).with_nav_bar(NavBar::new(NodeId(99), destinations(3), Some(0)));
        for (name, metrics) in PANELS {
            let layout = screen.layout_for(&metrics);
            let content_bottom = metrics.height - metrics.nav_bar_height();
            for node in &layout.nodes {
                if node.kind == LayoutKind::Text {
                    assert!(
                        node.rect.y < content_bottom,
                        "{name}: content starts underneath the nav bar"
                    );
                }
            }
        }
    }

    #[test]
    fn exactly_one_destination_reads_as_selected() {
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            destinations(3),
            Some(2),
        ));
        let layout = screen.layout();
        let selected = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::NavDestinationSelected(_)))
            .count();
        assert_eq!(selected, 1);
    }
}

#[cfg(test)]
mod row_tests {
    use super::tests::PANELS;
    use super::*;

    fn list(count: u32, summary: &str) -> Screen {
        Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: (0..count)
                    .map(|index| {
                        Row::new(
                            ActionId(index + 1),
                            format!("Entry {index}"),
                            summary.to_owned(),
                            Glyph::App,
                        )
                    })
                    .collect(),
            }],
        )
    }

    fn rects(layout: &Layout) -> Vec<Rect> {
        layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
            .map(|node| node.rect)
            .collect()
    }

    #[test]
    fn every_row_is_large_enough_to_tap_on_every_panel() {
        for (name, metrics) in PANELS {
            let layout = list(4, "A short summary.").layout_for(&metrics);
            for rect in rects(&layout) {
                assert!(
                    rect.height >= metrics.touch_target_minimum(),
                    "{name}: row is only {} tall",
                    rect.height
                );
            }
        }
    }

    #[test]
    fn rows_never_overlap_each_other() {
        for (name, metrics) in PANELS {
            let layout = list(
                6,
                "A summary long enough to wrap onto a second line on any panel we support.",
            )
            .layout_for(&metrics);
            let rects = rects(&layout);
            for pair in rects.windows(2) {
                assert!(
                    pair[0].y + pair[0].height <= pair[1].y,
                    "{name}: a row starting at {} overlaps the one ending at {}",
                    pair[1].y,
                    pair[0].y + pair[0].height
                );
            }
        }
    }

    #[test]
    fn a_tap_anywhere_in_a_row_chooses_that_row() {
        let metrics = CLARA_BW_METRICS;
        let layout = list(3, "Something to read.").layout_for(&metrics);
        for (index, rect) in rects(&layout).iter().enumerate() {
            let expected = ActionId(index as u32 + 1);
            for (x, y) in [
                (rect.x + 1, rect.y + 1),
                (rect.x + rect.width / 2, rect.y + rect.height / 2),
                (rect.x + rect.width - 2, rect.y + rect.height - 2),
            ] {
                assert_eq!(
                    layout.hit_test(x, y),
                    Some(expected),
                    "tapping {x},{y} did not choose row {index}"
                );
            }
        }
    }

    #[test]
    fn the_summary_is_actually_shown() {
        // The launcher carried a summary for every entry and drew none of them.
        let layout = list(2, "The part that explains the entry.").layout_for(&CLARA_BW_METRICS);
        let summaries = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::RowSummary))
            .count();
        assert_eq!(summaries, 2);
    }

    #[test]
    fn an_entry_without_a_summary_still_lays_out() {
        let layout = list(2, "").layout_for(&CLARA_BW_METRICS);
        assert_eq!(rects(&layout).len(), 2);
        assert!(!layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::RowSummary)));
    }

    #[test]
    fn no_rule_is_drawn_after_the_last_row() {
        // A trailing rule collided with the divider the launcher drew next.
        let layout = list(3, "Summary.").layout_for(&CLARA_BW_METRICS);
        let rules = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Divider))
            .count();
        assert_eq!(rules, 2, "three rows need two separators");
    }

    #[test]
    fn tiles_pack_more_entries_than_rows_and_rows_say_more_about_each() {
        // The trade between the two primitives, asserted rather than
        // described. A tile is sized for an icon and a name, so nine of them
        // fit in less height than nine rows; a row spends that height on the
        // summary, which is text a tile has nowhere to put. Getting this
        // backwards is how a launcher ends up showing four enormous buttons.
        let metrics = CLARA_BW_METRICS;
        let summary = "A one line summary of the entry.";
        let rows = list(9, summary).layout_for(&metrics);
        let tiles = Screen::new(
            1,
            vec![Node::TileGrid {
                shape: TileShape::Square,
                id: NodeId(1),
                tiles: (0..9)
                    .map(|index| Tile::new(ActionId(index + 1), "Entry", Glyph::App))
                    .collect(),
            }],
        )
        .layout_for(&metrics);
        let said = |layout: &Layout| {
            layout
                .nodes
                .iter()
                .flat_map(|node| node.text_lines.clone())
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert!(said(&rows).contains("summary"), "a row drops its summary");
        assert!(!said(&tiles).contains("summary"), "a tile grew a summary");
        let (Some(rows), Some(tiles)) = (rows.bounds(), tiles.bounds()) else {
            panic!("both layouts should have bounds");
        };
        assert!(
            tiles.height < rows.height,
            "rows took {} and tiles took {}",
            rows.height,
            tiles.height
        );
    }

    #[test]
    fn the_list_length_is_bounded() {
        let layout = list(MAX_ROWS as u32 + 10, "Summary.").layout_for(&CLARA_BW_METRICS);
        assert_eq!(rects(&layout).len(), MAX_ROWS);
    }
}

#[cfg(test)]
mod tile_tests {
    use super::tests::PANELS;
    use super::*;

    fn grid(count: u32) -> Screen {
        Screen::new(
            1,
            vec![Node::TileGrid {
                shape: TileShape::Square,
                id: NodeId(1),
                tiles: (0..count)
                    .map(|index| Tile::new(ActionId(index + 1), format!("App {index}"), Glyph::App))
                    .collect(),
            }],
        )
    }

    #[test]
    fn tiles_never_exceed_the_panels_column_budget() {
        for (name, metrics) in PANELS {
            let layout = grid(9).layout_for(&metrics);
            let tops = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(_)))
                .map(|node| node.rect.y)
                .collect::<Vec<_>>();
            let first_row = tops.iter().filter(|top| **top == tops[0]).count();
            assert!(
                first_row <= metrics.grid_columns(TileShape::Square),
                "{name}: {first_row} tiles on a row, budget is {}",
                metrics.grid_columns(TileShape::Square)
            );
        }
    }

    #[test]
    fn every_tile_is_large_enough_to_tap_on_every_panel() {
        for (name, metrics) in PANELS {
            let layout = grid(6).layout_for(&metrics);
            for node in &layout.nodes {
                if matches!(node.kind, LayoutKind::Tile(_)) {
                    assert!(
                        node.rect.width >= metrics.touch_target_minimum()
                            && node.rect.height >= metrics.touch_target_minimum(),
                        "{name}: tile is {}x{}",
                        node.rect.width,
                        node.rect.height
                    );
                }
            }
        }
    }

    #[test]
    fn tiles_do_not_overlap_each_other() {
        for (name, metrics) in PANELS {
            let layout = grid(7).layout_for(&metrics);
            let rects = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(_)))
                .map(|node| node.rect)
                .collect::<Vec<_>>();
            for (index, rect) in rects.iter().enumerate() {
                for other in rects.iter().skip(index + 1) {
                    assert!(
                        rect.intersection(*other).is_none(),
                        "{name}: two tiles overlap"
                    );
                }
            }
        }
    }

    #[test]
    fn tapping_a_tile_returns_that_tiles_action() {
        let screen = grid(4);
        let layout = screen.layout();
        for node in &layout.nodes {
            if let LayoutKind::Tile(action) = node.kind {
                assert_eq!(
                    layout.hit_test(node.rect.x + 2, node.rect.y + 2),
                    Some(action)
                );
            }
        }
    }

    #[test]
    fn a_grid_stays_inside_the_screen_margins() {
        for (name, metrics) in PANELS {
            let layout = grid(5).layout_for(&metrics);
            for node in &layout.nodes {
                if matches!(node.kind, LayoutKind::Tile(_)) {
                    assert!(
                        node.rect.x >= metrics.screen_margin()
                            && node.rect.x + node.rect.width
                                <= metrics.width - metrics.screen_margin(),
                        "{name}: a tile runs into the margin"
                    );
                }
            }
        }
    }

    #[test]
    fn an_empty_grid_occupies_no_space() {
        let layout = grid(0).layout();
        assert!(!layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::Tile(_))));
    }
}

#[cfg(test)]
mod choice_tests {
    use super::tests::PANELS;
    use super::*;

    fn choice(options: usize, freeform: bool) -> Screen {
        Screen::new(
            1,
            vec![Node::Choice {
                id: NodeId(1),
                prompt: "How should this be filed?".into(),
                options: (0..options)
                    .map(|index| {
                        BarAction::new(ActionId(index as u32 + 1), format!("Option {index}"))
                    })
                    .collect(),
                selected: None,
                freeform: freeform.then(|| Freeform::new(ActionId(99), "Type something else")),
            }],
        )
    }

    #[test]
    fn every_option_is_a_full_width_finger_sized_row() {
        for (name, metrics) in PANELS {
            let layout = choice(4, true).layout_for(&metrics);
            let usable = metrics.width - 2 * metrics.screen_margin();
            for node in &layout.nodes {
                if matches!(
                    node.kind,
                    LayoutKind::ChoiceOption(_, _) | LayoutKind::ChoiceFreeform(_)
                ) {
                    assert_eq!(node.rect.width, usable, "{name}: option is not full width");
                    assert!(
                        node.rect.height >= metrics.touch_target_minimum(),
                        "{name}: option is {} tall",
                        node.rect.height
                    );
                }
            }
        }
    }

    #[test]
    fn the_answer_already_given_is_the_only_one_marked() {
        let Screen { nodes, .. } = choice(4, false);
        let [Node::Choice {
            id,
            prompt,
            options,
            freeform,
            ..
        }] = &nodes[..]
        else {
            unreachable!("the fixture is one choice")
        };
        let screen = Screen::new(
            1,
            vec![Node::Choice {
                id: *id,
                prompt: prompt.clone(),
                options: options.clone(),
                selected: Some(2),
                freeform: freeform.clone(),
            }],
        );
        let marked = screen
            .layout()
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::ChoiceOption(_, chosen) => Some(chosen),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(marked, vec![false, false, true, false]);
    }

    #[test]
    fn an_answer_beyond_the_options_marks_nothing_rather_than_panicking() {
        let Screen { nodes, .. } = choice(3, false);
        let [Node::Choice { id, options, .. }] = &nodes[..] else {
            unreachable!("the fixture is one choice")
        };
        let screen = Screen::new(
            1,
            vec![Node::Choice {
                id: *id,
                prompt: String::new(),
                options: options.clone(),
                selected: Some(200),
                freeform: None,
            }],
        );
        assert!(screen
            .layout()
            .nodes
            .iter()
            .all(|node| !matches!(node.kind, LayoutKind::ChoiceOption(_, true))));
    }

    #[test]
    fn the_freeform_row_comes_last() {
        let layout = choice(3, true).layout();
        let freeform = layout
            .nodes
            .iter()
            .position(|node| matches!(node.kind, LayoutKind::ChoiceFreeform(_)))
            .expect("freeform");
        let last_option = layout
            .nodes
            .iter()
            .rposition(|node| matches!(node.kind, LayoutKind::ChoiceOption(_, _)))
            .expect("option");
        assert!(freeform > last_option);
    }

    #[test]
    fn options_beyond_the_cap_are_dropped_rather_than_shrunk() {
        // Shrinking rows to fit would produce targets too small to hit, so the
        // node refuses the surplus instead. A longer list is a paged list.
        let layout = choice(MAX_CHOICE_OPTIONS + 4, false).layout();
        let count = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::ChoiceOption(_, _)))
            .count();
        assert_eq!(count, MAX_CHOICE_OPTIONS);
    }

    #[test]
    fn options_do_not_overlap_and_each_reports_its_own_action() {
        let layout = choice(4, true).layout();
        let rows = layout
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::ChoiceOption(_, _) | LayoutKind::ChoiceFreeform(_)
                )
            })
            .collect::<Vec<_>>();
        for (index, row) in rows.iter().enumerate() {
            for other in rows.iter().skip(index + 1) {
                assert!(row.rect.intersection(other.rect).is_none(), "rows overlap");
            }
            let (LayoutKind::ChoiceOption(expected, _) | LayoutKind::ChoiceFreeform(expected)) =
                row.kind
            else {
                unreachable!("a choice lays out only options and a freeform field")
            };
            assert_eq!(
                layout.hit_test(row.rect.x + 2, row.rect.y + 2),
                Some(expected)
            );
        }
    }

    #[test]
    fn a_single_row_can_be_patched_without_repainting_the_screen() {
        // Selecting an option should cost one small refresh of that row.
        let screen = choice(4, false);
        let layout = screen.layout();
        let rect = layout.rect_of_action(ActionId(2)).expect("row rectangle");
        let full = layout.bounds().expect("bounds");
        assert!(rect.height * 4 < full.height);
    }
}

#[cfg(test)]
mod loading_tests {
    use super::tests::PANELS;
    use super::*;

    #[test]
    fn an_attention_banner_is_at_least_finger_tall_on_every_panel() {
        let screen = Screen::new(
            1,
            vec![Node::Banner {
                id: NodeId(1),
                level: BannerLevel::Attention,
                text: "Battery low".into(),
            }],
        );
        for (name, metrics) in PANELS {
            let layout = screen.layout_for(&metrics);
            let banner = layout.nodes.first().expect("banner");
            assert!(
                banner.rect.height >= metrics.touch_target_minimum(),
                "{name}: banner is only {} tall",
                banner.rect.height
            );
        }
    }

    #[test]
    fn a_skeleton_occupies_the_space_the_real_text_will() {
        // The point of a skeleton is that nothing moves when data arrives.
        let lines = 5;
        let skeleton = Screen::new(
            1,
            vec![Node::Skeleton {
                id: NodeId(1),
                lines,
            }],
        );
        let real = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: (0..lines)
                    .map(|_| "wwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwwww")
                    .collect::<Vec<_>>()
                    .join(" "),
            }],
        );
        assert_eq!(
            skeleton.layout().nodes[0].rect.height,
            real.layout().nodes[0].rect.height
        );
    }

    #[test]
    fn skeleton_line_counts_are_clamped_rather_than_trusted() {
        let layout = Screen::new(
            1,
            vec![Node::Skeleton {
                id: NodeId(1),
                lines: 255,
            }],
        )
        .layout();
        assert_eq!(
            layout.nodes[0].rect.height,
            12 * FontSize::Body.line_height()
        );
    }

    #[test]
    fn progress_snaps_to_coarse_steps_so_a_download_cannot_flood_the_panel() {
        // One refresh per percent would be a hundred refreshes per download.
        let distinct = (0..=100)
            .map(|value| Percent::new(value).coarse().get())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(distinct.len(), 21);
        assert_eq!(Percent::new(0).coarse().get(), 0);
        assert_eq!(Percent::new(100).coarse().get(), 100);
        assert_eq!(Percent::new(52).coarse().get(), 50);
    }

    #[test]
    fn activity_offers_cancel_as_a_finger_sized_target() {
        let screen = Screen::new(
            1,
            vec![Node::Activity {
                id: NodeId(1),
                label: "Fetching".into(),
                progress: Some(Percent::new(30)),
                cancel: Some(BarAction::new(ActionId(1), "Cancel")),
            }],
        );
        for (name, metrics) in PANELS {
            let layout = screen.layout_for(&metrics);
            let cancel = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::ChoiceFreeform(_)))
                .expect("cancel row");
            assert!(
                cancel.rect.height >= metrics.touch_target_minimum(),
                "{name}: cancel is {} tall",
                cancel.rect.height
            );
            assert_eq!(
                layout.hit_test(cancel.rect.x + 2, cancel.rect.y + 2),
                Some(ActionId(1))
            );
        }
    }

    #[test]
    fn indeterminate_activity_draws_no_bar() {
        // A bar that invents its own position is worse than no bar.
        let layout = Screen::new(
            1,
            vec![Node::Activity {
                id: NodeId(1),
                label: "Connecting".into(),
                progress: None,
                cancel: None,
            }],
        )
        .layout();
        assert!(!layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::ActivityProgress));
    }

    /// One sample of every node kind, so a new arm is covered by the structural
    /// tests below the moment it is added rather than whenever someone
    /// remembers.
    fn one_of_every_node() -> Vec<Node> {
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Heading".into(),
            },
            Node::Text {
                id: NodeId(2),
                text: "Some body text that is long enough to wrap onto a second line.".into(),
            },
            Node::Button {
                id: NodeId(3),
                action: ActionId(3),
                label: "Button".into(),
                state: ControlState::Enabled,
            },
            Node::Card {
                id: NodeId(4),
                children: vec![Node::Text {
                    id: NodeId(5),
                    text: "Inside a card".into(),
                }],
            },
            Node::Divider { id: NodeId(6) },
            Node::Spacer {
                id: NodeId(7),
                space: Space::Medium,
            },
            Node::Progress {
                id: NodeId(8),
                value: Percent::new(40),
            },
            Node::PagedList {
                id: NodeId(9),
                page: 0,
                items: vec!["One".into(), "Two".into()],
            },
            Node::Grid {
                id: NodeId(10),
                columns: 3,
                square: true,
                cells: (0..9)
                    .map(|index| Cell::new(ActionId(100 + index), "O"))
                    .collect(),
            },
            Node::Rows {
                id: NodeId(11),
                rows: vec![Row::new(ActionId(11), "Row", "Summary", Glyph::App)],
            },
            Node::TileGrid {
                shape: TileShape::Square,
                id: NodeId(12),
                tiles: vec![Tile::new(ActionId(12), "Tile", Glyph::App)],
            },
            Node::Choice {
                id: NodeId(13),
                prompt: "Pick one".into(),
                options: vec![BarAction::new(ActionId(13), "Option")],
                selected: Some(0),
                freeform: Some(Freeform::new(ActionId(14), "Something else")),
            },
            Node::Banner {
                id: NodeId(15),
                level: BannerLevel::Attention,
                text: "Careful".into(),
            },
            Node::Skeleton {
                id: NodeId(16),
                lines: 3,
            },
            Node::Activity {
                id: NodeId(17),
                label: "Working".into(),
                progress: Some(Percent::new(50)),
                cancel: Some(BarAction::new(ActionId(18), "Stop")),
            },
        ]
    }

    #[test]
    fn no_node_kind_lets_the_next_one_land_on_top_of_it() {
        // Every layout arm must return the y it finished at, not the height it
        // consumed. Returning a height silently rewinds the cursor to near the
        // top of the screen, so the following node is drawn over this one, and
        // because hit testing takes the last match a tap then reaches the wrong
        // control. That is exactly how the grid shipped, so the rule is
        // enforced structurally rather than remembered.
        for node in one_of_every_node() {
            let name = format!("{node:?}");
            let name = name.split_whitespace().next().unwrap_or("?").to_owned();
            let screen = Screen::new(
                1,
                vec![
                    node,
                    Node::Button {
                        id: NodeId(900),
                        action: ActionId(900),
                        label: "After".into(),
                        state: ControlState::Enabled,
                    },
                ],
            );
            let layout = screen.layout();
            let after = layout
                .nodes
                .iter()
                .find(|candidate| {
                    candidate.kind == LayoutKind::Button(ActionId(900), ControlState::Enabled)
                })
                .expect("the following button was laid out")
                .rect;
            for other in &layout.nodes {
                if other.kind == LayoutKind::Button(ActionId(900), ControlState::Enabled) {
                    continue;
                }
                assert!(
                    other.rect.y + other.rect.height <= after.y,
                    "{name} leaves {:?} overlapping the node after it at {after:?}",
                    other.rect
                );
            }
        }
    }

    #[test]
    fn every_tappable_rectangle_is_reachable_by_a_tap_at_its_centre() {
        // Overlapping controls are invisible on a panel that renders both, so
        // the only way to catch them is to ask the hit tester whether each
        // control can still be reached where a finger would land.
        let screen = Screen::new(1, one_of_every_node());
        let layout = screen.layout();
        let targets = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Button(action, ControlState::Enabled)
                | LayoutKind::BarAction(action)
                | LayoutKind::Tile(action)
                | LayoutKind::Row(action)
                | LayoutKind::Cell(action)
                | LayoutKind::ChoiceOption(action, _)
                | LayoutKind::ChoiceFreeform(action) => Some((action, node.rect)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!targets.is_empty(), "the sample screen has no controls");
        for (action, rect) in targets {
            let x = rect.x + rect.width / 2;
            let y = rect.y + rect.height / 2;
            assert_eq!(
                layout.hit_test(x, y),
                Some(action),
                "a tap in the middle of {rect:?} did not reach it"
            );
        }
    }
}

#[cfg(test)]
mod prose_tests {
    use super::tests::PANELS;
    use super::*;

    #[test]
    fn the_fallback_typesetter_treats_crlf_as_one_separator() {
        // Without a typesetter installed the fallback answers every wrap, and
        // breaking after the carriage return as well as after the line feed
        // put a blank line between every pair of lines in CRLF text.
        assert_eq!(
            fallback_line_breaks("a\r\nb"),
            vec![
                (3, BreakOpportunity::Mandatory),
                (4, BreakOpportunity::Mandatory)
            ]
        );
    }

    #[test]
    fn a_book_with_windows_line_endings_still_has_paragraphs() {
        // Project Gutenberg serves CRLF. Splitting on "\n\n" alone matched
        // nothing, so an entire novel paginated as one paragraph: a solid wall
        // of words with no space anywhere in it.
        let area = CLARA_BW_METRICS.prose_area(true, false);
        let crlf = "First paragraph, which is short.\r\n\r\nSecond paragraph.\r\n\r\nThird one.";
        let unix = normalise_breaks(crlf);
        let from_crlf = paginate(crlf, area).concat();
        assert_eq!(
            from_crlf,
            paginate(&unix, area).concat(),
            "CRLF and LF paginated differently"
        );
        assert_eq!(
            from_crlf.len(),
            3,
            "paragraphs collapsed into {from_crlf:?}"
        );
        assert!(!from_crlf.iter().any(|line| line.contains('\r')));
    }

    const DESCRIPTION: &str = "It is a truth universally acknowledged, that a single man in \
        possession of a good fortune, must be in want of a wife.\n\nHowever little known the \
        feelings or views of such a man may be on his first entering a neighbourhood, this truth \
        is so well fixed in the minds of the surrounding families, that he is considered as the \
        rightful property of some one or other of their daughters.";

    const DIALOGUE: &str = "\u{201c}My dear Mr. Bennet,\u{201d} said his lady to him one day, \
        \u{201c}have you heard that Netherfield Park is let at last?\u{201d}\n\nMr. Bennet \
        replied that he had not.\n\n\u{201c}But it is,\u{201d} returned she.\n\n\u{201c}Do you \
        not want to know who has taken it?\u{201d}\n\n\u{201c}You want to tell me, and I have no \
        objection to hearing it.\u{201d}\n\nThis was invitation enough.";

    /// One comment, long enough to run past a page on its own.
    ///
    /// Deliberately a single paragraph with no blank line anywhere in it,
    /// because that is what a Hacker News reply is and what the old
    /// pagination could not handle.
    const LONG_REPLY: &str = "The thing nobody mentions about this approach is that it moves \
        the cost rather than removing it, and the place it moves the cost to is the one place \
        nobody is measuring. I ran into exactly this two years ago on a system an order of \
        magnitude smaller, and the failure looked like a performance problem for about a month \
        before anyone worked out that it was a correctness problem wearing a performance \
        problem as a coat. The short version is that the invariant everyone assumes holds at \
        the boundary does not hold once you have more than one writer, and every layer above \
        that boundary has quietly been relying on it. You can paper over it with a lock, and \
        that is what we did, and it worked, and then it stopped working the moment somebody \
        added a second process, because the lock was in the wrong address space. If you are \
        going to do this, do the boring thing first: write down what is actually guaranteed, \
        in one file, and make everything that depends on the guarantee say so out loud. It is \
        much less fun than the clever version and it is the only one I have seen survive a \
        year of other people editing it.";

    fn book(source: &str, times: usize) -> String {
        vec![source; times].join("\n\n")
    }

    /// Lays a page out exactly as the runtime would and returns the bottom of
    /// the lowest piece of text.
    fn drawn(page: &[String], metrics: &DisplayMetrics) -> (usize, i32) {
        let nodes = page
            .iter()
            .enumerate()
            .map(|(index, paragraph)| Node::Text {
                id: NodeId(index as u32 + 1),
                text: paragraph.clone(),
            })
            .collect();
        let screen = Screen::new(1, nodes)
            .with_top_bar(TopBar::new(NodeId(0), "A Book"))
            .with_nav_bar(NavBar::new(
                NodeId(900),
                vec![
                    BarAction::new(ActionId(1), "Back"),
                    BarAction::new(ActionId(2), "Library"),
                    BarAction::new(ActionId(3), "Next"),
                ],
                None,
            ));
        let layout = screen.layout_with(metrics, Chrome::with_back(true));
        let text = layout
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::Text)
            .collect::<Vec<_>>();
        let bottom = text
            .iter()
            .map(|node| node.rect.y + node.rect.height)
            .max()
            .unwrap_or(0);
        (text.len(), bottom)
    }

    #[test]
    fn every_page_is_drawn_whole_on_every_panel() {
        // The layout engine stops at the bottom of the content area and drops
        // the rest, so a page that measured as fitting and does not is a page
        // whose last paragraph silently never appears.
        for (name, metrics) in PANELS {
            let area = metrics.prose_area(true, true);
            for (kind, source) in [("description", DESCRIPTION), ("dialogue", DIALOGUE)] {
                let pages = paginate(&book(source, 12), area);
                assert!(!pages.is_empty(), "{name} {kind} produced no pages");
                for (index, page) in pages.iter().enumerate() {
                    let (shown, bottom) = drawn(page, &metrics);
                    assert_eq!(
                        shown,
                        page.len(),
                        "{name} {kind} page {index}: {} of {} paragraphs were drawn",
                        shown,
                        page.len()
                    );
                    assert!(
                        bottom <= metrics.height - metrics.nav_bar_height(),
                        "{name} {kind} page {index} ran {} pixels under the page controls",
                        bottom - (metrics.height - metrics.nav_bar_height())
                    );
                }
            }
        }
    }

    /// The same measurement as `drawn`, for a page that carries depth.
    fn drawn_quoted(page: &[(u8, String)], metrics: &DisplayMetrics) -> (usize, i32) {
        let nodes = page
            .iter()
            .enumerate()
            .map(|(index, (depth, paragraph))| Node::Quote {
                id: NodeId(index as u32 + 1),
                depth: *depth,
                text: paragraph.clone(),
            })
            .collect();
        let screen = Screen::new(1, nodes)
            .with_top_bar(TopBar::new(NodeId(0), "A Thread"))
            .with_nav_bar(NavBar::new(
                NodeId(900),
                vec![
                    BarAction::new(ActionId(1), "Back"),
                    BarAction::new(ActionId(2), "Stories"),
                    BarAction::new(ActionId(3), "Next"),
                ],
                None,
            ));
        let layout = screen.layout_with(metrics, Chrome::with_back(true));
        let quotes = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Quote(_)))
            .collect::<Vec<_>>();
        let bottom = quotes
            .iter()
            .map(|node| node.rect.y + node.rect.height)
            .max()
            .unwrap_or(0);
        (quotes.len(), bottom)
    }

    #[test]
    fn every_page_of_a_thread_is_drawn_whole_on_every_panel() {
        // An indented paragraph is narrower, so it wraps to more lines and
        // takes more of the page. Paginating a thread flat and drawing it
        // indented loses the bottom of nearly every page, and the layout
        // engine reports nothing when it does.
        for (name, metrics) in PANELS {
            let area = metrics.prose_area(true, true);
            let source = book(DIALOGUE, 12);
            let paragraphs = source
                .split("\n\n")
                .enumerate()
                .map(|(index, paragraph)| ((index % 5) as u8, paragraph))
                .collect::<Vec<_>>();
            let pages = paginate_quoted(&paragraphs, &metrics, area);
            assert!(!pages.is_empty(), "{name} produced no pages");
            for (index, page) in pages.iter().enumerate() {
                let (shown, bottom) = drawn_quoted(page, &metrics);
                assert_eq!(
                    shown,
                    page.len(),
                    "{name} page {index}: {shown} of {} paragraphs were drawn",
                    page.len()
                );
                assert!(
                    bottom <= metrics.height - metrics.nav_bar_height(),
                    "{name} page {index} ran {} pixels under the page controls",
                    bottom - (metrics.height - metrics.nav_bar_height())
                );
            }
        }
    }

    #[test]
    fn a_long_comment_fills_the_page_it_starts_on() {
        // A threaded discussion is paragraphs of wildly different lengths, and
        // one comment is one paragraph. Moving a paragraph whole to the next
        // page rather than splitting it meant a five-hundred-word reply left
        // most of the previous page blank, so a thread took twice the page
        // turns it needed. Both sides of a split keep at least two lines.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let line_height = FontSize::Body.line_height();
        let per_page = (area.height / line_height) as usize;
        // Two of them, still as one paragraph: a reply this long is ordinary
        // on a thread about anything contentious.
        let reply = [LONG_REPLY; 2].join(" ");
        let paragraphs = vec![(0_u8, "A short opening remark."), (1, reply.as_str())];
        let pages = paginate_quoted(&paragraphs, &CLARA_BW_METRICS, area);
        assert!(pages.len() > 1, "the reply was not long enough to split");
        assert_eq!(
            pages[0].len(),
            2,
            "the reply did not start on the first page"
        );
        let (_, bottom) = drawn_quoted(&pages[0], &CLARA_BW_METRICS);
        let slack = (area.height + CLARA_BW_METRICS.screen_margin()) - bottom;
        assert!(
            slack < line_height * 3,
            "the first page left {slack} pixels empty, about {} lines",
            slack / line_height
        );
        for (index, page) in pages.iter().enumerate() {
            let lines = page
                .iter()
                .map(|(depth, text)| {
                    let (_, width) = quote_offsets(&CLARA_BW_METRICS, area.width, *depth);
                    wrap_text(text, width, FontSize::Body).len()
                })
                .sum::<usize>();
            assert!(
                lines <= per_page,
                "page {index} carries {lines} lines into room for {per_page}"
            );
        }
        let last = pages.last().expect("pages");
        let (depth, text) = last.last().expect("a paragraph");
        let (_, width) = quote_offsets(&CLARA_BW_METRICS, area.width, *depth);
        assert!(
            wrap_text(text, width, FontSize::Body).len() >= MIN_KEEP_LINES,
            "the split left an orphan line alone on the last page"
        );
    }

    #[test]
    fn a_thread_holds_less_per_page_than_the_same_words_unindented() {
        // If this ever stops being true, indentation is not being measured and
        // the previous test is passing by luck.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let source = book(DESCRIPTION, 60);
        let flat = source
            .split("\n\n")
            .map(|paragraph| (0, paragraph))
            .collect::<Vec<_>>();
        let nested = source
            .split("\n\n")
            .map(|paragraph| (3, paragraph))
            .collect::<Vec<_>>();
        let flat_pages = paginate_quoted(&flat, &CLARA_BW_METRICS, area).len();
        let nested_pages = paginate_quoted(&nested, &CLARA_BW_METRICS, area).len();
        assert!(
            nested_pages > flat_pages,
            "the same words took {nested_pages} pages indented and {flat_pages} flat"
        );
    }

    #[test]
    fn a_reply_is_set_in_from_what_it_answers_and_stops_at_the_cap() {
        // Depth past the cap shares the deepest indent rather than marching
        // off the panel: at forty levels there would be no measure left.
        let nodes = (0..6)
            .map(|depth| Node::Quote {
                id: NodeId(depth + 1),
                depth: depth as u8,
                text: "A reply, which answers the one above it.".to_owned(),
            })
            .collect();
        let layout = Screen::new(1, nodes).layout_with(&CLARA_BW_METRICS, Chrome::default());
        let quotes = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Quote(_)))
            .collect::<Vec<_>>();
        assert_eq!(quotes.len(), 6);
        for pair in quotes.windows(2) {
            let (shallow, deep) = (pair[0], pair[1]);
            let capped = matches!(deep.kind, LayoutKind::Quote(depth) if depth == MAX_QUOTE_DEPTH)
                && matches!(shallow.kind, LayoutKind::Quote(depth) if depth == MAX_QUOTE_DEPTH);
            if capped {
                assert_eq!(shallow.rect.x, deep.rect.x, "past the cap the indent moved");
                assert_eq!(shallow.rect.width, deep.rect.width);
            } else {
                assert!(
                    deep.rect.x > shallow.rect.x,
                    "a reply at depth {:?} did not start further in than {:?}",
                    deep.kind,
                    shallow.kind
                );
                assert!(
                    deep.rect.width < shallow.rect.width,
                    "a deeper reply was not narrower"
                );
            }
        }
        let widest = quotes[0].rect.x;
        let deepest = quotes[5].rect.x;
        assert!(
            deepest - widest < CLARA_BW_METRICS.prose_area(false, false).width / 3,
            "the deepest indent spent more than a third of the measure"
        );
    }

    #[test]
    fn a_page_of_dialogue_holds_less_than_a_page_of_description() {
        // This is the whole reason pagination measures rather than counting
        // characters: short paragraphs spend most of the page on the gaps
        // between them, so one budget cannot serve both.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let description: usize = paginate(&book(DESCRIPTION, 12), area)[0]
            .iter()
            .map(|paragraph| paragraph.chars().count())
            .sum();
        let dialogue: usize = paginate(&book(DIALOGUE, 12), area)[0]
            .iter()
            .map(|paragraph| paragraph.chars().count())
            .sum();
        assert!(
            dialogue < description,
            "dialogue {dialogue} was not less than description {description}"
        );
    }

    #[test]
    fn no_words_are_lost_between_pages() {
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let source = book(DESCRIPTION, 9);
        let pages = paginate(&source, area);
        assert!(pages.len() > 1, "the sample fitted on one page");
        let paginated = pages
            .iter()
            .flat_map(|page| page.iter())
            .flat_map(|paragraph| paragraph.split_whitespace())
            .collect::<Vec<_>>();
        assert_eq!(paginated, source.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn a_paragraph_longer_than_a_page_is_split_rather_than_dropped() {
        // Gutenberg's front matter is sometimes one unbroken block, and a
        // reader that dropped it would open books at chapter two.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let monster = "word ".repeat(4000);
        let pages = paginate(&monster, area);
        assert!(pages.len() > 1);
        for page in &pages {
            let (shown, bottom) = drawn(page, &CLARA_BW_METRICS);
            assert_eq!(shown, page.len());
            assert!(bottom <= CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height());
        }
    }

    #[test]
    fn a_source_line_break_does_not_become_a_short_line_on_the_panel() {
        // Gutenberg hard wraps its plain text at about seventy columns. Taking
        // those as real breaks would give a narrow ragged column down the
        // middle of a panel that is wider than seventy characters.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let pages = paginate("one two three\nfour five six\nseven eight", area);
        assert_eq!(pages, vec![vec!["one two three four five six seven eight"]]);
    }

    #[test]
    fn an_area_too_small_for_a_line_produces_no_pages_rather_than_panicking() {
        let pages = paginate(
            DESCRIPTION,
            ProseArea {
                width: 400,
                height: 2,
                gap: 4,
                face: Face::Text,
            },
        );
        assert!(pages.is_empty());
    }

    /// The hazard that cost this project a working button once already.
    ///
    /// A screen whose text above a control grows by one wrapped line pushes
    /// that control down. On a panel that takes a moment to refresh, the
    /// control moves out from under the finger that just tapped it and the
    /// next tap lands on nothing, which looks like intermittent hardware.
    ///
    /// This pins both halves: that the layout engine really does move it, so
    /// nobody has to take the hazard on trust, and that keeping the varying
    /// text below the control fixes it. Applications are written to the second
    /// shape; this is why.
    #[test]
    fn text_that_wraps_above_a_control_moves_that_control() {
        let action = ActionId(7);
        let button = |before: &str, after: &str| -> Rect {
            let mut nodes = Vec::new();
            if !before.is_empty() {
                nodes.push(Node::Text {
                    id: NodeId(1),
                    text: before.to_string(),
                });
            }
            nodes.push(Node::Button {
                id: NodeId(2),
                action,
                label: "Do it".to_string(),
                state: ControlState::Enabled,
            });
            if !after.is_empty() {
                nodes.push(Node::Text {
                    id: NodeId(3),
                    text: after.to_string(),
                });
            }
            Screen::new(1, nodes)
                .layout_for(&CLARA_BW_METRICS)
                .rect_of_action(action)
                .expect("the button is always drawn")
        };
        let short = "Ready.";
        let long = concat!(
            "Ready, and then a great deal more text than that, enough of it ",
            "to run past the end of one line and onto a second one.",
        );

        assert_ne!(
            button(short, ""),
            button(long, ""),
            "a longer line above the button has to move it, or this test proves nothing"
        );
        assert_eq!(
            button("", short),
            button("", long),
            "text below the button must not move it"
        );
    }

    #[test]
    fn a_picture_is_never_enlarged_to_fill_its_space() {
        // Upscaling a thumbnail is how a sharp cover becomes a soft one, and
        // softness is the one thing a sixteen-grey panel cannot hide.
        assert_eq!(fit_within((190, 300), 800, 800), (190, 300));
    }

    #[test]
    fn a_picture_too_wide_and_too_tall_keeps_its_proportions() {
        let (width, height) = fit_within((1000, 500), 400, 400);
        assert_eq!((width, height), (400, 200));
        let (width, height) = fit_within((500, 1000), 400, 400);
        assert_eq!((width, height), (200, 400));
    }

    #[test]
    fn a_portrait_tile_is_taller_than_a_square_one() {
        let tiles = || vec![Tile::new(ActionId(7), "Moby Dick", Glyph::Book)];
        let square = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: tiles(),
                shape: TileShape::Square,
            }],
        )
        .layout();
        let portrait = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: tiles(),
                shape: TileShape::Portrait,
            }],
        )
        .layout();
        let height = |layout: &Layout| {
            layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Tile(_)))
                .expect("a tile")
                .rect
                .height
        };
        assert!(
            height(&portrait) > height(&square),
            "a shelf of covers has to be book shaped, not stamp shaped"
        );
    }

    #[test]
    fn a_tile_without_its_picture_yet_still_shows_its_glyph() {
        // Covers arrive one network request at a time, so most of a shelf's
        // life is spent with some of them missing. That has to be a usable
        // screen rather than a broken one.
        let screen =
            Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    tiles: vec![
                        Tile::new(ActionId(7), "Waiting", Glyph::Book),
                        Tile::new(ActionId(8), "Arrived", Glyph::Book)
                            .with_picture(TilePicture::new(PictureHandle(3), 190, 300)),
                    ],
                    shape: TileShape::Portrait,
                }],
            )
            .layout();
        assert_eq!(
            screen
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::TileGlyph(_)))
                .count(),
            1
        );
        assert_eq!(
            screen
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Picture(_)))
                .count(),
            1
        );
        assert_eq!(
            screen
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::TileLabel)
                .count(),
            2,
            "both tiles keep their label whatever is above it"
        );
    }

    #[test]
    fn the_cache_refuses_a_picture_whose_size_does_not_match_its_bytes() {
        let mut cache = PictureCache::default();
        assert!(!cache.put(PictureHandle(1), 10, 10, vec![0; 99]));
        assert!(cache.get(PictureHandle(1)).is_none());
        assert!(cache.put(PictureHandle(1), 10, 10, vec![0; 100]));
        assert_eq!(cache.get(PictureHandle(1)).map(|p| p.width), Some(10));
    }

    #[test]
    fn the_cache_evicts_what_was_drawn_longest_ago() {
        let mut cache = PictureCache::new(200);
        assert!(cache.put(PictureHandle(1), 10, 10, vec![1; 100]));
        assert!(cache.put(PictureHandle(2), 10, 10, vec![2; 100]));
        // Drawing the first one makes the second the older of the two.
        assert!(cache.get(PictureHandle(1)).is_some());
        assert!(cache.put(PictureHandle(3), 10, 10, vec![3; 100]));
        assert!(cache.get(PictureHandle(1)).is_some(), "still on screen");
        assert!(
            cache.get(PictureHandle(2)).is_none(),
            "least recently drawn"
        );
        assert!(cache.get(PictureHandle(3)).is_some());
        assert_eq!(cache.bytes_held(), 200);
    }

    #[test]
    fn cache_evictions_are_reported_to_the_runtime() {
        let mut cache = PictureCache::new(150);
        assert_eq!(
            cache.put_report(PictureHandle(1), 10, 10, vec![1; 100]),
            Some(Vec::new())
        );
        assert_eq!(
            cache.put_report(PictureHandle(2), 10, 10, vec![2; 100]),
            Some(vec![PictureHandle(1)])
        );
    }

    #[test]
    fn chunked_picture_becomes_live_only_after_a_complete_commit() {
        let mut cache = PictureCache::new(300);
        assert!(cache.begin_upload(PictureHandle(7), 10, 10));
        assert!(cache.upload_chunk(PictureHandle(7), 0, &[3; 40]));
        assert!(
            cache.get(PictureHandle(7)).is_none(),
            "not partially visible"
        );
        assert!(cache.upload_chunk(PictureHandle(7), 40, &[3; 60]));
        assert_eq!(cache.commit_upload(PictureHandle(7)), Some(Vec::new()));
        assert_eq!(cache.get(PictureHandle(7)).map(|p| p.grey[0]), Some(3));
    }

    #[test]
    fn an_out_of_order_chunk_cancels_the_upload() {
        let mut cache = PictureCache::new(300);
        assert!(cache.begin_upload(PictureHandle(7), 10, 10));
        assert!(!cache.upload_chunk(PictureHandle(7), 1, &[3; 40]));
        assert_eq!(cache.commit_upload(PictureHandle(7)), None);
    }

    #[test]
    fn replacing_a_picture_does_not_double_count_it() {
        let mut cache = PictureCache::new(300);
        assert!(cache.put(PictureHandle(1), 10, 10, vec![0; 100]));
        assert!(cache.put(PictureHandle(1), 10, 10, vec![9; 100]));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.bytes_held(), 100);
        assert_eq!(cache.get(PictureHandle(1)).map(|p| p.grey[0]), Some(9));
    }

    #[test]
    fn a_picture_is_drawn_where_it_was_placed_and_nowhere_else() {
        let mut cache = PictureCache::default();
        assert!(cache.put(PictureHandle(1), 2, 2, vec![0, 0, 0, 0]));
        let mut surface = Surface::new(8, 8);
        surface.clear(tone::PAPER);
        let rect = Rect {
            x: 2,
            y: 3,
            width: 2,
            height: 2,
        };
        let clip = Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };
        draw_picture(
            &mut surface,
            rect,
            cache.get(PictureHandle(1)).expect("held"),
            clip,
        );
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..4).contains(&x) && (3..5).contains(&y);
                let pixel = surface.pixels[y * 8 + x];
                assert_eq!(
                    pixel == 0,
                    inside,
                    "pixel ({x},{y}) should {} be ink",
                    if inside { "" } else { "not" }
                );
            }
        }
    }

    #[test]
    fn shrinking_a_picture_averages_rather_than_drops_pixels() {
        // Half the source is black and half white. Sampling would give one or
        // the other; averaging gives the grey that is actually there.
        let mut cache = PictureCache::default();
        assert!(cache.put(PictureHandle(1), 2, 2, vec![0, 255, 0, 255]));
        let mut surface = Surface::new(4, 4);
        surface.clear(tone::PAPER);
        let rect = Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        };
        draw_picture(
            &mut surface,
            rect,
            cache.get(PictureHandle(1)).expect("held"),
            Rect {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            },
        );
        assert_eq!(surface.pixels[0], 127);
    }
}

#[cfg(test)]
mod press_feedback_tests {
    use super::*;

    #[test]
    fn a_finger_on_a_button_finds_the_button_and_not_the_page() {
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(7),
                label: "Read".into(),
                state: ControlState::Enabled,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, Chrome::default());
        let button = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(_, _)))
            .expect("the button was laid out")
            .rect;

        let inside = layout
            .pressed_control(button.x + button.width / 2, button.y + button.height / 2)
            .expect("a finger in the middle of the button is on the button");
        assert_eq!(inside, button);
    }

    #[test]
    fn a_finger_on_bare_text_has_nothing_to_invert() {
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "Once upon a time".into(),
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, Chrome::default());
        let text = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Text))
            .expect("the text was laid out")
            .rect;

        // Tapping prose may well turn the page, but there is no control there,
        // and inverting a paragraph would look like a fault rather than
        // feedback.
        assert_eq!(
            layout.pressed_control(text.x + text.width / 2, text.y + text.height / 2),
            None
        );
    }

    #[test]
    fn inverting_a_rectangle_twice_leaves_the_picture_as_it_was() {
        let mut surface = Surface::new(20, 10);
        surface.fill_rect(
            Rect {
                x: 2,
                y: 2,
                width: 5,
                height: 5,
            },
            30,
        );
        let before = surface.pixels.clone();
        let rect = Rect {
            x: 1,
            y: 1,
            width: 8,
            height: 8,
        };

        surface.invert_rect(rect);
        assert_ne!(surface.pixels, before, "inverting has to change something");
        surface.invert_rect(rect);
        assert_eq!(
            surface.pixels, before,
            "releasing a control must restore it exactly"
        );
    }

    #[test]
    fn inverting_off_the_edge_touches_nothing_outside_the_surface() {
        let mut surface = Surface::new(8, 8);
        let before = surface.pixels.clone();
        surface.invert_rect(Rect {
            x: -40,
            y: -40,
            width: 10,
            height: 10,
        });
        assert_eq!(surface.pixels, before);
    }
}
