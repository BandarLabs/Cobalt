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

/// The shortest hairline a [`Node::Section`] will draw, in tenths of a
/// millimetre.
///
/// A rule two millimetres long between a long title and its count does not
/// read as a rule, it reads as a typographic accident, so the title is clamped
/// until the line has room to be one. Five millimetres is about a thumbnail's
/// width and is the point at which it stops looking like a stray mark.
pub const MIN_SECTION_RULE_TENTH_MM: i32 = 50;

/// The most slots one [`Node::Band`] will place beside each other.
///
/// Three, and the cap is the point of the node. A general horizontal box with
/// no limit is a flexbox, and a flexbox is how a layout becomes something an
/// application can get wrong: four columns on a 122 millimetre panel is four
/// columns of two words. Everything this platform actually needs beside
/// something else -- a cover and its metadata, a label and its value, a title
/// and its count -- is two things, occasionally three.
pub const MAX_BAND_SLOTS: usize = 3;

/// The most label and value pairs one [`Node::Facts`] will set.
///
/// Twelve. Past that a definition list has stopped being the facts about a
/// thing and become a database dump, and the reader scrolling a thirteenth
/// line is no longer reading, they are searching -- which is a list with a
/// filter, not a block of metadata.
pub const MAX_FACTS: usize = 12;

/// The most of a panel a facts label column may take, in eighths.
///
/// Three eighths. The label column is measured from the longest label, and one
/// long label would otherwise squeeze every value on the screen into a
/// two-word gutter: the column is there to serve the values, so it is capped
/// against them.
const FACTS_LABEL_LIMIT_EIGHTHS: i32 = 3;

/// The narrowest a band slot may be before the band gives up and stacks, in
/// tenths of a millimetre.
///
/// Twelve millimetres holds roughly one short word at caption size. Below that
/// a column is not narrow, it is broken: every line wraps after a syllable and
/// the reader gets a vertical ladder of fragments. Stacking is always the
/// better answer, which is why it is automatic rather than something the
/// application is asked to arrange.
pub const MIN_BAND_SLOT_TENTH_MM: i32 = 120;

/// What a paragraph of a threaded discussion is for.
///
/// A comment is two different things printed one above the other: a line
/// saying who wrote it and when, and the thing they actually wrote. Drawn at
/// one size in one tone they read as a single block of prose whose first
/// sentence happens to be a username, which is what a real thread on a real
/// panel looked like before this existed. The byline is metadata and is set
/// like metadata.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum QuoteRole {
    /// What was said.
    #[default]
    Body,
    /// Who said it, and when. Smaller, and in the muted tone.
    Byline,
}

impl QuoteRole {
    /// The size this paragraph is measured and drawn at.
    ///
    /// One place, because wrapping at one size and drawing at another is what
    /// puts a line past the margin it was fitted to.
    #[must_use]
    pub const fn size(self) -> FontSize {
        match self {
            Self::Body => FontSize::Body,
            Self::Byline => FontSize::Caption,
        }
    }

    /// The tone it is drawn in.
    #[must_use]
    pub const fn tone(self) -> u8 {
        match self {
            Self::Body => tone::INK,
            Self::Byline => tone::MUTED,
        }
    }
}

/// How much of the panel one control is allowed to claim.
///
/// Every enabled button used to be a filled black slab. Three of them on a
/// screen is three black slabs, which is both the reason the interface read as
/// a toy next to the stock reader and a real cost on this hardware: a solid
/// fill is the most expensive thing an E Ink panel can be asked to draw and
/// the slowest to clear. The stock reader fills exactly one control per screen
/// and outlines the rest, so the eye finds the primary action in one movement
/// instead of choosing between three equally loud rectangles.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Emphasis {
    /// Outlined: a rule around dark text on paper. The default, because most
    /// buttons are not the one thing the screen is for.
    #[default]
    Normal,
    /// Filled. At most one per screen, and the layout does not enforce that,
    /// an author who fills everything is back where this started.
    Primary,
}

/// The deepest a [`Node::Quote`] is drawn.
///
/// Measured rather than picked: one indent step is one small space, and this
/// panel's text column is 91 mm wide. Past three steps a reply has lost a
/// quarter of its measure, and a discussion that nests forty deep (which real
/// ones do) would otherwise end up one word per line. Deeper replies keep
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

    /// The border of a control that can be pressed.
    ///
    /// The same thickness as a rule, and stronger than one anyway, because a
    /// rule is drawn in grey and this is drawn in ink. That is the whole of
    /// the difference a button's outline needs: a rule separates two things
    /// and wants to be quiet, an outline says where the reader may put a
    /// finger and wants to be found.
    ///
    /// It was half again a rule for a while, on the argument that a button
    /// drawn at a divider's weight reads as one more line on the page. On the
    /// panel that came out as half a millimetre of solid black around a slab
    /// twelve millimetres tall and the width of the page, and it was the
    /// loudest thing on every screen that had one: heavier than the headline
    /// above it, which no button should ever be. The tone was already carrying
    /// the argument and the thickness was carrying it twice.
    #[must_use]
    pub const fn button_border(&self) -> i32 {
        self.rule_thickness()
    }

    /// The height of the fixed bar that carries the title and the way back.
    ///
    /// Eleven millimetres to start with, which was a millimetre more than the
    /// comfortable control default for no reason beyond looking settled, and
    /// on a 122 millimetre panel that is nine per cent of everything the
    /// reader has. Eight and a half is a quarter off and still one and a half
    /// millimetres above [`Self::touch_target_minimum`], which matters because
    /// this bar carries Back, the one control that is guaranteed to work, and
    /// so the one that must never be the size of a guess.
    #[must_use]
    pub const fn top_bar_height(&self) -> i32 {
        self.tenth_mm(85)
    }

    #[must_use]
    pub const fn nav_bar_height(&self) -> i32 {
        self.tenth_mm(120)
    }

    /// The strip carrying the clock, the radio and the battery.
    ///
    /// Five millimetres: tall enough for caption type with air around it, and
    /// small enough that giving up four per cent of a 122 millimetre panel to
    /// something nobody reads on purpose is defensible. Nothing in it is
    /// tappable, so it is deliberately below [`Self::touch_target_minimum`], a
    /// strip the size of a control invites a finger that has nowhere to go.
    #[must_use]
    pub const fn status_band_height(&self) -> i32 {
        self.tenth_mm(50)
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
    /// not a line of prose, holding tiles to the text figure gave a Clara two
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

/// How many of the five inks one screen may spend.
///
/// Four, not five. The tones are separated by less contrast on electrophoretic
/// paper than they appear to have on the LCD they are designed on, so a screen
/// that reaches for all five is a screen where two of them are the same grey
/// and the distinction the fifth was carrying is invisible.
pub const MAX_TONES_PER_SCREEN: usize = 4;

/// The most verbs an action bar will draw. See [`BarStyle::Actions`].
pub const MAX_ACTION_BAR_ACTIONS: usize = 3;

pub mod vector;

/// Grayscale values used by the built-in monochrome design system.
///
/// A GC16 refresh resolves sixteen levels and the middle ones ghost, so the
/// palette is deliberately tiny. Paper is pure white because that is the
/// panel's rest state.
pub mod tone {
    pub const PAPER: u8 = 255;
    pub const SURFACE: u8 = 232;
    pub const INK: u8 = 0;
    pub const MUTED: u8 = 96;
    pub const RULE: u8 = 160;
    /// The weaker of the two hairlines, for separating rows of a list from one
    /// another.
    ///
    /// This is the one place the palette carries the same role at two
    /// strengths, and it is deliberate rather than an oversight of the rule
    /// above. A line that divides a section from the next section and a line
    /// that divides one row from the row below it are not doing the same job,
    /// and drawn at the same weight the screen has no structure at all: a list
    /// of six entries under a top bar came out as seven identical rules and
    /// read as ruled notebook paper. Both platforms make the same split, iOS
    /// with `separator` against `opaqueSeparator` and Material with
    /// `outlineVariant` against `outline`.
    ///
    /// It does not count against [`MAX_TONES_PER_SCREEN`], because that limit
    /// is about how many *meanings* a screen asks the reader to tell apart, and
    /// these two mean the same thing.
    pub const RULE_LIGHT: u8 = 200;
    pub const FOCUS: u8 = 0;
}

// Same weight, no hierarchy: the line closing the top bar has to read as
// structural and the ones between entries as incidental. Asserted at compile
// time because both are constants and a test could only restate them.
const _: () = assert!(tone::RULE_LIGHT > tone::RULE);
const _: () = assert!(tone::RULE_LIGHT < tone::PAPER);

// A rule separates two things and wants to be quiet. A button's outline says
// where a finger may go and wants to be found. It does that with its tone
// rather than its thickness: ink against grey is already the whole of the
// difference, and adding weight on top of it made every button on the panel
// louder than the headline above it.
const _: () = assert!(CLARA_BW_METRICS.button_border() >= CLARA_BW_METRICS.rule_thickness());

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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Chrome {
    pub back: bool,
    /// What the runtime knows about the device, drawn above the top bar.
    ///
    /// `None` while reading and on any screen that has claimed the panel, so
    /// a book is a book and not a book with a clock on it.
    pub status: Option<Status>,
}

impl Chrome {
    #[must_use]
    pub const fn with_back(back: bool) -> Self {
        Self { back, status: None }
    }

    #[must_use]
    pub fn with_status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    /// The chrome to measure against when the real one is not known.
    ///
    /// An application is never told the clock, the signal or the battery, so
    /// a screen it measures for itself gets measured without them -- and the
    /// status band is laid out above everything else, so measuring without it
    /// hands the page a band of room that will not be there when it is drawn.
    /// Every application that paginates for itself made this mistake, and the
    /// symptom is always the same: a page that fits in the measurement and
    /// runs off the panel on the device.
    ///
    /// The values are representative rather than real. Only the height of the
    /// band matters, and that does not depend on what it says.
    #[must_use]
    pub fn measuring(back: bool) -> Self {
        Self {
            back,
            status: Some(Status {
                clock: "00:00".to_owned(),
                signal: Signal::Strong,
                battery: Some(Percent::new(50)),
                charging: false,
                // The busiest strip, so nothing measured against this comes
                // out narrower than the panel it will be drawn on.
                bluetooth: true,
            }),
        }
    }
}

/// How much radio there is.
///
/// Four states rather than a percentage. A number would invite an application
/// to draw its own bar, and the reader cannot act on the difference between
/// sixty and sixty-five percent, only on whether a page is going to load.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Signal {
    /// No radio, or no network. Drawn as a struck-through dot rather than as
    /// zero arcs, because zero arcs reads as a weak signal.
    #[default]
    Off,
    Weak,
    Fair,
    Strong,
}

impl Signal {
    /// The strength a link of this quality reports.
    ///
    /// The thresholds are the ones the stock reader uses, which matters
    /// because a reader who has learned what two arcs means on this device
    /// should not have to learn it again.
    #[must_use]
    pub const fn from_dbm(dbm: i32) -> Self {
        if dbm >= -60 {
            Self::Strong
        } else if dbm >= -70 {
            Self::Fair
        } else {
            Self::Weak
        }
    }
}

/// What the runtime knows about the device at the moment a screen is drawn.
///
/// Assembled by the daemon, never by an application: an application cannot
/// read the battery, cannot read the radio, and should not be able to claim a
/// stronger signal than there is. This is the same reasoning that makes the
/// way back the runtime's to draw.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Status {
    /// Already formatted, because what a time looks like is a question about
    /// the reader's locale and the layer that knows the answer is the one that
    /// read the clock. Empty means the device has no time worth showing.
    pub clock: String,
    pub signal: Signal,
    /// `None` when the battery could not be read, which is drawn as nothing
    /// rather than as zero: an empty battery and an unreadable one look the
    /// same and mean opposite things.
    pub battery: Option<Percent>,
    pub charging: bool,
    /// Whether something is connected right now, not whether the controller is
    /// powered. Drawn as a mark beside the radio so that the answer to "where
    /// is the sound going" is on every screen rather than only in Settings.
    pub bluetooth: bool,
}

/// Gives a screen a top bar to put the way back in, when it has none.
///
/// The way back is drawn in the top bar, so an application that did not ask
/// for one would otherwise trap the reader. The runtime supplies the bar
/// rather than trusting every application to remember, titled with the
/// application's own name so nothing is invented.
///
/// Here rather than in the daemon because the daemon has two renderers (the
/// panel and the host simulation) and only one of them was doing this. A
/// preview drawn without the way back is a preview of a screen that will never
/// exist, and it hides the one defect that leaves somebody stuck.
#[must_use]
pub fn ensure_way_back(mut screen: Screen, chrome: &Chrome, name: &str) -> Screen {
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
    /// Drawn instead of the label, when the thing it does has a picture
    /// everyone already knows.
    ///
    /// The label is still required and still carried: it is what the control
    /// is called, it is what a test or a preview reports, and a glyph nobody
    /// recognises with no word anywhere near it is how a panel becomes a
    /// puzzle. Only the drawing changes.
    pub glyph: Option<Glyph>,
}

/// What a grid cell is: part of a board, or a key.
///
/// A board is ruled squares, and the rules are the board. A keyboard is not:
/// forty-five outlined boxes is the noisiest thing on any screen that has one,
/// and none of the outlines carries information the position of the key does
/// not already give. A key is a quiet filled field instead, which is what both
/// phone platforms draw and what a printed keyboard looks like.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum CellStyle {
    #[default]
    Board,
    Key,
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
            glyph: None,
        }
    }

    /// Draws this control as a picture rather than as its label.
    #[must_use]
    pub const fn with_glyph(mut self, glyph: Glyph) -> Self {
        self.glyph = Some(glyph);
        self
    }
}

/// The fixed bar at the top of a screen.
///
/// Carries a title and at most [`MAX_BAR_ACTIONS`] actions. The cap is the
/// point: a bar that accepts a list of actions becomes a toolbar, and a
/// toolbar on a panel this size produces targets too small to hit. Back is not
/// a field here because it belongs to the runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopBar {
    pub id: NodeId,
    pub title: String,
    /// Right to left, in the order they were added. Capped rather than
    /// truncated silently at the far end: see [`Self::with_action`].
    pub actions: Vec<BarAction>,
}

/// How many controls a top bar may carry beside its title.
///
/// Two. One was too few the moment a reading screen wanted the type size and
/// the front light kept apart -- putting the light inside the type panel meant
/// a reader adjusting the brightness had the words they were judging it by
/// covered up. Three is a toolbar, and a toolbar leaves the title of a book
/// about forty pixels wide on this panel.
pub const MAX_BAR_ACTIONS: usize = 2;

impl TopBar {
    #[must_use]
    pub fn new(id: NodeId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            actions: Vec::new(),
        }
    }

    #[must_use]
    pub fn action(self, action: ActionId, label: impl Into<String>) -> Self {
        self.with_action(BarAction::new(action, label))
    }

    /// Adds a control, keeping the first [`MAX_BAR_ACTIONS`] of them.
    ///
    /// Dropped rather than refused, because the alternative to a bar missing
    /// its third control is a screen that does not draw at all, and the two
    /// that fit are the two the application asked for first.
    #[must_use]
    pub fn with_action(mut self, action: BarAction) -> Self {
        if self.actions.len() < MAX_BAR_ACTIONS {
            self.actions.push(action);
        }
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

/// A byline that can be tapped to hide or show what is underneath it.
///
/// # Why the count is carried and not written into the byline
///
/// "12 replies" only makes sense while the replies are hidden, so an
/// application that wrote it into the byline text would have to compose two
/// different strings and would end up with two different-length bylines
/// jumping as the reader folds and unfolds. The renderer knows which state it
/// is drawing, so it is the thing that should say so.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fold {
    /// Sent when the byline is tapped.
    pub action: ActionId,
    /// Whether what is underneath is currently hidden.
    pub collapsed: bool,
    /// How many replies are hidden. Only shown while collapsed.
    pub hidden: u16,
}

/// Which way a caret points.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Side {
    /// The caret is on top of the popover, so the anchor is above it.
    Up,
    /// The caret is underneath, so the anchor is below it.
    Down,
}

/// Something drawn over a screen, which the screen keeps underneath.
///
/// # Why this is not just more nodes
///
/// A dialogue built out of ordinary nodes replaces the screen: the reader
/// loses sight of what they were deciding about, and the application has to
/// rebuild the whole thing to dismiss it. Every real reader -- the stock one
/// included -- keeps the page underneath and puts the question on top of it.
///
/// # Why nothing is dimmed
///
/// The usual way to focus attention on an overlay is to darken everything
/// else. On this panel that is the single most expensive thing that could be
/// asked for: shading the whole screen changes every pixel, which forces a
/// full refresh with its black flash, and undoing it forces a second one. So
/// an overlay is separated by its own heavy border and by the paper it sits
/// on, not by spoiling what is behind it -- and the panel repaints only the
/// rectangle the overlay occupies.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Overlay {
    pub id: NodeId,
    pub kind: OverlayKind,
    /// Shown at the top of the overlay. Empty for a popover that is a bare
    /// list of choices, where a title is a line of chrome nobody reads.
    pub title: String,
    pub nodes: Vec<Node>,
}

/// Which of the two kinds of overlay this is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlayKind {
    /// Attached to the control that opened it, with a caret pointing at it.
    ///
    /// For choices that belong to a control: what a "..." offers, what tapping
    /// a size stepper reveals. Tapping anywhere else dismisses it, because a
    /// popover asks nothing and losing it costs the reader nothing.
    Popover { anchor: ActionId },
    /// Centred, and dismissed only by answering.
    ///
    /// For a question with consequences -- deleting something, replacing
    /// something. A tap outside is ignored rather than taken as an answer,
    /// because "somewhere else" is not one of the choices and a reader who
    /// brushes the panel has not decided anything.
    Modal,
}

impl Overlay {
    /// A popover hanging off `anchor`.
    #[must_use]
    pub fn popover(id: NodeId, anchor: ActionId, nodes: Vec<Node>) -> Self {
        Self {
            id,
            kind: OverlayKind::Popover { anchor },
            title: String::new(),
            nodes,
        }
    }

    /// A question that has to be answered.
    #[must_use]
    pub fn modal(id: NodeId, title: impl Into<String>, nodes: Vec<Node>) -> Self {
        Self {
            id,
            kind: OverlayKind::Modal,
            title: title.into(),
            nodes,
        }
    }

    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Whether a tap that misses the overlay dismisses it.
    #[must_use]
    pub const fn dismissed_by_a_miss(&self) -> bool {
        matches!(self.kind, OverlayKind::Popover { .. })
    }
}

/// What a bottom bar is for.
///
/// Android draws the same distinction and for the same reason: `NavigationBar`
/// carries destinations and marks the one you are looking at, `BottomAppBar`
/// carries verbs belonging to the screen and marks nothing. Sharing one type
/// here is deliberate -- the band, its hairline and its touch targets are
/// identical, and duplicating all of that to change one underline is how a
/// vocabulary doubles in size without saying anything new.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BarStyle {
    /// Places in the application. Exactly one of them is where you are, and
    /// the bar says which.
    #[default]
    Navigation,
    /// Verbs belonging to this screen. Nothing is marked, because none of them
    /// is a place you can be. Free to change from screen to screen, which
    /// destinations are not.
    Actions,
}

/// The fixed bar at the bottom of a screen, equivalent to the reader's own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavBar {
    pub id: NodeId,
    pub destinations: Vec<BarAction>,
    /// Which destination the reader is currently looking at, if any.
    ///
    /// `None` is a real answer rather than a missing one. A bar whose entries
    /// are *actions* (previous page, next page, the way out) has no current
    /// destination to mark, and marking one anyway tells the reader they are
    /// somewhere they are not. This used to be a plain `usize`, and the two
    /// screens that meant "none" said `usize::MAX`, which survived exactly as
    /// far as the wire: the byte saturated to 255 and the decoder clamped it
    /// to the last destination, so both screens shipped with the wrong entry
    /// underlined on the panel.
    pub selected: Option<usize>,
    /// Whether these are places or verbs. An action bar never marks a
    /// selection, whatever `selected` happens to say.
    pub style: BarStyle,
}

impl NavBar {
    #[must_use]
    pub fn new(id: NodeId, destinations: Vec<BarAction>, selected: Option<usize>) -> Self {
        Self {
            id,
            destinations,
            selected,
            style: BarStyle::Navigation,
        }
    }

    /// A bar of verbs rather than places.
    #[must_use]
    pub fn actions(id: NodeId, actions: Vec<BarAction>) -> Self {
        Self {
            id,
            destinations: actions,
            selected: None,
            style: BarStyle::Actions,
        }
    }

    /// The destinations that will actually be shown on a given panel.
    ///
    /// A bar with one destination is not navigation, and destinations narrower
    /// than a finger cannot be tapped, so the count is clamped to what the
    /// panel can physically carry rather than honoured blindly.
    #[must_use]
    pub fn visible(&self, metrics: &DisplayMetrics) -> &[BarAction] {
        let mut limit = min(self.destinations.len(), metrics.max_nav_destinations());
        if self.style == BarStyle::Actions {
            // Three verbs is the point at which a bar stops being read and
            // starts being searched. Beyond that the screen wants an overflow
            // menu, which the popover already provides.
            limit = min(limit, MAX_ACTION_BAR_ACTIONS);
        }
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
    /// A screen with one way off it (a launcher's way back to the reader, a
    /// dialogue's way out) has no navigation to draw, and a bar of one
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

    /// What a finger held still on the content area asks for.
    ///
    /// A tap and a hold are different intents on the same pixels, and a reading
    /// page has nothing but pixels: there is no control to press without
    /// covering the words. Held, rather than dragged out with two handles,
    /// because selecting a range by dragging on E Ink means chasing a caret
    /// that redraws a third of a second behind the finger.
    ///
    /// Like the page turns, this is a property of the screen and is whatever is
    /// left once every real control has been hit-tested, so holding a button
    /// still presses the button and holding under an overlay does nothing.
    pub hold: Option<ActionId>,

    /// Something drawn over this screen, with the screen kept underneath.
    ///
    /// Outside the node list for the same reason the bars are: a screen cannot
    /// carry two overlays, cannot bury one inside a card, and cannot place one
    /// halfway down the page.
    pub overlay: Option<Box<Overlay>>,
}

/// The actions a tap on the content area sends.
///
/// Some Kobo models also have physical page buttons. When those are wired up
/// they will send the same two actions, which is the reason this is a set of
/// intents rather than a set of touch zones.
///
/// `menu` is what a screen with no controls on it is reached by. A reading
/// screen deliberately carries nothing at the foot, and without a zone that
/// asks for them, every setting behind that bar is unreachable: type size,
/// front light, bookmarks and marked passages were all built, shipped, and
/// impossible to get at with a finger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageTurns {
    pub previous: ActionId,
    pub next: ActionId,
    pub menu: Option<ActionId>,
    /// Which page of how many, drawn centred beneath the content.
    ///
    /// Unlike the turn zones, which are free because they are the content area
    /// itself, this reserves one caption line at the foot of the panel. That
    /// is the honest cost and it is worth paying: the catalogue paginates up
    /// to fifty-four shelves and never told the reader which one they were on,
    /// so turning the page was indistinguishable from the list not moving.
    pub position: Option<(u16, u16)>,
}

impl PageTurns {
    #[must_use]
    pub const fn new(previous: ActionId, next: ActionId) -> Self {
        Self {
            previous,
            next,
            menu: None,
            position: None,
        }
    }

    /// The same, saying which page of how many this is.
    ///
    /// `page` is one-based. A total of zero, or a page past the total, draws
    /// nothing rather than "0 of 0": a count nobody can compute yet is better
    /// left unsaid than said wrongly.
    #[must_use]
    pub const fn with_position(mut self, page: u16, of: u16) -> Self {
        self.position = Some((page, of));
        self
    }

    /// The position to draw, once nonsense has been discarded.
    const fn drawable_position(self) -> Option<(u16, u16)> {
        match self.position {
            Some((page, of)) if of > 0 && page >= 1 && page <= of => Some((page, of)),
            _ => None,
        }
    }

    /// The same, with a middle column that asks for the reading controls.
    #[must_use]
    pub const fn with_menu(mut self, menu: ActionId) -> Self {
        self.menu = Some(menu);
        self
    }
}

/// The share of the width that goes back rather than forward.
///
/// A third, matching the stock reader. Forward is the common direction, so it
/// gets the larger share and the thumb that is already resting on the right
/// edge of the panel.
const BACK_ZONE: i32 = 3;

/// How many columns the content is divided into when there is a menu zone.
///
/// Three, the arrangement every other reader on this hardware uses: back on
/// the left, forward on the right, and the controls in the middle where
/// neither thumb rests. Turning a page keeps two thirds of the panel, which is
/// what it needs, and the third that is left is the only part of a reading
/// screen a reader has no other reason to touch.
const MENU_COLUMNS: i32 = 3;

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
            hold: None,
            overlay: None,
        }
    }

    /// Sends `action` when a finger is held still on the content area.
    #[must_use]
    pub const fn with_hold(mut self, action: ActionId) -> Self {
        self.hold = Some(action);
        self
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

    /// Draws `overlay` over this screen, keeping the screen underneath.
    #[must_use]
    pub fn with_overlay(mut self, overlay: Overlay) -> Self {
        self.overlay = Some(Box::new(overlay));
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
        self.layout_with(metrics, &Chrome::default())
    }

    /// Lays the screen out for a panel, including runtime-owned decoration.
    #[must_use]
    pub fn layout_with(&self, metrics: &DisplayMetrics, chrome: &Chrome) -> Layout {
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
        // The band sits above everything, including the top bar, which is
        // where the stock reader puts it and where a reader's eye already
        // goes. It is laid out first so the top bar starts below it rather
        // than being drawn over.
        let band = chrome
            .status
            .as_ref()
            .map_or(0, |status| layout_status_band(status, metrics, &mut layout));
        cursor = cursor.saturating_add(band);
        if let Some(top_bar) = &self.top_bar {
            cursor = layout_top_bar(top_bar, chrome, metrics, band, &mut layout);
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
            // A screen margin, the same one the sides get. Without a bar to
            // provide it the last control sat on the bezel: a button whose
            // border was the final row of pixels on the panel, and a keyboard
            // whose bottom row ran off the edge. `prose_area` has always
            // measured against this margin, so the engine not honouring it
            // also meant everything that paginates was handed a page taller
            // than the one that would be drawn.
            metrics.height - metrics.screen_margin()
        };
        // Reserved before any content is placed, for the same reason the bottom
        // bar is: a strip taken out from under a page that has already been set
        // is a strip that eats the last line of it.
        let position_band = if self
            .page_turns
            .is_some_and(|turns| turns.drawable_position().is_some())
            && self.overlay.is_none()
        {
            metrics.page_position_band()
        } else {
            0
        };
        let content_bottom = content_bottom - position_band;

        for (position, node) in self.nodes.iter().enumerate() {
            if layout.nodes.len() >= MAX_LAYOUT_NODES || cursor >= content_bottom {
                break;
            }
            // A splash centres itself in the band it is handed, and the band
            // it is handed is everything that is left. That is right when it
            // is the last thing on the screen and wrong the moment a recovery
            // button follows it, which is the usual shape: the button is
            // pushed off the bottom. So the band stops short of whatever
            // comes after.
            // Everything after this sits at the foot of the panel. Measured
            // with the same function a splash uses to stop short of what
            // follows it, and clamped so a full screen is not pulled upwards.
            if matches!(node, Node::Flex { .. }) {
                let trailing = trailing_height(
                    &self.nodes[position + 1..],
                    margin,
                    metrics.width - 2 * margin,
                    content_bottom,
                    metrics,
                    prose,
                    gap,
                );
                cursor = max(cursor, content_bottom.saturating_sub(trailing));
                continue;
            }
            let bottom = if matches!(node, Node::Splash { .. }) {
                content_bottom.saturating_sub(trailing_height(
                    &self.nodes[position + 1..],
                    margin,
                    metrics.width - 2 * margin,
                    content_bottom,
                    metrics,
                    prose,
                    gap,
                ))
            } else {
                content_bottom
            };
            cursor = layout_node(
                node,
                margin,
                cursor,
                metrics.width - 2 * margin,
                bottom,
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
        layout.hold = self.hold;
        if let Some((turns, (page, of))) = self
            .page_turns
            .filter(|_| position_band > 0)
            .and_then(|turns| turns.drawable_position().map(|shown| (turns, shown)))
        {
            let band = Rect {
                x: margin,
                y: content_bottom,
                width: metrics.width - 2 * margin,
                height: position_band,
            };
            layout.nodes.push(LayoutNode {
                id: NodeId(0),
                rect: band,
                kind: LayoutKind::PagePosition,
                text_lines: vec![format!("{page} of {of}")],
            });
            // Each chevron is offered only where there is a page on that side.
            // A control that promises a page which does not exist is worse
            // than no control: it answers a tap by doing nothing, and the
            // reader concludes the screen is stuck rather than finished.
            let side = min(metrics.touch_target_default(), band.width / 3);
            if page > 1 {
                layout.nodes.push(LayoutNode {
                    id: NodeId(0),
                    rect: Rect {
                        width: side,
                        ..band
                    },
                    kind: LayoutKind::PagePrevious(turns.previous),
                    text_lines: Vec::new(),
                });
            }
            if page < of {
                layout.nodes.push(LayoutNode {
                    id: NodeId(0),
                    rect: Rect {
                        x: band.x + band.width - side,
                        width: side,
                        ..band
                    },
                    kind: LayoutKind::PageNext(turns.next),
                    text_lines: Vec::new(),
                });
            }
        }
        layout.content = Rect {
            x: 0,
            y: content_top,
            width: metrics.width,
            height: max(0, content_bottom - content_top),
        };
        // Last, so it is on top of everything -- including the bars, which a
        // popover hanging off a top-bar control has to cover to be readable --
        // and so hit testing, which walks the list backwards, reaches it first.
        if let Some(overlay) = &self.overlay {
            layout_overlay(overlay, metrics, prose, &mut layout);
            // A popover cannot also be a page turn: the zones are whatever is
            // left of the content area, and "left over" must not include the
            // thing drawn over it.
            layout.page_turns = None;
            layout.hold = None;
        }
        layout
    }

    #[must_use]
    pub fn hit_test(&self, x: i32, y: i32) -> Option<ActionId> {
        self.layout().hit_test(x, y)
    }
}

/// Places an overlay over an already-laid-out screen.
///
/// Everything here is measured against what is already in `layout`, because a
/// popover's position is a fact about where its anchor ended up and cannot be
/// known before the screen has been laid out.
fn layout_overlay(overlay: &Overlay, metrics: &DisplayMetrics, prose: Face, layout: &mut Layout) {
    let margin = metrics.screen_margin();
    let padding = metrics.space(Space::Small);
    let gap = metrics.space(Space::Tight);
    // Never the full width. An overlay that reaches both margins is
    // indistinguishable from a new screen, which is the thing it exists not to
    // be: the reader has to be able to see that what they were looking at is
    // still there.
    let widest = min(metrics.width - 4 * margin, metrics.width * 5 / 6);
    // A modal takes all of that, because a modal is a dialogue and its prose
    // wants the room. A popover takes what it needs, because a popover is
    // usually a short menu: a box of that width holding the word "Delete" is a
    // band across the panel that looks like the screen changed rather than
    // like a mark was pressed. Bounded below so a one word menu is still a
    // comfortable target and still has room for its caret.
    let width = match overlay.kind {
        OverlayKind::Modal => widest,
        OverlayKind::Popover { .. } => {
            let narrowest = min(widest, 3 * metrics.touch_target_default());
            let title = if overlay.title.is_empty() {
                0
            } else {
                measure_text(&overlay.title, FontSize::Title).0
            };
            overlay
                .nodes
                .iter()
                .map(|node| intrinsic_width(node, widest - 2 * padding, metrics, prose))
                .chain(std::iter::once(title))
                .max()
                .unwrap_or(widest)
                .saturating_add(2 * padding)
                .clamp(narrowest, widest)
        }
    };
    let inner = width - 2 * padding;

    // Measured first, placed second. Where a popover goes depends on how tall
    // it turned out, so it is laid out into a scratch list at the origin and
    // then moved.
    // A modal is not dismissed by a tap that misses it, on purpose: it is the
    // shape used when an answer is actually required. That makes a way out
    // part of the frame rather than something each application has to
    // remember to put in a row, because the one that forgets has drawn a
    // screen with no way off it. A popover needs none -- a miss puts it away,
    // and a second cross beside the control that opened it is noise.
    let closes = matches!(overlay.kind, OverlayKind::Modal);
    let close = if closes {
        metrics.touch_target_default()
    } else {
        0
    };
    let mut scratch = Layout::default();
    let mut cursor = padding;
    let mut title_height = 0;
    if !overlay.title.is_empty() {
        title_height = FontSize::Title.line_height();
    }
    // The cross and the title share one band, so a modal with no title still
    // has room for the cross and one with a short title does not overlap it.
    let header = max(title_height, close);
    if header > 0 {
        cursor = cursor.saturating_add(header).saturating_add(gap);
    }
    for node in &overlay.nodes {
        if scratch.nodes.len() >= MAX_LAYOUT_NODES {
            break;
        }
        cursor = layout_node(
            node,
            padding,
            cursor,
            inner,
            // A popover is measured by its contents, so there is no band to
            // fill and a splash inside one would have nothing to centre in.
            metrics.height,
            0,
            metrics,
            prose,
            &mut scratch,
        );
        cursor = cursor.saturating_add(gap);
    }
    let height = min(cursor - gap + padding, metrics.height - 2 * margin);

    let caret = metrics.space(Space::Small);
    let (x, y, side) = match overlay.kind {
        OverlayKind::Modal => (
            (metrics.width - width) / 2,
            (metrics.height - height) / 2,
            None,
        ),
        OverlayKind::Popover { anchor } => {
            let target = layout
                .nodes
                .iter()
                .find(|node| node.kind.acts_on() == Some(anchor))
                .map(|node| node.rect);
            let Some(target) = target else {
                // An anchor that is not on the screen is an application bug,
                // and the least surprising thing to do with a popover that has
                // nothing to point at is to centre it rather than to drop it:
                // dropping it loses whatever it was going to say.
                return layout_overlay(
                    &Overlay {
                        kind: OverlayKind::Modal,
                        ..overlay.clone()
                    },
                    metrics,
                    prose,
                    layout,
                );
            };
            // Below the anchor if it fits, above it if that fits instead, and
            // otherwise on whichever side has more room, cut into the panel.
            //
            // The caret is decided from where the box actually ended up rather
            // than from which branch was taken. A panel too tall for either
            // side gets clamped into the middle of the screen, and the branch
            // that placed it there used to still claim a direction: the type
            // panel on a Clara hung from the top margin, covered its own
            // anchor, and drew a caret at the far bottom corner pointing down
            // at the page. A mark pointing at nothing is worse than no mark.
            let below = target.y + target.height + caret;
            let room_below = metrics.height - margin - below;
            let room_above = target.y - caret - margin;
            let y = if height <= room_below || room_below >= room_above {
                below
            } else {
                max(margin, target.y - caret - height)
            };
            let y = y.clamp(margin, max(margin, metrics.height - margin - height));
            let side = if y >= target.y + target.height {
                Some(Side::Up)
            } else if y + height <= target.y {
                Some(Side::Down)
            } else {
                None
            };
            let side = side.map(|side| (side, target));
            // Centred on the anchor, then pulled back inside the margins. The
            // caret stays with the anchor rather than with the box, which is
            // what makes an edge-anchored popover still point at the right
            // control.
            let wanted = target.x + (target.width - width) / 2;
            let x = wanted.clamp(margin, max(margin, metrics.width - margin - width));
            (x, y, side)
        }
    };

    // The scrim covers the panel and draws nothing. It exists so a tap that
    // misses the overlay is reported as a miss rather than reaching the screen
    // underneath, which would let a reader press a control they cannot see.
    layout.nodes.push(LayoutNode {
        id: overlay.id,
        rect: Rect {
            x: 0,
            y: 0,
            width: metrics.width,
            height: metrics.height,
        },
        kind: LayoutKind::Scrim {
            dismisses: overlay.dismissed_by_a_miss(),
        },
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id: overlay.id,
        rect: Rect {
            x,
            y,
            width,
            height,
        },
        kind: LayoutKind::Overlay,
        text_lines: Vec::new(),
    });
    if let Some((side, target)) = side {
        // Clamped so the caret cannot leave the box it belongs to, which is
        // what happens when a popover is pushed back inside the margin and its
        // anchor is right at the edge.
        let centre = target.x + target.width / 2;
        let caret_x = centre.clamp(x + padding, x + width - padding - caret);
        let caret_y = if side == Side::Up {
            y - caret
        } else {
            y + height
        };
        layout.nodes.push(LayoutNode {
            id: overlay.id,
            rect: Rect {
                x: caret_x - caret / 2,
                y: caret_y,
                width: caret,
                height: caret,
            },
            kind: LayoutKind::OverlayCaret(side),
            text_lines: Vec::new(),
        });
    }
    if !overlay.title.is_empty() {
        // Narrowed by whatever the cross took, so a long title is cut short
        // rather than set underneath it.
        let title_width = max(1, inner - close);
        layout.nodes.push(LayoutNode {
            id: overlay.id,
            rect: Rect {
                x: x + padding,
                y: y + padding + (header - title_height) / 2,
                width: title_width,
                height: title_height,
            },
            kind: LayoutKind::OverlayTitle,
            text_lines: wrap_text_in(&overlay.title, title_width, FontSize::Title, prose)
                .into_iter()
                .take(1)
                .collect(),
        });
    }
    if closes {
        layout.nodes.push(LayoutNode {
            id: overlay.id,
            rect: Rect {
                x: x + width - padding - close,
                y: y + padding,
                width: close,
                height: close,
            },
            kind: LayoutKind::OverlayClose,
            text_lines: Vec::new(),
        });
    }
    // Moved into place wholesale. Anything that fell past the bottom is left
    // out rather than drawn over the edge, which is the same rule the main
    // flow follows.
    let bottom = y + height - padding;
    for mut node in scratch.nodes {
        node.rect.x += x;
        node.rect.y += y;
        if node.rect.y + node.rect.height > bottom {
            continue;
        }
        if layout.nodes.len() >= MAX_LAYOUT_NODES {
            break;
        }
        layout.nodes.push(node);
    }
}

/// Lays out the strip carrying the clock, the radio and the battery.
///
/// Returns the height it took, which is zero when there is nothing worth
/// showing: a device with no clock, no battery reading and no radio would
/// otherwise give up five millimetres of a small panel to an empty strip.
fn layout_status_band(status: &Status, metrics: &DisplayMetrics, layout: &mut Layout) -> i32 {
    let height = metrics.status_band_height();
    let margin = metrics.screen_margin();
    layout.nodes.push(LayoutNode {
        id: NodeId(0),
        rect: Rect {
            x: 0,
            y: 0,
            width: metrics.width,
            height,
        },
        kind: LayoutKind::StatusBand,
        text_lines: Vec::new(),
    });
    if !status.clock.is_empty() {
        layout.nodes.push(LayoutNode {
            id: NodeId(0),
            rect: Rect {
                x: margin,
                y: 0,
                width: metrics.width / 3,
                height,
            },
            kind: LayoutKind::StatusClock,
            text_lines: vec![status.clock.clone()],
        });
    }
    // Marks are placed from the trailing edge inwards, so the battery is
    // outermost. That is the order every device this reader has used puts them
    // in, and the order is the only thing making them identifiable at this
    // size.
    let mark = height - 2 * metrics.space(Space::Tight);
    let gap = metrics.space(Space::Small);
    let mut right = metrics.width - margin;
    // Wider than tall: the design box is square, so a battery drawn into a
    // square would come out about three millimetres across and read as a dot.
    let battery_width = mark * 2;
    layout.nodes.push(LayoutNode {
        id: NodeId(0),
        rect: Rect {
            x: right - battery_width,
            y: metrics.space(Space::Tight),
            width: battery_width,
            height: mark,
        },
        kind: LayoutKind::StatusBattery(status.battery, status.charging),
        text_lines: Vec::new(),
    });
    right -= battery_width + gap;
    layout.nodes.push(LayoutNode {
        id: NodeId(0),
        rect: Rect {
            x: right - mark,
            y: metrics.space(Space::Tight),
            width: mark,
            height: mark,
        },
        kind: LayoutKind::StatusSignal(status.signal),
        text_lines: Vec::new(),
    });
    // Innermost, and only present when something is connected. Reserving the
    // space unconditionally would leave a hole beside the radio on every
    // screen for the sake of a mark that is usually not drawn.
    if status.bluetooth {
        right -= mark + gap;
        layout.nodes.push(LayoutNode {
            id: NodeId(0),
            rect: Rect {
                x: right - mark,
                y: metrics.space(Space::Tight),
                width: mark,
                height: mark,
            },
            kind: LayoutKind::StatusBluetooth,
            text_lines: Vec::new(),
        });
    }
    height
}

/// The size a screen's own title is set at, in the bar at the top.
///
/// Body, not [`FontSize::Title`]. The bar names the screen you are already
/// looking at; it is a label, not a headline, and at title size it was the
/// loudest thing on every page, louder than the first heading of the content
/// beneath it. Kobo's own reader sets its bar at about the size of its body
/// text for the same reason. An overlay's title keeps the larger size, because
/// an overlay has no content above it to compete with.
const BAR_TITLE: FontSize = FontSize::Body;

fn layout_top_bar(
    top_bar: &TopBar,
    chrome: &Chrome,
    metrics: &DisplayMetrics,
    top: i32,
    layout: &mut Layout,
) -> i32 {
    let margin = metrics.screen_margin();
    let height = metrics.top_bar_height();
    let width = metrics.width - 2 * margin;
    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: 0,
            y: top,
            width: metrics.width,
            height,
        },
        kind: LayoutKind::TopBar,
        text_lines: Vec::new(),
    });

    // Never taller than the bar it sits in. The comfortable control default is
    // ten millimetres and the bar is eight and a half, so taken literally this
    // put a control that overhangs its own bar at a negative offset, the back
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
                y: top + (height - control) / 2,
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
    // Right to left, in the order they were added, so adding a control never
    // moves the one that was already there: a reader who knows where "Aa" is
    // must still find it there on the next screen that carries it.
    let mut action_right = metrics.width - margin;
    for action in top_bar.actions.iter().take(MAX_BAR_ACTIONS) {
        let action_width = if action.glyph.is_some() {
            // Square. A picture has no natural width to measure, and a target
            // narrower than the bar is tall is one a thumb misses.
            control
        } else {
            let (text_width, _) = measure_text(&action.label, FontSize::Body);
            max(
                control,
                text_width.saturating_add(metrics.space(Space::Medium)),
            )
        };
        layout.nodes.push(LayoutNode {
            id: top_bar.id,
            rect: Rect {
                x: action_right - action_width,
                y: top + (height - control) / 2,
                width: action_width,
                height: control,
            },
            kind: match action.glyph {
                Some(glyph) => LayoutKind::BarGlyph(action.action, glyph),
                None => LayoutKind::BarAction(action.action),
            },
            text_lines: vec![action.label.clone()],
        });
        let taken = action_width.saturating_add(metrics.space(Space::Small));
        action_right -= taken;
        title_width = title_width.saturating_sub(taken);
    }

    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: title_x,
            y: top + (height - BAR_TITLE.line_height()) / 2,
            width: max(0, title_width),
            height: BAR_TITLE.line_height(),
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
        text_lines: vec![one_line(&top_bar.title, title_width, BAR_TITLE)],
    });

    layout.nodes.push(LayoutNode {
        id: top_bar.id,
        rect: Rect {
            x: 0,
            y: top + height,
            width: metrics.width,
            height: metrics.rule_thickness(),
        },
        kind: LayoutKind::Divider,
        text_lines: Vec::new(),
    });
    // Where the bar ends, not how tall it is. Returning the height was right
    // only while nothing was ever placed above the bar: the caller assigns
    // this to its cursor, so with a status band drawn the bar moved down by
    // the band and the content did not, and the first row of a grid was laid
    // out underneath the title. It never showed in simulation, because the
    // simulator drew no band and a band of zero makes the two the same
    // number.
    top.saturating_add(height)
        .saturating_add(metrics.rule_thickness())
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
    let label = one_line(&bottom.action.label, width - 32, FontSize::Body);
    layout.nodes.push(LayoutNode {
        id: bottom.id,
        rect: Rect {
            x: margin,
            y,
            width,
            height,
        },
        kind: LayoutKind::Button(
            bottom.action.action,
            ControlState::Enabled,
            Emphasis::Normal,
        ),
        text_lines: vec![label.clone()],
    });
    // The mark sits beside the centred word rather than replacing it. A bar
    // this wide has room for both, and the band is often the only way off a
    // screen, so it is the last place to make somebody guess.
    if let Some(glyph) = bottom.action.glyph {
        let (text_width, _) = measure_text(&label, FontSize::Body);
        let side = height * 2 / 5;
        let gap = metrics.space(Space::Small);
        let text_left = margin + (width - text_width) / 2;
        layout.nodes.push(LayoutNode {
            id: bottom.id,
            rect: Rect {
                x: text_left.saturating_sub(gap + side).max(margin),
                y: y + (height - side) / 2,
                width: side,
                height: side,
            },
            kind: LayoutKind::InlineGlyph(glyph),
            text_lines: Vec::new(),
        });
    }
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
            kind: if nav_bar.style == BarStyle::Navigation && nav_bar.selected == Some(index) {
                LayoutKind::NavDestinationSelected(destination.action, destination.glyph)
            } else {
                LayoutKind::NavDestination(destination.action, destination.glyph)
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
    /// A line about the content rather than the content: a date, an author, a
    /// size, a count, a status.
    ///
    /// Every screen in the platform set all of its prose at one size in one
    /// tone, so a heading was followed by an undifferentiated column in which
    /// "Downloading" and the third paragraph of a novel carried identical
    /// weight. The stock reader never does this: metadata is smaller and
    /// lighter than what it describes, which is what lets a list be read by
    /// scanning the titles alone. This is a separate node rather than a role on
    /// [`Self::Text`] because it is never prose: it is not paginated, not set
    /// in the reading face, and never wraps to more than a line or two.
    Secondary {
        id: NodeId,
        text: String,
    },
    /// A labelled break in the flow: the name of the group that follows it.
    ///
    /// Every application was hand-building this out of a divider, a spacer and
    /// a line of prose, and getting a slightly different answer each time: the
    /// component gallery alone stacked twelve spacers, which is what a missing
    /// primitive looks like from the outside. Using [`Self::Heading`] instead
    /// is worse, because a heading is display type belonging to the *screen*,
    /// so a screen with four of them has four titles and no hierarchy at all.
    ///
    /// The title is set at caption weight in the muted tone with a hairline
    /// running to the right margin, which is quiet enough that the screen's own
    /// heading still wins. Nothing here transforms the words: setting a section
    /// in capitals is a house style that breaks on scripts with no case and
    /// turns a Turkish dotted i into the wrong letter, so the application
    /// supplies the capitals it wants.
    ///
    /// A section is bound to whatever follows it during pagination. A header
    /// stranded at the foot of a page with its content overleaf is the single
    /// most common way a paginated layout reads as broken.
    Section {
        id: NodeId,
        title: String,
        /// A count or a total, set against the right margin. The hairline is
        /// measured to stop short of it rather than run underneath it.
        value: Option<String>,
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
        /// Whether this paragraph is what was said, or who said it.
        role: QuoteRole,
        text: String,
        /// Set on a byline to make it a handle for showing and hiding the
        /// replies underneath it. Ignored on a body: a paragraph is not a
        /// thing that can be folded away, only the comment it belongs to is.
        fold: Option<Fold>,
    },
    Button {
        id: NodeId,
        action: ActionId,
        label: String,
        state: ControlState,
        /// Whether this is the one thing the screen is for. See [`Emphasis`].
        emphasis: Emphasis,
    },
    Card {
        id: NodeId,
        children: Vec<Node>,
    },
    /// A bordered line showing what has been typed into it.
    ///
    /// Tapping it yields the action and the application routes to its own
    /// keyboard screen; the field does not summon a keyboard itself, because
    /// the runtime does not own one. What it does own is the ability to show
    /// the current contents, which is the part no existing node could do: a
    /// search entry point had to be a full-width button or a nav-bar slot, and
    /// neither can display the query it would be editing.
    Field {
        id: NodeId,
        action: ActionId,
        /// What has been typed. Empty falls back to `placeholder`, muted.
        value: String,
        placeholder: String,
        /// The cross that empties it. Absent when there is nothing to empty.
        clear: Option<ActionId>,
    },
    /// A wrapping run of short tappable labels: facets, subjects, filters.
    ///
    /// Distinct from [`Self::Choice`], which is capped at
    /// [`MAX_CHOICE_OPTIONS`] and is a full-width vertical question. A subject
    /// cloud is neither vertical nor a question, and zero or many of it may be
    /// on at once. The renderer owns the wrapping and the truncation; the
    /// application supplies no geometry.
    Chips {
        id: NodeId,
        /// Entries past [`MAX_CHIPS`] are dropped and reported.
        chips: Vec<Chip>,
    },
    /// Up to [`MAX_TABS`] peer views of one destination.
    ///
    /// A nav bar is pinned to the bottom and models *destinations*. Discover,
    /// Popular and Subjects are filters on one destination rather than three
    /// destinations, and until this existed there was no way to say so — so
    /// applications said it with a nav bar and the reader was told they had
    /// left the screen they were on.
    Tabs {
        id: NodeId,
        tabs: Vec<Chip>,
        /// Exactly one is always current. Out of range is clamped to the first
        /// rather than refused, for the reason [`NavBar`] gives.
        selected: usize,
    },
    /// A set of facts about the thing on the screen: labels and their values.
    ///
    /// The shape every detail screen needs and none could express. Given only
    /// [`Self::Secondary`], an application with a dozen facts to show stacks a
    /// dozen grey paragraphs, which is why the catalogue's book page held the
    /// whole of a Gutendex record in memory and displayed none of it.
    ///
    /// Not a [`Self::Band`] per row, because the labels share one column width
    /// measured across every row at once, and separate bands would each
    /// measure their own and step raggedly down the panel. That shared
    /// measurement is the entire difference between a definition list and two
    /// columns of text.
    Facts {
        id: NodeId,
        /// Label and value. Entries past [`MAX_FACTS`] are dropped rather than
        /// set, and reported by [`Screen::validate`].
        entries: Vec<(String, String)>,
    },
    /// Two or three blocks placed beside each other rather than stacked.
    ///
    /// The layout engine is otherwise a pure downward flow: every node takes
    /// the full width and the next one starts underneath it. That is right for
    /// reading and wrong for the handful of things that are only legible side
    /// by side -- a cover beside its title and author, a label beside its
    /// value, a name beside its count. Without this each of those had to
    /// become a bespoke node with its own hand-written "measure the right hand
    /// thing, then clamp the left against what is left" arithmetic, and that
    /// arithmetic was being written a fourth time before this existed.
    ///
    /// Deliberately not a flexbox. Slots are capped at [`MAX_BAND_SLOTS`],
    /// widths are a token rather than a number, alignment is a token rather
    /// than a number, and there is no nesting budget beyond the one the layout
    /// engine already enforces. An application still cannot express a bad
    /// arrangement: when the slots cannot each keep
    /// [`MIN_BAND_SLOT_TENTH_MM`], the band stacks itself and says nothing
    /// about it, because a stacked column is always readable and a four
    /// character column never is.
    Band {
        id: NodeId,
        align: BandAlign,
        slots: Vec<BandSlot>,
    },
    Divider {
        id: NodeId,
    },
    Spacer {
        id: NodeId,
        space: Space,
    },
    /// Every node after this one is pushed to the foot of the content area.
    ///
    /// The keyboard is why. It is the tallest thing any screen draws and it
    /// belongs under the thumbs, but it is placed in flow like everything
    /// else, so a compose screen with two lines above it put a keyboard in the
    /// middle of the panel with five hundred pixels of paper below it. Both
    /// phone platforms anchor it, and so does the stock reader.
    ///
    /// It fills rather than centres, so a screen that is already full is
    /// unchanged: the nodes after it never move up, only down.
    Flex {
        id: NodeId,
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
    /// This is the general one. A tile grid is opinionated (it picks its own
    /// column count for readability and expects an icon and a word) and that
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
    /// A mark, a name and a sentence, centred in whatever room is left.
    ///
    /// The one node that centres, and the one node that takes the rest of the
    /// screen, because that is the whole shape it exists to draw: the moment
    /// between asking for something and it arriving. Everything else in this
    /// system flows from the top and is set ranged left, which is correct for
    /// reading and wrong for a screen with four words on it -- those land in
    /// the top corner and read as a page that failed to load.
    ///
    /// It is last in the flow by construction: it eats the remaining band, so
    /// anything after it has nowhere to go.
    Splash {
        id: NodeId,
        glyph: Option<Glyph>,
        title: String,
        summary: String,
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
    /// change (that is what makes the model simple) and a book cover is eighty
    /// thousand bytes, so carrying them inline would put a cover on the wire
    /// for every tap. Instead the application hands the picture over once and
    /// refers to it afterwards by `handle`.
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
        /// Bytes received, and the total when the server admitted one.
        ///
        /// Formatted here rather than by the application, which is the whole
        /// point of putting it on the node: six of the nine example apps
        /// hand-rolled a byte formatter and no two agreed on whether a
        /// kilobyte was 1000 or 1024. A total of `None` means the length was
        /// never announced, so the bar stays indeterminate and the reader is
        /// told what has arrived rather than a percentage nobody can compute.
        transferred: Option<(u64, Option<u64>)>,
        /// Why it stopped, and whether asking again is worth anything.
        ///
        /// A failed activity keeps whatever is around it on screen. Replacing
        /// the screen with an error is how the catalogue lost the book you
        /// were looking at because its cover failed to decode.
        failure: Option<TransferFailure>,
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

/// How a band's slots line up against each other across their shared height.
///
/// A token rather than a number, and only three of them, because the cases
/// worth naming are: metadata that starts level with the top of the picture it
/// describes, a value centred against a taller label, and a caption that sits
/// on the foot of what it captions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BandAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// How much of a band a slot asks for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SlotWidth {
    /// As wide as the content measures, and no wider. A count, a label, a
    /// short verb.
    Natural,
    /// Whatever is left once the natural and fixed slots have been served.
    /// Two fills share the remainder evenly.
    #[default]
    Fill,
    /// A physical width in tenths of a millimetre, so a cover is the same size
    /// on every panel rather than the same number of pixels.
    Fixed(u16),
}

/// One column of a [`Node::Band`].
///
/// A column rather than a single node, because the thing beside a cover is
/// never one node: it is a title, an author and two facts, stacked. Inside a
/// slot the ordinary downward flow resumes, so a slot is the point at which
/// the engine goes back to being the engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BandSlot {
    pub width: SlotWidth,
    pub nodes: Vec<Node>,
}

impl BandSlot {
    #[must_use]
    pub fn new(width: SlotWidth, nodes: Vec<Node>) -> Self {
        Self { width, nodes }
    }

    /// A slot that takes what the content measures.
    #[must_use]
    pub fn natural(nodes: Vec<Node>) -> Self {
        Self::new(SlotWidth::Natural, nodes)
    }

    /// A slot that takes what is left.
    #[must_use]
    pub fn fill(nodes: Vec<Node>) -> Self {
        Self::new(SlotWidth::Fill, nodes)
    }

    /// A slot at a physical width, in tenths of a millimetre.
    #[must_use]
    pub fn fixed(tenths_mm: u16, nodes: Vec<Node>) -> Self {
        Self::new(SlotWidth::Fixed(tenths_mm), nodes)
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
    /// and a half of shelf between the bars, the third row was cut in half by
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
    /// What has happened to the thing the tile stands for.
    pub state: TileState,
    /// A count, or a word of at most [`TILE_BADGE_LIMIT`] characters, set in
    /// the tile's leading corner. Empty means no badge.
    pub badge: String,
    /// A second line under the name: an author, an owner, a size. Empty means
    /// no second line, and a grid in which no tile has one is set at the same
    /// height it was before.
    pub subtitle: String,
}

/// What has become of the thing a tile stands for.
///
/// This exists because applications kept writing it into the label instead.
/// Gutenbird set `format!("{title} (kept)")`, which is a sentence in a name's
/// place: it lengthens the label until it ellipsises, it cannot be translated,
/// it cannot be drawn differently, and it means the tile's own text is no
/// longer the thing's own text. A state is a fact about the tile, so the tile
/// carries it and the renderer decides how it looks.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TileState {
    /// Nothing to say. The tile is its mark and its name.
    #[default]
    Normal,
    /// The device already has it. A tick in the trailing corner.
    Held,
    /// Known, listed, and not obtainable at the moment. Marked with a cross
    /// and, unlike the other three, not tappable: a destination that cannot be
    /// reached must not answer a tap, or the application is obliged to explain
    /// the refusal on a screen the reader did not ask for.
    Unavailable,
    /// Something is happening to it right now. A clock in the trailing corner.
    /// Still tappable, because a tap during a download is how a reader asks
    /// what the download is doing.
    Busy,
}

impl TileState {
    /// The mark drawn in the tile's trailing corner, if any.
    #[must_use]
    pub const fn glyph(self) -> Option<Glyph> {
        match self {
            Self::Normal => None,
            Self::Held => Some(Glyph::Check),
            Self::Unavailable => Some(Glyph::Close),
            Self::Busy => Some(Glyph::Clock),
        }
    }

    /// Whether a tile in this state answers a tap.
    #[must_use]
    pub const fn is_tappable(self) -> bool {
        !matches!(self, Self::Unavailable)
    }
}

/// The most characters a tile badge is allowed. Past this it is a label, and a
/// label belongs on the label line where it can be wrapped and measured.
pub const TILE_BADGE_LIMIT: usize = 4;

impl Tile {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>, glyph: Glyph) -> Self {
        Self {
            action,
            label: label.into(),
            glyph,
            picture: None,
            state: TileState::Normal,
            badge: String::new(),
            subtitle: String::new(),
        }
    }

    #[must_use]
    pub fn with_picture(mut self, picture: TilePicture) -> Self {
        self.picture = Some(picture);
        self
    }

    #[must_use]
    pub fn with_state(mut self, state: TileState) -> Self {
        self.state = state;
        self
    }

    #[must_use]
    pub fn with_badge(mut self, badge: impl Into<String>) -> Self {
        self.badge = badge.into();
        self
    }

    #[must_use]
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = subtitle.into();
        self
    }
}

/// One square of a [`Node::Grid`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub action: ActionId,
    pub label: String,
    /// Drawn above the label, centred, when the action has a picture everyone
    /// already knows. Optional because most cells do not: a glyph invented for
    /// a verb nobody draws is worse than the verb written out.
    pub glyph: Option<Glyph>,
}

impl Cell {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
            glyph: None,
        }
    }

    /// Gives this cell a picture to sit above its label.
    ///
    /// The label stays. A transport control drawn as a bare triangle is
    /// unambiguous to anyone who has used a tape recorder and silent to a
    /// screen reader, and "back thirty seconds" is a fact only the words can
    /// carry.
    #[must_use]
    pub const fn with_glyph(mut self, glyph: Glyph) -> Self {
        self.glyph = Some(glyph);
        self
    }
}

/// One tappable label in a [`Node::Chips`] run or a [`Node::Tabs`] strip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Chip {
    pub action: ActionId,
    pub label: String,
    /// Drawn inverted. Meaningless for a tab, which takes its selection from
    /// [`Node::Tabs::selected`] so that exactly one can ever be current.
    pub selected: bool,
}

impl Chip {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
            selected: false,
        }
    }

    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

/// The most chips one run will lay out. Past this it is a list, and a list is
/// [`Node::Rows`].
pub const MAX_CHIPS: usize = 16;

/// The most tabs one strip will lay out.
///
/// Four, because they share the panel's width equally and a fifth on the Clara
/// leaves each one narrower than the two-word label most of them carry.
pub const MAX_TABS: usize = 4;

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
    /// A short value against the right edge: a score, a size, a date, a count.
    ///
    /// Carried rather than written into the title, for the reason every other
    /// state on this type is: a title that ends in its own score cannot be
    /// clamped without eating the score, and cannot be right aligned at all.
    /// The value is measured first and the title is clamped against what is
    /// left, never the other way round.
    pub trailing: Option<String>,
    /// A second thing this row can be asked to do, drawn as a vertical three
    /// dot mark against the right edge and hit-tested ahead of the row itself.
    ///
    /// A row has one obvious verb: open it. Everything else a reader might
    /// want to do to the thing a row names -- remove it, rename it, stop
    /// following it -- has nowhere to live on a panel with no long press worth
    /// relying on and no room for a second button. This is that place, and it
    /// is deliberately one action rather than a menu, because what opens is
    /// the application's business: a popover, a confirmation, another screen.
    pub menu: Option<ActionId>,
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
            trailing: None,
            menu: None,
        }
    }

    /// The same, with a short value set against the right edge.
    #[must_use]
    pub fn with_trailing(mut self, value: impl Into<String>) -> Self {
        self.trailing = Some(value.into());
        self
    }

    /// The same, with an overflow mark against the right edge naming `action`.
    ///
    /// The mark keeps a finger's width of the row to itself and the title
    /// wraps into what is left, on the same reasoning as a trailing value.
    #[must_use]
    pub fn with_menu(mut self, action: ActionId) -> Self {
        self.menu = Some(action);
        self
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
/// the well holds a number instead, the same thing Hacker News itself puts
/// there. `From<Glyph>` exists so that the icon case, which is still the right
/// answer for a menu of unlike things, stays the shortest thing to write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowLead {
    Icon(Glyph),
    /// The row's position in an ordered list, drawn as digits.
    Number(u16),
    /// A cover, letterboxed into the lead square, with a glyph for when the
    /// handle has not arrived or has been evicted from the cache.
    ///
    /// Without this a list of books is either a grid with no room for
    /// metadata, or a list with the same generic book icon on every row --
    /// which SDK.md itself calls out as a whole touch target's width saying
    /// nothing.
    Picture(TilePicture, Glyph),
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

/// Why a transfer stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferFailure {
    /// What went wrong, in the reader's terms rather than the protocol's.
    pub reason: String,
    /// Whether the same request would be worth making again. A retry offered
    /// for something that can never succeed is a control that teaches readers
    /// the controls do not work.
    pub resumable: bool,
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
    /// A disc with rays: the front light, drawn as brightness is drawn
    /// everywhere, including on the reader this replaces.
    Light,
    /// Two crossed strokes: shut this. Drawn by the frame on every modal, and
    /// available to an application that wants one of its own.
    Close,
    /// An arrow into a tray: fetch this, or the transfer that is fetching it.
    Download,
    /// A ribbon with a notch cut from its foot: kept, saved, come back to.
    Bookmark,
    /// A funnel: narrow what is listed. Distinct from [`Self::Search`], which
    /// finds something not yet on screen; a filter subtracts from what is.
    Filter,
    /// A head and shoulders: an author, a submitter, an account.
    Person,
    /// A label with a hole punched in it: a subject, a category, a topic.
    Tag,
    /// A sphere with a meridian and a parallel: a language, a source, a domain.
    Globe,
    /// Two arrows chasing each other: fetch it again. Three applications drew
    /// this as the word "Reload" in a button before it existed.
    Refresh,
    /// Three dots in a row: whatever else this bar would have offered if it
    /// had more than [`MAX_BAR_ACTIONS`] places to offer it in.
    More,
    /// The Bluetooth rune. Settings drew Bluetooth with the gear before this
    /// existed, which made the one row about headphones look like a link back
    /// to the screen it was already on.
    Bluetooth,
    /// A key: a credential, a secret, a permission that has to be installed
    /// rather than granted. The permission state drew a head and shoulders
    /// before this existed, which read as an account problem when the usual
    /// cause is an API key nobody has put on the device yet.
    Key,
    /// A horseshoe magnet, poles down. The hall sensor behind the bezel has no
    /// natural picture, so this is the thing you hold against it rather than
    /// the thing that does the sensing.
    Magnet,
    /// A right-pointing triangle: start playing. Every transport control on
    /// every device made since the tape recorder draws this, which is why the
    /// audiobook player is the one place a picture beats a word.
    Play,
    /// Two upright bars: stop, but keep the place. Distinct from a square,
    /// which is stop and forget it, and which this platform has no use for.
    Pause,
    /// An arrow curling anticlockwise around the numeral 30: go back half a
    /// minute. The number is inside the glyph rather than in a label beside
    /// it, because these controls are drawn without words and an arrow on its
    /// own cannot say how far it goes.
    Rewind30,
    /// The same, curling clockwise: go forward half a minute.
    Forward30,
    /// A speaker cone with a minus: quieter.
    VolumeDown,
    /// A speaker cone with a plus: louder.
    VolumeUp,
    /// The same three dots stood on end: what else can be done to the one
    /// thing this row names. Distinct from [`Self::More`], which belongs to a
    /// bar and speaks for the whole screen.
    MoreVertical,
    /// A bin: remove this, and mean it. Feeds spelled the same verb out in a
    /// sentence before this existed, which made the one destructive thing in
    /// the menu the longest line in it.
    Trash,
    /// A chevron pointing back the way the reader came: the page before this
    /// one. The same mark the top bar's own Back is cut from, offered as a
    /// glyph so that a list which pages can say which way it goes without
    /// spending a bar on two words.
    Previous,
    /// The chevron the other way: the page after this one.
    Next,
    /// Add. The one verb on this device drawn often enough that the mark is
    /// read faster than the word, and the word for it differs by screen:
    /// "Add", "Add a feed", "New item".
    Plus,
    /// A band over two cups: listening rather than reading. The audiobook
    /// application wore [`Self::Download`] before this existed, which said
    /// "fetch something" about the one place on the device that plays sound.
    Headphones,
}

impl Glyph {
    /// Every glyph in the set, in wire order.
    ///
    /// Exhaustive by construction and asserted to stay that way, because the
    /// hand-written list this replaces had drifted to nineteen entries while
    /// the set was twenty-one: `Light` and `Close` were authored, shipped, and
    /// covered by none of the tests that walk every glyph. A glyph nobody
    /// rasterises in a test is a blank space beside a label on the panel.
    pub const ALL: [Self; 44] = [
        Self::App,
        Self::Book,
        Self::Note,
        Self::Clock,
        Self::Settings,
        Self::Folder,
        Self::Chart,
        Self::Search,
        Self::Wifi,
        Self::Battery,
        Self::Reader,
        Self::Power,
        Self::Grid,
        Self::Circle,
        Self::Check,
        Self::Terminal,
        Self::Chat,
        Self::News,
        Self::Rss,
        Self::Light,
        Self::Close,
        Self::Download,
        Self::Bookmark,
        Self::Filter,
        Self::Person,
        Self::Tag,
        Self::Globe,
        Self::Refresh,
        Self::More,
        Self::Bluetooth,
        Self::Key,
        Self::Magnet,
        Self::Play,
        Self::Pause,
        Self::Rewind30,
        Self::Forward30,
        Self::VolumeDown,
        Self::VolumeUp,
        Self::MoreVertical,
        Self::Trash,
        Self::Previous,
        Self::Next,
        Self::Plus,
        Self::Headphones,
    ];
}

impl Node {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        match self {
            Self::Heading { id, .. }
            | Self::Text { id, .. }
            | Self::Secondary { id, .. }
            | Self::Section { id, .. }
            | Self::Quote { id, .. }
            | Self::Button { id, .. }
            | Self::Card { id, .. }
            | Self::Field { id, .. }
            | Self::Chips { id, .. }
            | Self::Tabs { id, .. }
            | Self::Band { id, .. }
            | Self::Facts { id, .. }
            | Self::Divider { id }
            | Self::Spacer { id, .. }
            | Self::Flex { id, .. }
            | Self::Progress { id, .. }
            | Self::Splash { id, .. }
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

/// A byte count in the units a reader uses.
///
/// Powers of two with the customary single-letter suffixes, and one decimal
/// place below ten so that a 4.2 MB book does not round to "4 MB" and sit
/// there looking stuck while a megabyte of it arrives. Formatted here because
/// six example applications each wrote their own and no two agreed.
#[must_use]
pub fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    // Integer throughout. A float here would have been shorter and would have
    // rendered a 16 GB card as "16.000000000000004 GB" on some rounding modes,
    // which is exactly the sort of detail an e-reader has no way to hide.
    let mut scale = 1u64;
    let mut unit = 0;
    while bytes / scale >= 1024 && unit + 1 < UNITS.len() {
        scale *= 1024;
        unit += 1;
    }
    if unit == 0 {
        return format!("{bytes} B");
    }
    let whole = bytes / scale;
    if whole < 10 {
        // Round to one decimal place, half-up.
        let tenths = (bytes % scale * 10 + scale / 2) / scale;
        let (whole, tenths) = if tenths >= 10 {
            (whole + 1, 0)
        } else {
            (whole, tenths)
        };
        format!("{whole}.{tenths} {}", UNITS[unit])
    } else {
        format!("{} {}", (bytes + scale / 2) / scale, UNITS[unit])
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
    /// A byline that hides or shows what is underneath it. Carries the action
    /// to send and whether it is currently folded shut.
    QuoteFold(ActionId, bool),
    /// The card an overlay is drawn on.
    Overlay,
    /// The title inside an overlay.
    OverlayTitle,
    /// The cross that shuts a modal. Answers with [`ActionId::BACK`], which is
    /// what a miss on a popover already answers with, so an application that
    /// handles one handles both.
    OverlayClose,
    /// The triangle joining a popover to the control that opened it. The value
    /// is which way it points, which is decided by whether the popover ended
    /// up below its anchor or above it.
    OverlayCaret(Side),
    /// Everything an overlay is not covering.
    ///
    /// Emitted so a tap that misses a popover can be told apart from a tap on
    /// what is behind it. It draws nothing: the panel is not dimmed. `dismisses`
    /// says whether missing means "put it away" or means nothing at all.
    Scrim {
        dismisses: bool,
    },
    /// The strip above the top bar. Emitted only by the runtime.
    StatusBand,
    /// The time, at the leading edge of the band.
    StatusClock,
    /// The radio, drawn at the strength the runtime measured.
    StatusSignal(Signal),
    StatusBluetooth,
    /// The battery, drawn at the level the runtime read, and whether it is on
    /// the charger. `None` means unreadable, which is drawn as nothing.
    StatusBattery(Option<Percent>, bool),
    Heading,
    Text,
    /// A line of metadata: smaller than the body, and in the muted tone.
    Secondary,
    /// The name of a group, with a hairline to the right margin and an
    /// optional count against it. The value is carried in `text_lines` as the
    /// title followed by the count, so the drawing pass needs no tree.
    Section,
    /// An indented paragraph. The values are the clamped depth and what the
    /// paragraph is for, so the renderer can draw the gutter rules and pick a
    /// size without consulting the tree.
    Quote(u8, QuoteRole),
    Button(ActionId, ControlState, Emphasis),
    Card,
    /// The extent of a horizontal group. Draws nothing itself: it exists so a
    /// repaint can dirty the whole group rather than each column.
    Band,
    /// A row's right-aligned value. Muted, and never part of the title.
    RowTrailing,
    /// A row's overflow mark: a finger-wide target against the right edge,
    /// hit-tested ahead of the row it sits in because it is pushed after it.
    RowMenu(ActionId),
    /// A text field's border and its tap target.
    Field(ActionId),
    /// What is in the field, or its placeholder when it is empty. The bool is
    /// which, because the two are drawn in different tones and the drawing
    /// pass must not have to consult the tree to find out.
    FieldValue(bool),
    /// The cross that empties a field.
    FieldClear(ActionId),
    /// One chip. `selected` chips are drawn inverted, which on a panel with
    /// two usable tones is the only unambiguous way to say "on".
    Chip(ActionId, bool),
    /// One tab. `selected` takes ink and a doubled rule beneath it; the rest
    /// are muted with no rule.
    Tab(ActionId, bool),
    /// The hairline the whole tab strip sits on, so an unselected tab still
    /// reads as attached to the content below it.
    TabRule,
    /// "4 of 12", centred under the content. Muted: it answers a question the
    /// reader only asks occasionally and must not compete with the page.
    PagePosition,
    /// The chevrons either side of the position, and the only visible sign a
    /// screen turns at all.
    ///
    /// The side-tap zones are how a Kobo has always turned a page, but nothing
    /// on the panel says so, and a paginated screen that shows only "1 of 6"
    /// asks the reader to guess. Drawn only in the direction a page exists, so
    /// the last page never offers to go forward into nothing.
    PagePrevious(ActionId),
    PageNext(ActionId),
    /// What a fact is called. Muted, and set in the shared left column.
    FactLabel,
    /// What the fact is. Ink, and wrapped in whatever the label column leaves.
    FactValue,
    Divider,
    /// The weaker hairline between two rows of a list.
    ///
    /// Separate from [`LayoutKind::Divider`] because it is drawn lighter and
    /// starts at the text margin rather than the screen margin, which is what
    /// stops a list reading as ruled notebook paper.
    RowRule,
    Spacer,
    Progress,
    PagedList,
    TopBar,
    TopBarTitle,
    /// Emitted only by the runtime, never by an application.
    Back,
    BarAction(ActionId),
    /// The same, drawn as a picture rather than as a word.
    BarGlyph(ActionId, Glyph),
    /// The mark on a splash: large, centred, and not a control.
    SplashGlyph(Glyph),
    /// A splash's name, set centred rather than ranged left.
    SplashTitle,
    /// A splash's sentence, set centred under its name.
    SplashText,
    NavBar,
    /// An entry in the bottom bar. The mark is optional and drawn above the
    /// word, never in place of it: a bar entry is often the only way off a
    /// screen.
    NavDestination(ActionId, Option<Glyph>),
    NavDestinationSelected(ActionId, Option<Glyph>),
    Row(ActionId),
    Cell(ActionId, CellStyle),
    CellLabel,
    RowTitle,
    /// A title whose work is finished: muted and struck through.
    RowTitleDone,
    RowSummary,
    RowLead(RowLead),
    /// A tile's tap target: the whole cell, mark and name together. Carries a
    /// [`ControlState`] for the same reason a button does — an unavailable
    /// tile is still drawn, still occupies its place in the grid, and still
    /// must not answer a tap.
    Tile(ActionId, ControlState),
    TileLabel,
    /// A tile's second line. Muted, one line, and only emitted when at least
    /// one tile in the grid asked for one.
    TileSubtitle,
    /// The mark for a tile's state, set in a chip in the trailing corner. The
    /// chip is filled with paper first, because the corner it sits in may be a
    /// cover, and a tick drawn straight onto a dark cover is not there.
    TileState(TileState),
    /// A count or a word in the tile's leading corner, in the same chip.
    TileBadge,
    TileGlyph(Glyph),
    /// A picture drawn inside another control's rect, carrying no action of
    /// its own: the mark above a grid cell's label, or the one beside a bottom
    /// action's word. Deliberately not a control, so hit testing and press
    /// inversion both belong to the thing underneath it. A glyph that was its
    /// own target would invert a square in the middle of a button.
    InlineGlyph(Glyph),
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
    /// What has arrived, and of how much when that is known. Muted caption.
    ActivityBytes,
    /// Why the transfer stopped. Muted rather than inverted: a failure that
    /// shouts is a failure that has taken the screen away from whatever the
    /// reader was actually doing.
    ActivityFailure,
    /// A grid of characters. `text_lines` holds one entry per row, already
    /// clipped to the grid.
    TerminalGrid,
    /// One inverted cell. `text_lines` holds the single character underneath
    /// it, so the cursor can be repainted alone without the row it sits in.
    TerminalCursor,
}

impl LayoutKind {
    /// The action this kind carries, for finding a control by name once it has
    /// been laid out.
    ///
    /// The one place a laid-out thing is mapped to the action it names.
    /// `Layout::hit_control` is written in terms of this rather than
    /// repeating the list, because it was repeated once and the copies drifted:
    /// a bar glyph was added to the hit test and not to here, so the front
    /// light popover could not find the control it hung from, quietly became a
    /// centred modal, and its scrim then ate the second tap that should have
    /// put it away.
    #[must_use]
    pub const fn acts_on(&self) -> Option<ActionId> {
        match *self {
            Self::Button(action, _, _)
            | Self::BarAction(action)
            | Self::BarGlyph(action, _)
            | Self::NavDestination(action, ..)
            | Self::NavDestinationSelected(action, ..)
            | Self::Tile(action, ControlState::Enabled)
            | Self::Field(action)
            | Self::FieldClear(action)
            | Self::Chip(action, _)
            | Self::Tab(action, _)
            | Self::Row(action)
            | Self::RowMenu(action)
            | Self::Cell(action, ..)
            | Self::ChoiceOption(action, _)
            | Self::ChoiceFreeform(action)
            | Self::QuoteFold(action, _)
            | Self::PagePrevious(action)
            | Self::PageNext(action) => Some(action),
            Self::Back | Self::OverlayClose => Some(ActionId::BACK),
            _ => None,
        }
    }
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
    /// Set when the screen asked to hear about a held finger.
    pub hold: Option<ActionId>,
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
    /// A bar of destinations with nothing marked as current.
    ///
    /// A warning rather than an error, and deliberately so. `selected: None`
    /// is a real answer for a bar whose entries are verbs -- that is what
    /// [`BarStyle::Actions`] is for -- so this fires only when the bar claims
    /// to be navigation. It says: either mark where the reader is, or say
    /// plainly that these are actions.
    NavBarWithoutSelection,
    /// A title that carries its own state in the words.
    ///
    /// Fires on a trailing `(word)` or `[word]` in a label that has a state
    /// field available. Gutenbird's shelf drew `format!("{title} (kept)")` for
    /// a year: on a narrow tile the parenthesis is the first thing the
    /// ellipsis eats, so the state vanished exactly when the title was long
    /// enough to need it. State is a field.
    StateInLabel,
    /// More than one control on this screen means going back.
    ///
    /// The runtime already draws Back, so a screen that also offers "Back to
    /// the results" has two controls with one meaning and no way for a reader
    /// to tell which is which -- the book detail screen had three.
    AmbiguousBack,
    /// More than one primary action.
    ///
    /// Primary means *the thing the reader came here to do*, so a second one
    /// is not emphasis, it is the absence of a decision. If two actions are
    /// equally important then neither is primary.
    MultiplePrimaryActions,
    /// A section header at the fold with its rows on the next page.
    ///
    /// The most common way a paginated layout reads as broken, and on a panel
    /// that takes a second to turn the reader has a whole second to look at
    /// it. [`paginate_rows_in_sections`] exists to prevent it.
    OrphanedSection,
    /// An indeterminate wait that knows its own denominator.
    ///
    /// A spinner says "this will take an unknown time"; a bar says "this much
    /// of that much". An application that has counted the work and still shows
    /// a spinner is withholding what it knows.
    IndeterminateWithKnownTotal,
    /// More than four of the five inks on one screen.
    ///
    /// The tones are separated by less contrast than they appear to be on an
    /// LCD. Using all five means two of them are indistinguishable on the
    /// panel, so the distinction the fifth was carrying is simply not visible.
    ToneBudget {
        used: usize,
    },
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
            LayoutIssueKind::NavBarWithoutSelection => write!(
                formatter,
                "{node}: a bar of destinations marks none of them as current; \
                 use an action bar if these are verbs rather than places"
            ),
            LayoutIssueKind::StateInLabel => write!(
                formatter,
                "{node}: state is written into the label;                  use the node's own state field so the renderer decides how it looks"
            ),
            LayoutIssueKind::AmbiguousBack => write!(
                formatter,
                "{node}: more than one control on this screen means going back"
            ),
            LayoutIssueKind::MultiplePrimaryActions => write!(
                formatter,
                "{node}: more than one primary action;                  if two are equally important then neither is primary"
            ),
            LayoutIssueKind::OrphanedSection => write!(
                formatter,
                "{node}: a section header falls at the fold with its content on the next page"
            ),
            LayoutIssueKind::IndeterminateWithKnownTotal => write!(
                formatter,
                "{node}: an indeterminate wait that already knows its own total"
            ),
            LayoutIssueKind::ToneBudget { used } => write!(
                formatter,
                "{node}: {used} of the five inks on one screen;                  the panel does not resolve that many"
            ),
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
                    LayoutKind::Button(_, ControlState::Enabled, _)
                        | LayoutKind::Back
                        | LayoutKind::BarAction(_)
                        | LayoutKind::BarGlyph(..)
                        | LayoutKind::NavDestination(..)
                        | LayoutKind::NavDestinationSelected(..)
                        | LayoutKind::Row(_)
                        | LayoutKind::RowMenu(_)
                        | LayoutKind::Cell(..)
                        | LayoutKind::Tile(_, ControlState::Enabled)
                        | LayoutKind::Field(_)
                        | LayoutKind::FieldClear(_)
                        | LayoutKind::Chip(_, _)
                        | LayoutKind::Tab(_, _)
                        | LayoutKind::ChoiceOption(_, _)
                        | LayoutKind::ChoiceFreeform(_)
                )
            })
            .min_by_key(|node| {
                i64::from(node.rect.width.max(0)) * i64::from(node.rect.height.max(0))
            })
            .map(|node| node.rect)
    }

    /// How much of the band between the bars is already spoken for.
    ///
    /// Measured off the placed nodes rather than added up by the caller,
    /// because the only number that matters is the one the layout engine
    /// actually arrived at. An application that adds up what it thinks its
    /// rows cost is guessing at line heights, and being a few pixels short
    /// here is not a visible bug: the engine drops what does not fit in
    /// silence.
    #[must_use]
    pub fn content_used(&self) -> i32 {
        self.nodes
            .iter()
            .filter(|node| {
                node.rect.y >= self.content.y
                    && node.rect.y < self.content.y.saturating_add(self.content.height)
            })
            .map(|node| node.rect.y.saturating_add(node.rect.height))
            .max()
            .map_or(0, |bottom| bottom.saturating_sub(self.content.y).max(0))
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

    /// What a finger held on empty content means, if any.
    ///
    /// A held finger on a real control is that control being pressed, not a
    /// hold: the alternative is a reader who rests a thumb on Back and gets
    /// something else entirely.
    #[must_use]
    pub fn hit_hold(&self, x: i32, y: i32) -> Option<ActionId> {
        let hold = self.hold?;
        if !self.content.contains(x, y) || self.hit_control(x, y).is_some() {
            return None;
        }
        Some(hold)
    }

    /// The page turn a tap on empty content means, if any.
    #[must_use]
    pub fn hit_page_turn(&self, x: i32, y: i32) -> Option<ActionId> {
        let turns = self.page_turns?;
        if !self.content.contains(x, y) {
            return None;
        }
        let Some(menu) = turns.menu else {
            return if x < self.content.x + self.content.width / BACK_ZONE {
                Some(turns.previous)
            } else {
                Some(turns.next)
            };
        };
        let column = self.content.width / MENU_COLUMNS;
        if x < self.content.x + column {
            Some(turns.previous)
        } else if x < self.content.x + 2 * column {
            Some(menu)
        } else {
            Some(turns.next)
        }
    }

    fn hit_control(&self, x: i32, y: i32) -> Option<ActionId> {
        // Backwards, because later nodes are drawn on top of earlier ones and
        // what is on top is what the finger touched.
        for node in self.nodes.iter().rev() {
            if !node.rect.contains(x, y) {
                continue;
            }
            let action = match node.kind {
                // A control that is drawn as unavailable is not one, whatever
                // action it still names.
                LayoutKind::Button(_, ControlState::Disabled, _) => continue,
                // The scrim ends the search. Everything past it is underneath
                // an overlay, and a control the reader cannot see is a control
                // they cannot have meant to press. A popover treats the miss
                // as "put it away", which is what Back already means to an
                // application that owns its own history.
                LayoutKind::Scrim { dismisses } => {
                    return dismisses.then_some(ActionId::BACK);
                }
                kind => match kind.acts_on() {
                    Some(action) => action,
                    None => continue,
                },
            };
            return Some(action);
        }
        None
    }

    /// Whether the tap landed on a control that exists but cannot act.
    fn hit_inert_control(&self, x: i32, y: i32) -> bool {
        for node in self.nodes.iter().rev() {
            if !node.rect.contains(x, y) {
                continue;
            }
            // A modal's scrim swallows the tap rather than letting it become a
            // page turn underneath, on the same reasoning as a disabled
            // control: what the reader touched was not the page.
            if matches!(
                node.kind,
                LayoutKind::Button(_, ControlState::Disabled, _) | LayoutKind::Scrim { .. }
            ) {
                return true;
            }
        }
        false
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
                LayoutKind::Button(candidate, ControlState::Enabled, _)
                | LayoutKind::BarAction(candidate)
                | LayoutKind::BarGlyph(candidate, _)
                | LayoutKind::NavDestination(candidate, ..)
                | LayoutKind::NavDestinationSelected(candidate, ..)
                | LayoutKind::Tile(candidate, ControlState::Enabled)
                | LayoutKind::Field(candidate)
                | LayoutKind::FieldClear(candidate)
                | LayoutKind::Chip(candidate, _)
                | LayoutKind::Tab(candidate, _)
                | LayoutKind::ChoiceOption(candidate, _)
                | LayoutKind::Cell(candidate, ..)
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
/// headline whole, at the cost of a list whose rows differ in height, which is
/// the right trade, because a row's height is not information and its title
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

/// How wide a node wants to be when nothing is forcing it wider.
///
/// Only the nodes that have an honest answer give one. Everything else says
/// "all of it", which makes [`SlotWidth::Natural`] on a complicated node behave
/// exactly like [`SlotWidth::Fill`] rather than guessing a number and being
/// quietly wrong about it.
fn intrinsic_width(node: &Node, available: i32, metrics: &DisplayMetrics, prose: Face) -> i32 {
    match node {
        Node::Heading { text, .. } => measure_text(text, FontSize::Heading).0,
        Node::Text { text, .. } => measure_text_in(text, FontSize::Body, prose).0,
        Node::Secondary { text, .. } => measure_text(text, FontSize::Caption).0,
        Node::Button { label, .. } => measure_text(label, FontSize::Body)
            .0
            .saturating_add(2 * metrics.space(Space::Small)),
        // A list knows exactly how wide it wants to be, and a menu is a list.
        // The arithmetic is the inverse of `row_text_width`: a row always
        // reserves the lead column whether or not it has a lead, so that a
        // list of rows with mixed leads keeps one text margin.
        Node::Rows { rows, .. } => rows
            .iter()
            .map(|row| {
                let mut text = max(
                    measure_text(&row.title, FontSize::Body).0,
                    measure_text(&row.summary, FontSize::Caption).0,
                );
                if let Some(trailing) = &row.trailing {
                    text = text
                        .saturating_add(measure_text(trailing, FontSize::Caption).0)
                        .saturating_add(metrics.space(Space::Small));
                }
                if row.menu.is_some() {
                    text = text.saturating_add(metrics.touch_target_default());
                }
                text.saturating_add(row_mark_column(metrics))
                    .saturating_add(2 * metrics.space(Space::Small))
            })
            .max()
            .unwrap_or(available),
        Node::Picture {
            source,
            max_height_tenths_mm,
            ..
        } => {
            let height = metrics.tenth_mm(i32::from(*max_height_tenths_mm));
            let (source_width, source_height) = *source;
            if source_height == 0 {
                available
            } else {
                // Its own shape at the tallest it is allowed to be, which is
                // the width it will actually be drawn at.
                i32::try_from(u64::from(source_width) * height as u64 / u64::from(source_height))
                    .unwrap_or(available)
            }
        }
        _ => available,
    }
}

/// Where a row's lead is drawn inside the column reserved for it.
///
/// The column is always a touch target wide, so that every row in the system
/// keeps one text margin whatever leads it. What goes in the column is not.
///
/// A picture fills it: a cover is the content of its row and wants every pixel
/// the column has. An icon does not. An icon is a label for the row, and drawn
/// at the full width of the column it is roughly nine millimetres tall beside a
/// three and a half millimetre title, which is what makes a list of icons read
/// as clip art next to the text it belongs to. The stroke cannot fix that on
/// its own, because the stroke scales with the box: however thinly it is drawn,
/// an icon four times the height of the words beside it is the loudest thing on
/// the row. So it is drawn at the size of the type instead, centred in the
/// column it no longer fills.
///
/// A fifth again the title's em. Not the title's line height, which includes
/// the leading and would put the icon back where it started, and not its cap
/// height either: the artwork only fills about three quarters of its own box,
/// so a box the size of a capital letter draws an icon smaller than one.
/// The gutter in front of a row's text.
///
/// It was a touch target wide, on the reasoning that a row should line up with
/// every other tappable thing in the system. Nothing in that gutter is
/// tappable: the target is the whole row, and the lead is a mark sitting
/// inside it at the size the type sets it, about a third of the width. So a
/// finger's width of empty paper ran down the left of every list in the
/// system, and in front of a ranked list it was worse, because a rank is two
/// characters of caption text. On a 1072 pixel panel that is a tenth of the
/// measure spent on nothing, which is exactly what it looked like.
///
/// It is now sized to what actually sits in it, which is where `UIKit` and
/// Material both put it: Material's list item leads with a 24dp icon and
/// starts its text at 56dp, not at the 48dp touch target.
///
/// A cover is the exception, and one cover widens the column for the whole
/// list. A cover is the content of its row rather than a label on it, and at
/// the width of a mark it would be a postage stamp. Widening every row rather
/// than only the ones with artwork is deliberate: a list where some titles
/// start further left than others is worse than a wide gutter.
fn row_lead_column(metrics: &DisplayMetrics, rows: &[Row]) -> i32 {
    let target = metrics.touch_target_default();
    let mut column = 0;
    for row in rows.iter().take(MAX_ROWS) {
        column = max(
            column,
            match row.lead {
                RowLead::Picture(..) => target,
                RowLead::Icon(_) => row_mark_column(metrics),
                RowLead::Number(number) => row_rank_column(metrics, number),
            },
        );
    }
    if column == 0 {
        row_mark_column(metrics)
    } else {
        min(target, column)
    }
}

/// The column a list of marks needs, and the one every measure outside the
/// layout engine assumes.
///
/// A ranked list comes out narrower than this and a list with artwork comes
/// out wider. Narrower is harmless: the real title has more room than it was
/// paginated for, so a page under-fills rather than spilling. Wider is not,
/// which is why nothing that paginates leads with a cover, and why the one
/// application that does sets a fixed number of results per page.
/// The column the digits of a ranked list need.
///
/// Narrower than a mark, and the layout engine has drawn it that way since the
/// gutter was sized to what sits in it. Anything measuring a ranked row has to
/// ask for it too, or it hands every title less width than it will be drawn
/// with and wraps headlines that would have fitted on one line.
#[must_use]
pub fn row_rank_column(metrics: &DisplayMetrics, highest: u16) -> i32 {
    min(
        metrics.touch_target_default(),
        measure_text(&highest.to_string(), FontSize::Caption).0,
    )
}

fn row_mark_column(metrics: &DisplayMetrics) -> i32 {
    min(
        metrics.touch_target_default(),
        metrics.tenth_mm(FontSize::Body.tenth_mm() * 6 / 5),
    )
}

/// The pitch of one stand-in row, which has to be the pitch of the real row it
/// stands in for. Drawn as paragraph lines it was under half the height of the
/// list it preceded, so every screen that showed one jumped when the content
/// arrived, which is the one thing a placeholder exists to prevent. The
/// arithmetic is the `Node::Rows` arm's, and if that changes this must.
fn skeleton_band(metrics: &DisplayMetrics) -> i32 {
    max(
        metrics.touch_target_default(),
        FontSize::Body
            .line_height()
            .saturating_add(FontSize::Caption.line_height())
            .saturating_add(metrics.space(Space::Small) * 2),
    )
}

/// What sits between two stand-in rows, which is what sits between two real
/// ones: a gap, the separator, and a gap again.
fn skeleton_gap(metrics: &DisplayMetrics) -> i32 {
    metrics.space(Space::Tight) * 2
}

/// How wide each stand-in title runs, as a percentage of the column. Real
/// headlines do not all end in the same place; a stack of identical full width
/// bars reads as a loading graphic rather than as the list that is coming. The
/// pattern is fixed rather than random so that a placeholder redrawn mid-fetch
/// does not shuffle under the reader.
const SKELETON_TITLE_WIDTHS: [i32; 6] = [100, 84, 96, 72, 91, 79];

/// The same for the line under it, which stands in for a source or a date and
/// is always short.
const SKELETON_UNDER_WIDTHS: [i32; 6] = [38, 27, 45, 31, 24, 41];

/// A number keeps the whole column, and sits on the first line of the title
/// rather than in the middle of the row. It is already type, set at caption
/// size, so boxing it smaller would only risk a three digit rank running out of
/// one; but centred against the row it floated beside a two line title while
/// every other rank in the list sat beside a one line one, and a column of
/// ranks at four different heights is what made a numbered list look
/// unfinished. Bottom aligned to the title's first line rather than baseline
/// aligned, because the two sizes' descenders are close enough at these sizes
/// that the difference is under a pixel and the renderer has no ascent to ask
/// for.
fn lead_rect(
    metrics: &DisplayMetrics,
    lead: RowLead,
    x: i32,
    y: i32,
    column: i32,
    height: i32,
    text_y: i32,
) -> Rect {
    if let RowLead::Number(_) = lead {
        let line = FontSize::Caption.line_height();
        return Rect {
            x,
            y: text_y.saturating_add(FontSize::Body.line_height() - line),
            width: column,
            height: line,
        };
    }
    let side = match lead {
        // A cover is the content of its row and wants every pixel the column
        // has.
        RowLead::Picture(..) | RowLead::Number(_) => column,
        RowLead::Icon(_) => min(column, metrics.tenth_mm(FontSize::Body.tenth_mm() * 6 / 5)),
    };
    Rect {
        x: x.saturating_add((column - side) / 2),
        y: y.saturating_add((height - side) / 2),
        width: side,
        height: side,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_node(
    node: &Node,
    x: i32,
    y: i32,
    width: i32,
    // Where the content area ends. Only a splash reads it: everything else
    // measures itself and lets the caller stop when the cursor runs past the
    // bottom, which is what keeps layout a downward pass.
    bottom: i32,
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
        Node::Secondary { id, text } => {
            // Measured at its own size, and with no minimum height. The floor
            // in MIN_TEXT_HEIGHT is there so a control is never smaller than a
            // finger; metadata is not touched, and applying it puts a blank
            // line between a caption and the thing it captions.
            let lines = wrap_text(text, width, FontSize::Caption);
            let height = lines.len() as i32 * FontSize::Caption.line_height();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Secondary,
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Section { id, title, value } => {
            let lead = metrics.space(Space::Small);
            let trail = metrics.space(Space::Tight);
            let line = FontSize::Caption.line_height();
            let gap = metrics.space(Space::Tight);
            // The value is measured first and the title clamped against what
            // is left, so a long name gives up its own hairline rather than
            // pushing a total off the right margin.
            let value = value
                .as_ref()
                .map(|value| one_line(value, width, FontSize::Caption));
            let value_width = value
                .as_ref()
                .map_or(0, |value| measure_text(value, FontSize::Caption).0);
            let reserved = value_width
                .saturating_add(if value_width > 0 { gap } else { 0 })
                .saturating_add(gap)
                .saturating_add(metrics.tenth_mm(MIN_SECTION_RULE_TENTH_MM));
            let title = one_line(
                title,
                max(0, width.saturating_sub(reserved)),
                FontSize::Caption,
            );
            let mut text_lines = vec![title];
            if let Some(value) = value {
                text_lines.push(value);
            }
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y: y.saturating_add(lead),
                    width,
                    height: line,
                },
                kind: LayoutKind::Section,
                text_lines,
            });
            y.saturating_add(lead)
                .saturating_add(line)
                .saturating_add(trail)
        }
        Node::Quote {
            id,
            depth,
            role,
            text,
            fold,
        } => {
            let depth = (*depth).min(MAX_QUOTE_DEPTH);
            let (offset, full_width) = quote_offsets(metrics, width, depth);
            let text_x = x.saturating_add(offset);
            let size = role.size();
            // Only a byline folds, and only a byline gives up room for the
            // mark. A body that shortened its measure to leave space for a
            // control it does not have would ragged the whole comment.
            let fold = match role {
                QuoteRole::Byline => *fold,
                QuoteRole::Body => None,
            };
            let mark = fold.map_or(0, |_| fold_mark_width(metrics));
            let text_width = max(1, full_width - mark);
            let lines = wrap_text_in(text, text_width, size, prose);
            // A byline is one short line and is allowed to be shorter than a
            // finger: it is not a control, and forcing it to the minimum text
            // height is what put a comment's author a whole blank line away
            // from the comment.
            let measured = lines.len() as i32 * size.line_height_in(prose);
            let height = match role {
                QuoteRole::Body => max(MIN_TEXT_HEIGHT, measured),
                QuoteRole::Byline => byline_height(measured, metrics),
            };
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: text_x,
                    y,
                    // The whole column, even when the words were fitted to
                    // less of it. The band a byline is drawn on has to reach
                    // the fold mark at the far edge, or the mark floats on
                    // bare paper next to a grey strip and reads as belonging
                    // to neither. The lines were already wrapped, so a wider
                    // rectangle moves nothing.
                    width: full_width,
                    height,
                },
                kind: LayoutKind::Quote(depth, *role),
                text_lines: lines,
            });
            if let Some(fold) = fold {
                // The whole strip, not just the mark. A ten-point plus sign is
                // not something a finger can be asked to find on a panel this
                // size, and the byline is already a band the reader can see.
                // Pushed after the quote so it is drawn over the tint and hit
                // before it.
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: text_x,
                        y,
                        width: full_width,
                        height,
                    },
                    kind: LayoutKind::QuoteFold(fold.action, fold.collapsed),
                    // A bare figure beside the plus, not "12 replies": the
                    // words would be four times as wide as the number and
                    // would have to be taken out of the byline, and "+12"
                    // beside a fold is not something a reader has to be told.
                    text_lines: if fold.collapsed && fold.hidden > 0 {
                        vec![fold.hidden.to_string()]
                    } else {
                        Vec::new()
                    },
                });
            }
            y.saturating_add(height)
        }
        Node::Button {
            id,
            action,
            label,
            state,
            emphasis,
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
                kind: LayoutKind::Button(*action, *state, *emphasis),
                text_lines: wrap_text(label, width - 32, FontSize::Body),
            });
            y.saturating_add(height)
        }
        Node::Field {
            id,
            action,
            value,
            placeholder,
            clear,
        } => {
            let height = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            let inset = metrics.space(Space::Small);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Field(*action),
                text_lines: Vec::new(),
            });
            // The cross is a control in its own right and must clear a finger,
            // so it takes a square the height of the field rather than the
            // caption-sized chip a tile badge gets away with.
            let clear_width = if clear.is_some() { height } else { 0 };
            let text_width = max_i32(1, width - inset * 2 - clear_width);
            let empty = value.is_empty();
            let shown = if empty { placeholder } else { value };
            let line = FontSize::Body.line_height();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: x + inset,
                    y: y + (height - line) / 2,
                    width: text_width,
                    height: line,
                },
                kind: LayoutKind::FieldValue(empty),
                text_lines: vec![one_line(shown, text_width, FontSize::Body)],
            });
            if let Some(clear) = clear {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: x + width - height,
                        y,
                        width: height,
                        height,
                    },
                    kind: LayoutKind::FieldClear(*clear),
                    text_lines: Vec::new(),
                });
            }
            y.saturating_add(height)
        }
        Node::Chips { id, chips } => {
            let gap = metrics.space(Space::Tight);
            // A chip is a control, so it is never shorter than a finger. That
            // is also why a run of them is capped: sixteen at this height is
            // already four rows on the Clara.
            let height = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            let pad = metrics.space(Space::Small);
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
            let mut cursor_x = x;
            let mut cursor_y = y;
            let mut rows = 1;
            for chip in chips.iter().take(MAX_CHIPS) {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                let label = one_line(&chip.label, max_i32(1, width - pad * 2), FontSize::Caption);
                let chip_width = (measure_text(&label, FontSize::Caption).0 + pad * 2).min(width);
                // Wrapped by the renderer, which is the whole point: an
                // application that had to place these itself would be choosing
                // coordinates, and a subject cloud is exactly the content whose
                // width nobody can predict.
                if cursor_x > x && cursor_x + chip_width > x + width {
                    cursor_x = x;
                    cursor_y = cursor_y.saturating_add(height + gap);
                    rows += 1;
                }
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cursor_x,
                        y: cursor_y,
                        width: chip_width,
                        height,
                    },
                    kind: LayoutKind::Chip(chip.action, chip.selected),
                    text_lines: vec![label],
                });
                cursor_x = cursor_x.saturating_add(chip_width + gap);
            }
            let total = rows * height + (rows - 1) * gap;
            layout.nodes[index].rect.height = total;
            y.saturating_add(total)
        }
        Node::Tabs { id, tabs, selected } => {
            let shown = tabs.len().min(MAX_TABS);
            if shown == 0 {
                return y;
            }
            let height = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            let rule = metrics.rule_thickness();
            let current = if *selected < shown { *selected } else { 0 };
            let each = width / shown as i32;
            for (position, tab) in tabs.iter().take(shown).enumerate() {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                // The last tab takes the rounding remainder so the strip always
                // sums to the panel width, exactly as a band's last fill slot
                // does.
                let tab_x = x + each * position as i32;
                let tab_width = if position + 1 == shown {
                    x + width - tab_x
                } else {
                    each
                };
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: tab_x,
                        y,
                        width: tab_width,
                        height,
                    },
                    kind: LayoutKind::Tab(tab.action, position == current),
                    text_lines: vec![one_line(
                        &tab.label,
                        max_i32(1, tab_width - metrics.space(Space::Tight) * 2),
                        FontSize::Caption,
                    )],
                });
            }
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y: y.saturating_add(height),
                    width,
                    height: rule,
                },
                kind: LayoutKind::TabRule,
                text_lines: Vec::new(),
            });
            y.saturating_add(height + rule)
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
                    bottom,
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
        Node::Band { id, align, slots } => {
            let slots = &slots[..min(slots.len(), MAX_BAND_SLOTS)];
            if slots.is_empty() {
                return y;
            }
            let index = layout.nodes.len();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: 0,
                },
                kind: LayoutKind::Band,
                text_lines: Vec::new(),
            });
            let gap = metrics.space(Space::Small);
            let count = slots.len() as i32;
            let available = max(0, width.saturating_sub(gap.saturating_mul(count - 1)));

            // Natural and fixed slots are served first and the fills divide
            // what survives. That order is the whole reason this node exists:
            // it is what lets a title give way to its own count rather than
            // pushing the count off the margin.
            let mut widths = vec![0; slots.len()];
            let mut fills = Vec::new();
            let mut taken: i32 = 0;
            for (slot_index, slot) in slots.iter().enumerate() {
                let measured = match slot.width {
                    SlotWidth::Fixed(tenths) => min(available, metrics.tenth_mm(i32::from(tenths))),
                    SlotWidth::Natural => {
                        // The widest line in the column, since a column is only
                        // as narrow as its longest unbreakable member.
                        let widest = slot
                            .nodes
                            .iter()
                            .map(|node| intrinsic_width(node, available, metrics, prose))
                            .max()
                            .unwrap_or(0);
                        min(available, widest)
                    }
                    SlotWidth::Fill => {
                        fills.push(slot_index);
                        continue;
                    }
                };
                widths[slot_index] = measured;
                taken = taken.saturating_add(measured);
            }
            let remainder = max(0, available.saturating_sub(taken));
            if !fills.is_empty() {
                let share = remainder / fills.len() as i32;
                let mut handed = 0;
                for (nth, &slot_index) in fills.iter().enumerate() {
                    // The last fill takes the rounding, so two columns always
                    // add up to the panel rather than leaving a stray pixel.
                    widths[slot_index] = if nth + 1 == fills.len() {
                        remainder.saturating_sub(handed)
                    } else {
                        share
                    };
                    handed = handed.saturating_add(share);
                }
            }

            // The floor guards the fills and only the fills. A natural slot
            // holding the word "32" is two characters wide because that is
            // what it asked for, and stacking the band around it would be the
            // node second-guessing a measurement it was given. A fill is where
            // prose ends up, and prose below twelve millimetres is a ladder of
            // fragments.
            let minimum = metrics.tenth_mm(MIN_BAND_SLOT_TENTH_MM);
            let stacked =
                taken > available || fills.iter().any(|&slot_index| widths[slot_index] < minimum);
            let mut cursor = y;
            if stacked {
                for slot in slots {
                    for node in &slot.nodes {
                        if layout.nodes.len() >= MAX_LAYOUT_NODES {
                            break;
                        }
                        cursor = layout_node(
                            node,
                            x,
                            cursor,
                            width,
                            bottom,
                            depth + 1,
                            metrics,
                            prose,
                            layout,
                        );
                    }
                }
            } else {
                let mut slot_x = x;
                let mut placed = Vec::with_capacity(slots.len());
                for (slot_index, slot) in slots.iter().enumerate() {
                    if layout.nodes.len() >= MAX_LAYOUT_NODES {
                        break;
                    }
                    let first = layout.nodes.len();
                    let mut end = y;
                    for node in &slot.nodes {
                        if layout.nodes.len() >= MAX_LAYOUT_NODES {
                            break;
                        }
                        end = layout_node(
                            node,
                            slot_x,
                            end,
                            widths[slot_index],
                            bottom,
                            depth + 1,
                            metrics,
                            prose,
                            layout,
                        );
                    }
                    placed.push((first, layout.nodes.len(), end.saturating_sub(y)));
                    cursor = max(cursor, end);
                    slot_x = slot_x
                        .saturating_add(widths[slot_index])
                        .saturating_add(gap);
                }
                // Cross-axis alignment is applied afterwards by shifting the
                // nodes each slot produced. Measuring twice to find the tallest
                // first would mean laying every slot out twice, and layout runs
                // on every repaint.
                let tallest = placed
                    .iter()
                    .map(|(_, _, height)| *height)
                    .max()
                    .unwrap_or(0);
                if *align != BandAlign::Top {
                    for (first, last, height) in placed {
                        let slack = tallest.saturating_sub(height);
                        let offset = match align {
                            BandAlign::Top => 0,
                            BandAlign::Middle => slack / 2,
                            BandAlign::Bottom => slack,
                        };
                        if offset == 0 {
                            continue;
                        }
                        for node in &mut layout.nodes[first..last] {
                            node.rect.y = node.rect.y.saturating_add(offset);
                        }
                    }
                }
            }
            let height = cursor.saturating_sub(y);
            layout.nodes[index].rect.height = height;
            y.saturating_add(height)
        }
        Node::Facts { id, entries } => {
            let entries = &entries[..min(entries.len(), MAX_FACTS)];
            if entries.is_empty() {
                return y;
            }
            let gap = metrics.space(Space::Small);
            let label_size = FontSize::Caption;
            let value_size = FontSize::Body;
            // One measurement for the whole block. Measuring per row is what
            // makes a definition list step raggedly down the panel instead of
            // reading as two columns.
            let widest = entries
                .iter()
                .map(|(label, _)| measure_text(label, label_size).0)
                .max()
                .unwrap_or(0);
            let column = min(widest, width * FACTS_LABEL_LIMIT_EIGHTHS / 8);
            let value_x = x.saturating_add(column).saturating_add(gap);
            let value_width = max(0, width.saturating_sub(column).saturating_sub(gap));
            // The label rides down to sit on the first line of its value, so a
            // caption beside body text shares a baseline rather than floating
            // above it.
            let baseline = max(0, value_size.line_height() - label_size.line_height());
            let mut cursor = y;
            for (label, value) in entries {
                if layout.nodes.len() + 2 > MAX_LAYOUT_NODES {
                    break;
                }
                let lines = wrap_text(value, value_width, value_size);
                let height = max(
                    value_size.line_height(),
                    lines.len() as i32 * value_size.line_height(),
                );
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor.saturating_add(baseline),
                        width: column,
                        height: label_size.line_height(),
                    },
                    kind: LayoutKind::FactLabel,
                    text_lines: vec![one_line(label, column, label_size)],
                });
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: value_x,
                        y: cursor,
                        width: value_width,
                        height,
                    },
                    kind: LayoutKind::FactValue,
                    text_lines: lines,
                });
                cursor = cursor.saturating_add(height).saturating_add(gap / 2);
            }
            cursor
        }
        // Nothing is drawn and nothing is reserved. The screen loop has
        // already moved the cursor down; anywhere else -- inside a card, an
        // overlay, a band -- there is no foot to push anything to, so it is
        // simply nothing.
        Node::Flex { .. } => y,
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
            let (cell_height, style) = if *square {
                (cell_width, CellStyle::Board)
            } else {
                (metrics.touch_target_default(), CellStyle::Key)
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
                if layout.nodes.len() + 3 > MAX_LAYOUT_NODES {
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
                    kind: LayoutKind::Cell(cell.action, style),
                    text_lines: Vec::new(),
                });
                // A cell with a picture is drawn as the picture alone. The
                // label is still carried, because it is the action's name and
                // the only thing a reader could be told out loud, but setting
                // both is the worst of the two: an icon that has to be checked
                // against a word underneath it is slower to read than either.
                //
                // So the mark has to be large enough to be the whole control,
                // not the thumbnail that sat above a caption.
                match cell.glyph {
                    Some(glyph) => {
                        let mark = min(cell_height, cell_width) * 3 / 5;
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect: Rect {
                                x: rect.x.saturating_add((cell_width - mark).max(0) / 2),
                                y: rect.y.saturating_add((cell_height - mark).max(0) / 2),
                                width: mark,
                                height: mark,
                            },
                            kind: LayoutKind::InlineGlyph(glyph),
                            text_lines: vec![cell.label.clone()],
                        });
                    }
                    None => layout.nodes.push(LayoutNode {
                        id: *id,
                        rect,
                        kind: LayoutKind::CellLabel,
                        text_lines: vec![cell.label.clone()],
                    }),
                }
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
            let icon = row_lead_column(metrics, rows);
            let text_x = x.saturating_add(icon).saturating_add(padding);
            let text_width = max(1, width - icon - padding * 2);
            let mut cursor = y;
            for (position, row) in rows.iter().take(MAX_ROWS).enumerate() {
                if layout.nodes.len() + 6 > MAX_LAYOUT_NODES {
                    break;
                }
                // Separators go between rows, never after the last one. A
                // trailing rule collides with whatever the screen puts next and
                // reads as a mistake, which it was.
                //
                // Inset to the text margin and drawn at the weaker hairline,
                // because a line between two rows is not doing the same job as
                // the line under the top bar. Run full width at full weight
                // they came out as one more identical rule per row and the
                // screen read as ruled paper.
                if position > 0 {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: text_x,
                            y: cursor,
                            width: max(1, width - icon - padding),
                            height: metrics.rule_thickness(),
                        },
                        kind: LayoutKind::RowRule,
                        text_lines: Vec::new(),
                    });
                    cursor = cursor.saturating_add(gap);
                }
                // The overflow mark is measured before anything else, because
                // it is the one part of a row whose width is fixed: the title,
                // the summary and the value all wrap or clamp into what is
                // left over, and none of them may run under it.
                // The overflow mark is a target in its own right, so it keeps
                // a finger's width whatever the lead column came out at.
                let menu_column = if row.menu.is_some() {
                    metrics.touch_target_default()
                } else {
                    0
                };
                let text_width = max(1, text_width - menu_column);
                // Measured before the title is wrapped, so the value keeps its
                // room and the title gives up its own instead.
                let trailing =
                    row.trailing
                        .as_ref()
                        .filter(|value| !value.is_empty())
                        .map(|value| {
                            let value = one_line(value, text_width, FontSize::Caption);
                            let measured = measure_text(&value, FontSize::Caption).0;
                            (value, measured)
                        });
                let text_width = trailing.as_ref().map_or(text_width, |(_, measured)| {
                    max(1, text_width - measured - padding)
                });
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
                let text_y = cursor.saturating_add((height - content) / 2);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: lead_rect(metrics, row.lead, x, cursor, icon, height, text_y),
                    kind: LayoutKind::RowLead(row.lead),
                    text_lines: Vec::new(),
                });
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
                if let Some((value, measured)) = trailing {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            // Against the same right margin the row itself
                            // ends at, not the text column's.
                            x: x.saturating_add(width)
                                .saturating_sub(padding)
                                .saturating_sub(menu_column)
                                .saturating_sub(measured),
                            y: text_y,
                            width: measured,
                            height: FontSize::Caption.line_height(),
                        },
                        kind: LayoutKind::RowTrailing,
                        text_lines: vec![value],
                    });
                }
                if let Some(action) = row.menu {
                    // Pushed after the row's own target, so the backwards hit
                    // test finds the mark first. A tap on the dots is not a
                    // tap on the row, and the two must never both fire.
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(width).saturating_sub(menu_column),
                            y: cursor,
                            width: menu_column,
                            height,
                        },
                        kind: LayoutKind::RowMenu(action),
                        text_lines: Vec::new(),
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
            // Rows are set tighter than columns, and deliberately so. A cell
            // is taller than its mark (the glyph and its name are centred as a
            // pair and the rest of the cell is air) so an equal gap on both
            // axes reads as twice the space between rows that it does between
            // columns. On the panel the grid sat a tight step below the top
            // bar's rule and then twice that between its own rows, which made
            // the second and third rows look detached from the first. This is
            // the same step the grid itself begins after.
            let row_gap = metrics.space(Space::Tight);
            let cell = (width - gutter * (columns - 1)) / columns;
            // The body, plus a band beneath for the label. A tile shorter than
            // it is wide reads as a button, not a destination.
            let body = cell * shape.eighths() / 8;
            // Measured across the whole grid, not per tile. Cells in a grid are
            // the same height by definition, so one tile with a second line
            // gives every tile the room for one. The alternative is a ragged
            // bottom edge, which stops reading as a grid at all.
            let subtitled = tiles.iter().any(|tile| !tile.subtitle.is_empty());
            let caption = FontSize::Caption.line_height();
            // How many lines the longest title needs, up to two. One line
            // ellipsises real titles mid-phrase -- "Crime and..." names no
            // book -- and two is where the returns stop: a third line on a
            // six inch panel costs a whole row of the shelf.
            //
            // Measured across the whole grid for the same reason the subtitle
            // is: cells in a grid are the same height by definition, so one
            // tile needing a second line gives every tile the room for one,
            // and the alternative is a ragged bottom edge.
            let label_inset = metrics.space(Space::Tight);
            let title_width = max_i32(
                1,
                (width - gutter * (columns - 1)) / columns - label_inset * 2,
            );
            let title_lines = tiles
                .iter()
                .map(|tile| wrap_text(&tile.label, title_width, FontSize::Caption).len())
                .max()
                .unwrap_or(1)
                .clamp(1, 2) as i32;
            let label_band =
                caption * (title_lines + i32::from(subtitled)) + metrics.space(Space::Tight);
            // A cell's width is derived from the width the grid is given. Its
            // height was derived from that width alone, which was right until
            // something took a band out from under the content: a page
            // position under a shelf of six covers left the grid 28 pixels
            // short, and the grid drew the rows anyway, so the last row of
            // captions and the "1 of 6" beneath it were printed through each
            // other. A grid is a fixed set of cells on a panel that does not
            // scroll, so the room it has vertically is as much a constraint on
            // a cell as the room it has across.
            //
            // The label band is text and does not shrink; only the body does,
            // and only as far as a mark that is still a target. Past that the
            // rows that cannot fit are dropped, which is what every other node
            // does when it runs out of panel.
            let across = max_i32(1, columns);
            let rows_needed = max_i32(1, (tiles.len() as i32 + across - 1) / across);
            let room = bottom.saturating_sub(y);
            let body = if rows_needed > 0 {
                let per_cell = (room - row_gap * (rows_needed - 1)) / rows_needed;
                body.min(max_i32(
                    metrics.touch_target_minimum(),
                    per_cell - label_band,
                ))
            } else {
                body
            };
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
                if layout.nodes.len() + 6 > MAX_LAYOUT_NODES {
                    break;
                }
                let column = position as i32 % columns;
                let row = position as i32 / columns;
                let cell_x = x.saturating_add(column * (cell + gutter));
                let cell_y = y.saturating_add(row * (cell_height + row_gap));
                // The clamp above fits the rows whenever a readable cell can
                // fit them. When it cannot, the grid stops rather than drawing
                // over whatever the band was reserved for.
                if cell_y.saturating_add(cell_height) > bottom {
                    break;
                }
                rows = row + 1;
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x,
                        y: cell_y,
                        width: cell,
                        height: cell_height,
                    },
                    kind: LayoutKind::Tile(
                        tile.action,
                        if tile.state.is_tappable() {
                            ControlState::Enabled
                        } else {
                            ControlState::Disabled
                        },
                    ),
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
                // Mark and name are one object, centred together, rather than
                // a mark centred in the body with the name pinned to the cell's
                // bottom edge. Those are the same thing only when the mark
                // fills the body: a glyph is barely a third of it, so the name
                // ended up stranded a finger's width below its own icon and
                // hard against the tile's rule. Every phone home screen sets an
                // icon and its label as a pair for the same reason.
                let names = caption * (title_lines + i32::from(subtitled));
                let group = mark_height
                    .saturating_add(inset)
                    .saturating_add(names)
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
                let label = wrap_text(
                    &clamp_lines(
                        &tile.label,
                        label_width,
                        FontSize::Caption,
                        title_lines as usize,
                    ),
                    label_width,
                    FontSize::Caption,
                );
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x + inset,
                        y: group_y.saturating_add(mark_height).saturating_add(inset),
                        width: label_width,
                        height: caption * title_lines,
                    },
                    kind: LayoutKind::TileLabel,
                    text_lines: label,
                });
                if subtitled {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: cell_x + inset,
                            y: group_y
                                .saturating_add(mark_height)
                                .saturating_add(inset)
                                .saturating_add(caption * title_lines),
                            width: label_width,
                            height: caption,
                        },
                        kind: LayoutKind::TileSubtitle,
                        text_lines: vec![one_line(&tile.subtitle, label_width, FontSize::Caption)],
                    });
                }
                // Both corner chips are square and the same size, so a tile
                // that carries a badge and a state reads as one pair rather
                // than two unrelated stickers. They are placed against the
                // body's corners, not the cell's, because the label band below
                // is the one part of the tile that is certainly text.
                let chip = caption.saturating_add(inset);
                let chip_inset = metrics.rule_thickness().saturating_mul(2);
                if tile.state.glyph().is_some() {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: cell_x + cell - chip - chip_inset,
                            y: cell_y + chip_inset,
                            width: chip,
                            height: chip,
                        },
                        kind: LayoutKind::TileState(tile.state),
                        text_lines: Vec::new(),
                    });
                }
                if !tile.badge.is_empty() {
                    let badge: String = tile.badge.chars().take(TILE_BADGE_LIMIT).collect();
                    let width =
                        max_i32(chip, measure_text(&badge, FontSize::Caption).0 + inset * 2);
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: cell_x + chip_inset,
                            y: cell_y + chip_inset,
                            width,
                            height: chip,
                        },
                        kind: LayoutKind::TileBadge,
                        text_lines: vec![badge],
                    });
                }
            }
            let height = if rows == 0 {
                0
            } else {
                rows * cell_height + (rows - 1) * row_gap
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
        Node::Splash {
            id,
            glyph,
            title,
            summary,
        } => {
            let gap = metrics.space(Space::Medium);
            let mark = if glyph.is_some() {
                metrics.tenth_mm(140)
            } else {
                0
            };
            // Narrower than the page. A sentence set to the full width of a
            // panel and centred gives ragged ends on both sides and reads as
            // damage; three quarters keeps it to two or three short lines.
            let text_width = max(1, width * 3 / 4);
            let title_lines = wrap_text(title, text_width, FontSize::Title);
            let title_height = title_lines.len() as i32 * FontSize::Title.line_height();
            let summary_lines = if summary.is_empty() {
                Vec::new()
            } else {
                wrap_text(summary, text_width, FontSize::Body)
            };
            let summary_height = summary_lines.len() as i32 * FontSize::Body.line_height();
            let mut stack = title_height;
            if mark > 0 {
                stack = stack.saturating_add(mark).saturating_add(gap);
            }
            if summary_height > 0 {
                stack = stack.saturating_add(summary_height).saturating_add(gap);
            }
            // The band it was handed, or its own height if that band turned
            // out to be smaller -- a splash that centred itself inside a
            // negative remainder would be drawn above the bar it was pushed
            // under.
            let band = max(stack, bottom.saturating_sub(y));
            let mut cursor = y.saturating_add((band - stack) / 2);
            let text_x = x.saturating_add((width - text_width) / 2);
            if let Some(glyph) = glyph {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: x.saturating_add((width - mark) / 2),
                        y: cursor,
                        width: mark,
                        height: mark,
                    },
                    kind: LayoutKind::SplashGlyph(*glyph),
                    text_lines: Vec::new(),
                });
                cursor = cursor.saturating_add(mark).saturating_add(gap);
            }
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: text_x,
                    y: cursor,
                    width: text_width,
                    height: title_height,
                },
                kind: LayoutKind::SplashTitle,
                text_lines: title_lines,
            });
            cursor = cursor.saturating_add(title_height);
            if summary_height > 0 {
                cursor = cursor.saturating_add(gap);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: text_x,
                        y: cursor,
                        width: text_width,
                        height: summary_height,
                    },
                    kind: LayoutKind::SplashText,
                    text_lines: summary_lines,
                });
                cursor = cursor.saturating_add(summary_height);
            }
            max(cursor, y.saturating_add(band))
        }
        Node::Skeleton { id, lines } => {
            let count = i32::from((*lines).clamp(1, 12));
            let band = skeleton_band(metrics);
            let height = count * band + (count - 1) * skeleton_gap(metrics);
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
            transferred,
            failure,
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
            // Under the bar, because it is the bar's caption. Set even when
            // there is no bar: "4.2 MB" alone is a truthful report of an
            // unknown-length download, and a fabricated percentage is not.
            if let Some((received, total)) = transferred {
                let text = match total {
                    Some(total) => format!("{} of {}", byte_size(*received), byte_size(*total)),
                    None => byte_size(*received),
                };
                let height = FontSize::Caption.line_height();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::ActivityBytes,
                    text_lines: vec![text],
                });
                cursor = cursor.saturating_add(height).saturating_add(gap);
            }
            if let Some(failure) = failure {
                let lines = wrap_text(&failure.reason, width, FontSize::Caption);
                let height = lines.len() as i32 * FontSize::Caption.line_height();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::ActivityFailure,
                    text_lines: lines,
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
    // Measured with the status band in place, because it will be there when
    // the screen is drawn. Without it the grid came back two rows too tall,
    // and a terminal two rows too tall does not scroll: it pushes the keys it
    // shares the screen with off the bottom of the panel, which is where the
    // space bar went.
    // Back is asked for because a top bar with a control in it is never
    // shorter than one without, and only heights matter here.
    let layout = screen.layout_with(metrics, &Chrome::measuring(true));
    let content = layout.content;
    let Some(terminal) = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::TerminalGrid)
    else {
        return (0, 0);
    };
    let bottom = content.y.saturating_add(content.height);
    // The room between the top of the terminal and whatever is under it,
    // rather than what is left at the bottom of the panel. Those were the same
    // number until the keys were anchored to the foot: now the last thing on
    // the screen always ends at the bottom edge, and measuring the remainder
    // there says a terminal gets no rows at all.
    let floor = layout
        .nodes
        .iter()
        .filter(|node| {
            !matches!(
                node.kind,
                LayoutKind::TerminalGrid | LayoutKind::TerminalCursor
            ) && node.rect.y >= terminal.rect.y.saturating_add(terminal.rect.height)
        })
        .map(|node| node.rect.y)
        .min()
        .unwrap_or(bottom);
    terminal_grid(
        terminal.rect.width,
        min(floor, bottom).saturating_sub(terminal.rect.y),
    )
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
    /// so a pixel size would be a different physical size on every model.
    ///
    /// The scale used to be set against a printed paperback: body at 3.6 mm,
    /// heading at 6.8 mm. That was the wrong reference. A paperback is a page of
    /// running prose and nothing else; an interface is a page of labels, and
    /// every mainstream platform sets its interface well under book size. iOS
    /// body is 2.65 mm and its largest title 5.3 mm; Android's body-large is
    /// 2.82 mm and its headline-small 4.23 mm. Against those a 6.8 mm heading is
    /// not emphatic, it is a children's book, and it is the single loudest
    /// reason these screens read as toys.
    ///
    /// The deciding argument is that a reader who wants larger type already has
    /// it. [`TextScale`] offers 120% and 140%, so today's default *is* the large
    /// setting, and the two settings above it were merely larger still. Coming
    /// down restores the range: Large now lands where the default used to be.
    ///
    /// So the scale sits deliberately above iOS and Android rather than at them,
    /// because a reflective panel has no subpixel antialiasing and less contrast
    /// than glass, and then stops. The steps are tighter at the top than they
    /// were, because size is currently the only thing carrying hierarchy and a
    /// scale that can only shout compensates by shouting. When a bold cut ships
    /// the top of the scale should come down further still, not go back up.
    #[must_use]
    pub const fn tenth_mm(self) -> i32 {
        match self {
            Self::Caption => 24,
            Self::Body => 31,
            Self::Title => 42,
            Self::Heading => 54,
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

    /// The built-in bitmap's line height, at the current type size.
    ///
    /// Scaled like the real one. Without this a host test could not tell two
    /// type sizes apart, and pagination is exactly the thing that has to be
    /// tested without hardware.
    #[must_use]
    pub fn fallback_line_height(self) -> i32 {
        (self.unscaled_fallback_line_height() * text_scale().percent() + 50) / 100
    }

    const fn unscaled_fallback_line_height(self) -> i32 {
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
    /// misread, that is why it is Atkinson Hyperlegible, drawn by the Braille
    /// Institute to keep similar letterforms apart. Prose is the opposite
    /// problem: nothing is glanced at, everything is read in sequence, and
    /// what matters is that the eye is carried along the line without noticing
    /// the type at all. Every dedicated reader answers that with a serif, and
    /// this one does too.
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

// The type size everything is currently being measured and drawn at.
//
// Why this is ambient rather than a parameter
// -------------------------------------------
//
// A reader adjusting the size of a book changes the size of the *type*, and
// every one of the dozens of places that measures a word, wraps a line, picks
// a line height or rasterises a glyph has to agree about what that size is.
// Threading a scale through all of them would work exactly until one call site
// was missed -- and the symptom of missing one is not a compile error, it is a
// page measured at one size and drawn at another, which loses its last lines
// off the bottom of the panel with nothing to say they were ever there.
//
// So it sits beside the typeface, which is ambient for the same reason and has
// been all along. A frame is laid out and drawn in one pass on one thread, and
// the scale is set at the top of that pass, so the whole of a frame is always
// measured at one size.
// It is per-thread rather than per-process for the same reason it exists at
// all: a second thread measuring a page at its own size would otherwise cut
// this one's page to a size it was never laid out at, and the result is the
// same lost lines by a different route. A frame belongs to the thread drawing
// it.
thread_local! {
    static TEXT_SCALE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// Sets the type size for everything measured or drawn after this.
///
/// The runtime calls this once per screen, from the scale the screen asked
/// for; an application paginating for that screen calls it with the same value
/// so that the two agree.
pub fn set_text_scale(scale: TextScale) {
    TEXT_SCALE.with(|slot| slot.set(scale.wire_value()));
}

/// The size text is currently being set at.
#[must_use]
pub fn text_scale() -> TextScale {
    TextScale::from_wire(TEXT_SCALE.with(std::cell::Cell::get)).unwrap_or_default()
}

/// Runs `body` with the type at `scale`, putting it back afterwards.
///
/// For measuring a page that is not the one on screen -- which is what an
/// application does the moment a reader touches A+, because it has to know how
/// the book breaks up at the new size before it can draw it.
///
/// The previous size is put back even if `body` panics, because a scale left
/// behind by a failure would quietly mis-set every screen after it.
pub fn with_text_scale<T>(scale: TextScale, body: impl FnOnce() -> T) -> T {
    struct Restore(TextScale);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_text_scale(self.0);
        }
    }
    let _restore = Restore(text_scale());
    set_text_scale(scale);
    body()
}

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
    let percent = text_scale().percent();
    let width = glyphs.saturating_mul(6).saturating_mul(scale);
    (
        (width.saturating_mul(percent) + 50) / 100,
        (7 * scale * percent + 50) / 100,
    )
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

    /// The strip a page position takes out of the bottom of the content.
    ///
    /// Reserved by the layout engine before any content is placed, so anything
    /// measuring what will fit has to subtract it too. Pagination that does
    /// not comes back one row too many, and the row it added is drawn under
    /// the position and clipped by the bar.
    ///
    /// A touch target tall, because the band holds the chevrons that turn the
    /// page as well as the words that say which page it is. It was a caption
    /// line, and a caption line is not a control.
    #[must_use]
    pub fn page_position_band(&self) -> i32 {
        max(
            FontSize::Caption.line_height() + self.space(Space::Tight),
            self.touch_target_minimum(),
        )
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
        .map(|paragraph| (0, QuoteRole::Body, paragraph))
        .collect::<Vec<_>>();
    // The metrics only ever reach `quote_offsets`, and at depth zero that
    // returns the full width whatever panel this is, so an unindented page is
    // measured identically on every device.
    paginate_quoted(&paragraphs, &DisplayMetrics::default(), area)
        .into_iter()
        .map(|page| page.into_iter().map(|(_, _, text)| text).collect())
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
    paragraphs: &[(u8, QuoteRole, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<(u8, QuoteRole, String)>> {
    let tagged: Vec<_> = paragraphs
        .iter()
        .map(|(depth, role, text)| (0u32, *depth, *role, *text))
        .collect();
    paginate_tagged(&tagged, metrics, area)
        .into_iter()
        .map(|page| {
            page.into_iter()
                .map(|(_, depth, role, text)| (depth, role, text))
                .collect()
        })
        .collect()
}

/// The same, with a number of the application's choosing carried alongside
/// every paragraph and handed back on every piece it ends up in.
///
/// # Why this exists
///
/// A paragraph does not survive pagination intact: a long one is split across
/// two pages, and a byline is repeated at the top of a continuation. So an
/// application cannot find its way back from a paragraph on a page to the
/// thing the paragraph came from by counting, and matching on the text is
/// wrong the moment two people write the same sentence. Anything that has to
/// act on what was tapped -- folding a comment away, for instance -- needs the
/// paginator to carry the identity rather than to reconstruct it afterwards.
#[must_use]
pub fn paginate_tagged(
    paragraphs: &[(u32, u8, QuoteRole, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<(u32, u8, QuoteRole, String)>> {
    let mut pages: Vec<Page> = Vec::new();
    let mut page: Page = Vec::new();
    let mut used = 0;
    let body_height = FontSize::Body.line_height_in(area.face);
    if area.width <= 0 || area.height < body_height {
        return pages;
    }

    // Who is speaking, carried across page breaks. A comment is a byline
    // followed by its paragraphs, so the last byline seen is the author of
    // everything until the next one.
    let mut speaker: Option<(u32, u8, String)> = None;

    for (tag, depth, role, paragraph) in paragraphs {
        let tag = *tag;
        let depth = (*depth).min(MAX_QUOTE_DEPTH);
        let role = *role;
        let size = role.size();
        let line_height = size.line_height_in(area.face);
        let (_, width) = quote_offsets(metrics, area.width, depth);
        // Line breaks inside a paragraph are the source file's, not the
        // author's; Gutenberg's plain text is hard wrapped at seventy columns
        // and honouring that would give a column of ragged short lines.
        let paragraph = paragraph.replace('\n', " ");
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        if role == QuoteRole::Byline {
            speaker = Some((tag, depth, paragraph.to_owned()));
        }
        let mut lines = wrap_text_in(paragraph, width, size, area.face);
        while !lines.is_empty() {
            let spacing = if page.is_empty() { 0 } else { area.gap };
            let room = area.height - used - spacing;
            let fits = max_i32(0, room / line_height) as usize;
            if fits == 0 {
                // Nothing more will fit, so start a page rather than draw off
                // the bottom of the panel.
                used = turn_page(&mut pages, &mut page, speaker.as_ref(), role, metrics, area);
                continue;
            }
            if fits >= lines.len() {
                // The layout engine gives a body paragraph a floor, so a
                // one-line one can occupy more than one line's height. A
                // byline has no floor, because it is not a control.
                let measured = lines.len() as i32 * line_height;
                used += spacing
                    + match role {
                        QuoteRole::Body => max_i32(MIN_TEXT_HEIGHT, measured),
                        QuoteRole::Byline => byline_height(measured, metrics),
                    };
                page.push((tag, depth, role, lines.join(" ")));
                break;
            }
            // The paragraph does not fit in what is left. Splitting it at a
            // line boundary is what a book does; moving it whole to the next
            // page is what this used to do, and on a threaded discussion
            // (where a single comment is a single paragraph and often a long
            // one) it left page after page half empty, with the reader turning
            // twice as often to read the same words.
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
                page.push((tag, depth, role, lines.join(" ")));
                used = turn_page(&mut pages, &mut page, speaker.as_ref(), role, metrics, area);
                lines = rest;
            } else {
                used = turn_page(&mut pages, &mut page, speaker.as_ref(), role, metrics, area);
            }
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// One page of threaded prose: the application's tag, depth, what the
/// paragraph is for, and the words.
type Page = Vec<(u32, u8, QuoteRole, String)>;

/// Closes the current page and opens the next, naming the speaker again.
///
/// A comment longer than a page used to continue onto the next one with
/// nothing above it, so a reader who turned the page was reading words with no
/// idea whose they were, visible on a real thread, where a page began
/// mid-sentence under a bare indent. The byline is repeated at the top of the
/// continuation and charged to the new page's height, so repeating it cannot
/// push the last line off the bottom.
///
/// Only for prose. A page that breaks immediately after a byline does not need
/// the byline again; the comment has not started yet.
fn turn_page(
    pages: &mut Vec<Page>,
    page: &mut Page,
    speaker: Option<&(u32, u8, String)>,
    role: QuoteRole,
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> i32 {
    pages.push(std::mem::take(page));
    let Some((tag, depth, byline)) = speaker else {
        return 0;
    };
    if role != QuoteRole::Body {
        return 0;
    }
    let size = QuoteRole::Byline.size();
    let height = byline_height(size.line_height_in(area.face), metrics);
    if height > area.height {
        return 0;
    }
    page.push((
        *tag,
        *depth,
        QuoteRole::Byline,
        format!("{byline} \u{2026}"),
    ));
    height
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
    row_text_width_beside(metrics, area, row_mark_column(metrics))
}

/// The same, for a list whose lead column is not a mark's.
fn row_text_width_beside(metrics: &DisplayMetrics, area: ProseArea, lead: i32) -> i32 {
    let padding = metrics.space(Space::Small);
    max(1, area.width - lead - padding * 2)
}

/// Sets a cover out of type when there is no artwork to show.
///
/// Lives here rather than in `kobo-image` on purpose: a typographic cover is
/// type, and the only thing on this device that can set type is the renderer.
/// Putting it beside the JPEG decoder would have meant giving the decoder a
/// font.
///
/// The result is grey bytes in the panel's own layout, ready for
/// `Context::put_picture`.
///
/// # Why this exists
///
/// A catalogue draws whatever the server sent. When the server sent nothing,
/// or sent something that failed to decode, the shelf was left painting an
/// undecoded rectangle: on the panel that is a near-black block, which is
/// worse than an empty space because it reads as a cover of a very dark book.
/// Two of the ten books on Gutenbird's first shelf looked like that. A framed
/// title on the surface tone reads as a book without a cover, which is what it
/// is.
#[must_use]
pub fn typographic_cover(title: &str, author: Option<&str>, width: u32, height: u32) -> Vec<u8> {
    let (Ok(pixel_width), Ok(pixel_height)) = (usize::try_from(width), usize::try_from(height))
    else {
        return Vec::new();
    };
    let mut surface = Surface::new(pixel_width, pixel_height);
    let bounds = Rect {
        x: 0,
        y: 0,
        width: i32::try_from(width).unwrap_or(i32::MAX),
        height: i32::try_from(height).unwrap_or(i32::MAX),
    };
    surface.clear(tone::SURFACE);
    // A frame, so the cover has an edge of its own against the paper. Without
    // it a pale cover and the page it sits on are the same thing.
    let rule = max(1, bounds.width / 100);
    for edge in [
        Rect {
            width: bounds.width,
            height: rule,
            ..bounds
        },
        Rect {
            y: bounds.height - rule,
            height: rule,
            ..bounds
        },
        Rect {
            width: rule,
            ..bounds
        },
        Rect {
            x: bounds.width - rule,
            width: rule,
            ..bounds
        },
    ] {
        surface.fill_rect(edge, tone::RULE);
    }
    let inset = max(rule * 3, bounds.width / 10);
    let measure = bounds.width - inset * 2;
    if measure <= 0 {
        return surface.pixels;
    }
    let mut lines = wrap_text_in(title, measure, FontSize::Body, Face::Reading);
    // Truncated rather than allowed to run off the bottom edge, and truncated
    // with an ellipsis so it is plainly a shortened title rather than a book
    // whose name happens to stop mid-word.
    let room = ((bounds.height - inset * 2) / FontSize::Body.line_height_in(Face::Reading)).max(1);
    let room = usize::try_from(room).unwrap_or(1);
    let author_line = author.map(|author| one_line(author, measure, FontSize::Caption));
    let room = room
        .saturating_sub(usize::from(author_line.is_some()))
        .max(1);
    if lines.len() > room {
        lines.truncate(room);
        if let Some(last) = lines.last_mut() {
            *last = one_line(&format!("{last}…"), measure, FontSize::Body);
        }
    }
    let text_height = lines.len() as i32 * FontSize::Body.line_height_in(Face::Reading);
    let block = text_height
        + author_line
            .as_ref()
            .map_or(0, |_| FontSize::Caption.line_height());
    let mut y = (bounds.height - block) / 2;
    draw_lines_in(
        &mut surface,
        &lines,
        inset,
        y,
        FontSize::Body,
        Face::Reading,
        tone::INK,
        bounds,
    );
    y = y.saturating_add(text_height);
    if let Some(author) = author_line {
        draw_lines(
            &mut surface,
            std::slice::from_ref(&author),
            inset,
            y,
            FontSize::Caption,
            tone::MUTED,
            bounds,
        );
    }
    surface.pixels
}

/// The height a [`Node::Section`] header occupies, lead and trail included.
///
/// Public because pagination happens in the application, one layer above the
/// engine that knows this number.
#[must_use]
pub fn section_height(metrics: &DisplayMetrics) -> i32 {
    metrics
        .space(Space::Small)
        .saturating_add(FontSize::Caption.line_height())
        .saturating_add(metrics.space(Space::Tight))
}

/// Breaks a list into pages, keeping every section header with its first row.
///
/// `rows` carries an optional section title against each row; the title is
/// understood to be drawn immediately above that row. A page break is never
/// taken between the two, because a header alone at the foot of a page with
/// its contents overleaf is the single most common way a paginated layout
/// reads as broken -- and on a panel that takes a second to turn, the reader
/// has a whole second to look at it.
///
/// Returns row indices per page, the same as [`paginate_rows`]. Re-emit the
/// section title before any row on a page that carries one.
#[must_use]
pub fn paginate_rows_in_sections(
    rows: &[(Option<&str>, &str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    // A gap, the divider drawn inside it, then another gap: the engine
    // advances by the row's height and a gap, then leaves a second gap after
    // the rule it draws before the next row. The rule's own thickness is not
    // part of the stride, and counting it instead of the second gap made
    // every separator eight pixels short -- enough, over four of them, to
    // pull a whole extra row onto a page it then overflowed.
    let separator = area.gap * 2;
    let header = section_height(metrics);
    let mut pages: Vec<Vec<usize>> = Vec::new();
    let mut page: Vec<usize> = Vec::new();
    let mut used = 0;

    for (index, (section, title, summary)) in rows.iter().enumerate() {
        // The header and the row it introduces are measured as one block, so
        // the break can only ever fall before the header or after the row.
        let height = measured_row_height(
            metrics,
            area,
            title,
            summary,
            "",
            false,
            row_mark_column(metrics),
        ) + if section.is_some() { header } else { 0 };
        let spacing = if page.is_empty() { 0 } else { separator };
        if !page.is_empty() && used + spacing + height > area.height {
            pages.push(std::mem::take(&mut page));
            used = 0;
        }
        let spacing = if page.is_empty() { 0 } else { separator };
        used += spacing + height;
        page.push(index);
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

/// How wide a row's title really is beside whatever shares its right edge.
///
/// The value and the overflow mark are measured first and keep their columns;
/// the title wraps inside what is left. Anything that measures such a row --
/// paginating it, clamping its title -- has to use this, or it measures a
/// width the row will never have and comes back one line short.
#[must_use]
pub fn row_title_width(
    metrics: &DisplayMetrics,
    area: ProseArea,
    trailing: &str,
    menu: bool,
) -> i32 {
    row_title_width_beside(metrics, area, trailing, menu, row_mark_column(metrics))
}

/// The same, for a list whose lead column is not a mark's.
fn row_title_width_beside(
    metrics: &DisplayMetrics,
    area: ProseArea,
    trailing: &str,
    menu: bool,
    lead: i32,
) -> i32 {
    let mut width = row_text_width_beside(metrics, area, lead);
    if menu {
        width = max(1, width - metrics.touch_target_default());
    }
    if trailing.is_empty() {
        return width;
    }
    let value = one_line(trailing, width, FontSize::Caption);
    max(
        1,
        width - measure_text(&value, FontSize::Caption).0 - metrics.space(Space::Small),
    )
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
/// How much room the nodes after a splash need, so the splash can leave it.
///
/// Measured by laying them out into a layout that is thrown away, which is the
/// only measurement that cannot drift from the one that gets drawn.
#[allow(clippy::too_many_arguments)]
fn trailing_height(
    nodes: &[Node],
    margin: i32,
    width: i32,
    bottom: i32,
    metrics: &DisplayMetrics,
    prose: Face,
    gap: i32,
) -> i32 {
    if nodes.is_empty() {
        return 0;
    }
    let mut scratch = Layout::default();
    let mut cursor = 0;
    for node in nodes {
        if scratch.nodes.len() >= MAX_LAYOUT_NODES {
            break;
        }
        cursor = layout_node(
            node,
            margin,
            cursor,
            width,
            bottom,
            0,
            metrics,
            prose,
            &mut scratch,
        );
        cursor = cursor.saturating_add(gap);
    }
    cursor
}

/// How tall one row comes out, measured the way the layout engine measures it.
///
/// Including the columns a trailing value and an overflow mark keep for
/// themselves. Pagination that ignores either wraps the title and the summary
/// at a width the row will never have, comes back one row short per page, and
/// the extra row is drawn under the bottom bar where it is clipped away.
fn measured_row_height(
    metrics: &DisplayMetrics,
    area: ProseArea,
    title: &str,
    summary: &str,
    trailing: &str,
    menu: bool,
    lead: i32,
) -> i32 {
    let padding = metrics.space(Space::Small);
    let text_width = row_title_width_beside(metrics, area, trailing, menu, lead);
    let title_height =
        wrap_text(title, text_width, FontSize::Body).len() as i32 * FontSize::Body.line_height();
    let summary_height = if summary.is_empty() {
        0
    } else {
        wrap_text(summary, text_width, FontSize::Caption).len() as i32
            * FontSize::Caption.line_height()
    };
    // Never shorter than a finger, however terse the entry is: the same
    // floor the layout engine applies.
    max(
        metrics.touch_target_default(),
        title_height + summary_height + padding * 2,
    )
}

/// The same, for rows that carry a value at their trailing edge.
///
/// A separate entry point rather than a fourth argument to [`paginate_rows`],
/// because it mirrors the builder: a screen either sets rows or sets rows with
/// trailing values, and it should paginate with the one it draws with.
#[must_use]
pub fn paginate_rows_with_trailing(
    rows: &[(&str, &str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    paginate_rows_measured(rows, metrics, area, false, row_mark_column(metrics))
}

/// The same, for a ranked list, whose rows lead with digits rather than a mark.
///
/// `highest` is the largest rank the list will show, because the column is as
/// wide as the widest number in it. Measured against a mark instead, every
/// title loses the difference and a headline that is drawn on one line is
/// paginated as two, so the page comes back a row short and the white at the
/// foot of it is a row's worth.
#[must_use]
pub fn paginate_ranked_rows_with_trailing(
    rows: &[(&str, &str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
    highest: u16,
) -> Vec<Vec<usize>> {
    paginate_rows_measured(
        rows,
        metrics,
        area,
        false,
        row_rank_column(metrics, highest),
    )
}

/// The same, for rows that carry an overflow mark against their right edge.
///
/// A separate entry point for the same reason as
/// [`paginate_rows_with_trailing`]: a screen paginates with the shape it
/// draws with. The mark keeps a finger's width whatever the row says, so a
/// list paginated without it fits one line more per row than it will get.
#[must_use]
pub fn paginate_rows_with_menu(
    rows: &[(&str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    let rows: Vec<(&str, &str, &str)> = rows
        .iter()
        .map(|(title, summary)| (*title, *summary, ""))
        .collect();
    paginate_rows_measured(&rows, metrics, area, true, row_mark_column(metrics))
}

fn paginate_rows_measured(
    rows: &[(&str, &str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
    menu: bool,
    lead: i32,
) -> Vec<Vec<usize>> {
    // A gap, the divider drawn inside it, then another gap: the engine
    // advances by the row's height and a gap, then leaves a second gap after
    // the rule it draws before the next row. The rule's own thickness is not
    // part of the stride, and counting it instead of the second gap made
    // every separator eight pixels short -- enough, over four of them, to
    // pull a whole extra row onto a page it then overflowed.
    let separator = area.gap * 2;
    let mut pages: Vec<Vec<usize>> = Vec::new();
    let mut page: Vec<usize> = Vec::new();
    let mut used = 0;
    for (index, (title, summary, trailing)) in rows.iter().enumerate() {
        let height = measured_row_height(metrics, area, title, summary, trailing, menu, lead);
        let spacing = if page.is_empty() { 0 } else { separator };
        if !page.is_empty() && used + spacing + height > area.height {
            pages.push(std::mem::take(&mut page));
            used = 0;
        }
        let spacing = if page.is_empty() { 0 } else { separator };
        used += spacing + height;
        page.push(index);
        if used > area.height && page.len() == 1 {
            pages.push(std::mem::take(&mut page));
            used = 0;
        }
    }
    if !page.is_empty() {
        pages.push(page);
    }
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

#[must_use]
pub fn paginate_rows(
    rows: &[(&str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    // A gap, the divider drawn inside it, then another gap: the engine
    // advances by the row's height and a gap, then leaves a second gap after
    // the rule it draws before the next row. The rule's own thickness is not
    // part of the stride, and counting it instead of the second gap made
    // every separator eight pixels short -- enough, over four of them, to
    // pull a whole extra row onto a page it then overflowed.
    let separator = area.gap * 2;
    let mut pages: Vec<Vec<usize>> = Vec::new();
    let mut page: Vec<usize> = Vec::new();
    let mut used = 0;

    for (index, (title, summary)) in rows.iter().enumerate() {
        let height = measured_row_height(
            metrics,
            area,
            title,
            summary,
            "",
            false,
            row_mark_column(metrics),
        );
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
/// look like a layout bug, the layout engine drops what does not fit in
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
    // Rows are set on the tight step and columns on the small one, which is
    // what the grid itself does: a cell is much taller than the mark inside
    // it, so an equal gap on both axes reads as twice the air between rows.
    // Measuring here with the column gutter and drawing with the tight one is
    // a disagreement of a whole row on a six inch panel.
    let row_gap = metrics.space(Space::Tight);
    let cell = (area.width - gutter * (columns as i32 - 1)) / columns as i32;
    let body = cell * shape.eighths() / 8;
    let label_band = FontSize::Caption.line_height() + metrics.space(Space::Tight);
    let cell_height = max(1, body.saturating_add(label_band));
    // At least one row, however short the area is. A page that holds nothing
    // is a catalogue that can never be read.
    let rows = max(1, (area.height + row_gap) / (cell_height + row_gap));
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

/// The corner radius of a button, in tenths of a millimetre.
///
/// Square corners on a full-width outlined rectangle is what an HTML form
/// looked like in 1996, and it is most of why these controls read as
/// wireframes rather than as buttons.
pub const BUTTON_RADIUS_TENTH_MM: i32 = 10;

/// How far a press mark sits inside the control it acknowledges, in tenths of
/// a millimetre. Enough to clear a row separator and the screen margin, not so
/// much that the mark stops covering the thing that was touched.
pub const PRESS_INSET_TENTH_MM: i32 = 12;

/// The corner radius of a press mark, in tenths of a millimetre.
pub const PRESS_RADIUS_TENTH_MM: i32 = 12;

/// How far a rounded rectangle's edge is pulled in on one row.
///
/// `from_edge` is the row's distance from the nearer horizontal edge, so the
/// same arithmetic serves the top corners and the bottom ones. Measured from
/// the middle of the pixel, in half units throughout, which is why everything
/// is doubled: a circle sampled at pixel corners is visibly flat on its axes.
fn corner_inset(radius: i32, from_edge: i32) -> i32 {
    if radius <= 0 || from_edge >= radius {
        return 0;
    }
    let reach = radius * 2;
    let rise = reach - from_edge * 2 - 1;
    let run = (reach * reach - rise * rise).max(0).isqrt();
    radius - (run + 1) / 2
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
    /// Done to the finished surface rather than by drawing the control
    /// differently, so it costs nothing to lay out, applies to every kind of
    /// control including the ones drawn from vectors, and reverses exactly by
    /// being done again. [`Surface::invert_press`] is what a touched control
    /// uses; this is the square, full-bleed form.
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

    /// Turns every pixel inside a rounded `rect` to its opposite tone.
    ///
    /// The shape is derived from `rect` and `radius` alone, so this reverses
    /// exactly by being done again, which is the whole reason the press state
    /// is drawn by inverting a finished surface rather than by laying the
    /// control out twice.
    pub fn invert_rounded(&mut self, rect: Rect, radius: i32) {
        let bounds = Rect {
            x: 0,
            y: 0,
            width: i32::try_from(self.width).unwrap_or(i32::MAX),
            height: i32::try_from(self.height).unwrap_or(i32::MAX),
        };
        let radius = radius.clamp(0, min(rect.width, rect.height) / 2);
        for row in 0..rect.height {
            let inset = corner_inset(radius, min(row, rect.height - 1 - row));
            let span = Rect {
                x: rect.x.saturating_add(inset),
                y: rect.y.saturating_add(row),
                width: rect.width.saturating_sub(inset * 2),
                height: 1,
            };
            let Some(clipped) = span.intersection(bounds) else {
                continue;
            };
            let start = usize::try_from(clipped.y)
                .unwrap_or(0)
                .saturating_mul(self.width);
            for x in clipped.x..clipped.x + clipped.width {
                let index = start.saturating_add(usize::try_from(x).unwrap_or(0));
                if let Some(pixel) = self.pixels.get_mut(index) {
                    *pixel = u8::MAX - *pixel;
                }
            }
        }
    }

    /// Draws `rect` as a control the reader's finger is resting on.
    ///
    /// Inverting the control's whole hit rectangle was the obvious thing and
    /// the wrong one. A list row's rectangle is the full width of the panel and
    /// most of a centimetre tall, so a tap turned an eighth of the page solid
    /// black, edge to edge, between the rules above and below it. On a
    /// reflective panel that is not feedback, it is a flash, and it made a list
    /// that is otherwise quiet feel like a toy every time it was touched.
    ///
    /// So the mark is inset and its corners are taken off. It reads as
    /// something laid on the row rather than the row itself changing state, it
    /// leaves the separators and the screen margin alone, and it is the same
    /// shape whether the control is a row, a button or a tile.
    ///
    /// Still an inversion, not a grey fill: the refresh planner sees pure black
    /// and white in one small rectangle and picks the fast waveform, where a
    /// mid tone would need the slow one and take longer to appear than the tap
    /// it is acknowledging.
    ///
    /// The inset is capped at an eighth of the shorter side and the radius at a
    /// quarter of the inset shape, so that a control smaller than the inset
    /// itself, a checkbox say, is still plainly marked rather than reduced to a
    /// dot. A press nobody can see is the state this whole mechanism exists to
    /// avoid.
    pub fn invert_press(&mut self, rect: Rect, metrics: &DisplayMetrics) {
        let short = min(rect.width, rect.height);
        let inset = min(metrics.tenth_mm(PRESS_INSET_TENTH_MM), short / 8);
        let inner = Rect {
            x: rect.x.saturating_add(inset),
            y: rect.y.saturating_add(inset),
            width: rect.width.saturating_sub(inset * 2),
            height: rect.height.saturating_sub(inset * 2),
        };
        let radius = min(
            metrics.tenth_mm(PRESS_RADIUS_TENTH_MM),
            min(inner.width, inner.height) / 4,
        );
        self.invert_rounded(inner, radius);
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

/// How much repainting is permitted before the panel gets a cleaning refresh,
/// counted in whole panels' worth of changed pixels.
///
/// This used to count *updates*, which charged a one-character edit and a whole
/// page turn the same amount. Typing then bought a full-screen flash every
/// eighth keystroke: the panel went black and back several times while somebody
/// entered one address, which is the worst artefact E Ink has and it was being
/// spent on the cheapest possible change. Residue accumulates where pixels
/// flip, so that is what is counted now. A page turn repaints the panel and
/// still cleans on the eighth, exactly as before; a keystroke repaints a word
/// and a key, so hundreds fit in the same budget.
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
    /// Pixels that will have been repainted since the last cleaning refresh,
    /// once this transition has been applied. Zero directly after a clean.
    pub dirty: u64,
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
    dirty: u64,
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
            dirty: 0,
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
        let (region, waveform, dirty) = if self.started {
            let (changed, flipped) = self.changed(surface)?;
            // The budget is checked before this update is added to it, so that
            // a full panel's worth of repainting still buys exactly
            // PANEL_CLEAN_INTERVAL updates before anything flashes, as it did
            // when updates rather than pixels were being counted.
            if self.dirty >= self.clean_after() {
                (whole, PanelWaveform::Gc16, 0)
            } else if Self::has_grey(surface, changed) {
                (
                    changed,
                    PanelWaveform::Gl16,
                    self.dirty.saturating_add(flipped),
                )
            } else {
                (
                    changed,
                    PanelWaveform::Du,
                    self.dirty.saturating_add(flipped),
                )
            }
        } else {
            (whole, PanelWaveform::Gc16, 0)
        };
        Some(FrameTransition {
            region,
            waveform,
            full: waveform == PanelWaveform::Gc16,
            refresh: self.refreshes.saturating_add(1),
            dirty,
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
        self.dirty = transition.dirty;
        self.refreshes = transition.refresh;
        self.started = true;
        true
    }

    #[must_use]
    pub const fn refreshes(&self) -> u64 {
        self.refreshes
    }

    #[must_use]
    /// Pixels repainted since the last cleaning refresh.
    pub const fn dirty(&self) -> u64 {
        self.dirty
    }

    /// The box enclosing every changed pixel, and how many actually changed.
    ///
    /// Both come out of one pass because the count is what the cleaning budget
    /// is spent from and the box is what the controller is asked to repaint.
    /// They are deliberately different quantities: one keystroke changes a word
    /// at the top and a key at the bottom, so the box between them is most of
    /// the panel while the pixels that moved are a rounding error. Charging the
    /// box would put typing back where it started.
    fn changed(&self, surface: &Surface) -> Option<(Rect, u64)> {
        let (mut left, mut right) = (usize::MAX, 0usize);
        let (mut top, mut bottom) = (usize::MAX, 0usize);
        let mut flipped = 0_u64;
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
            flipped = flipped.saturating_add(1);
        }
        (left <= right).then(|| {
            (
                Rect {
                    x: i32::try_from(left).unwrap_or(i32::MAX),
                    y: i32::try_from(top).unwrap_or(i32::MAX),
                    width: i32::try_from(right - left + 1).unwrap_or(i32::MAX),
                    height: i32::try_from(bottom - top + 1).unwrap_or(i32::MAX),
                },
                flipped,
            )
        })
    }

    /// Changed pixels that may accumulate before the panel is cleaned.
    fn clean_after(&self) -> u64 {
        (self.width as u64)
            .saturating_mul(self.height as u64)
            .saturating_mul(u64::from(PANEL_CLEAN_INTERVAL))
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
/// Pictures are looked up at paint time rather than travelling with the
/// screen, so a source that has lost one (evicted, never delivered, or
/// refused) is a normal condition and answers `None`. Nothing is drawn in that
/// case, which is why a tile keeps its glyph as well as its picture.
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
        self.diagnostics(metrics, &Chrome::with_back(true)).issues
    }

    /// Produces layout and diagnostics from one consistent set of metrics.
    #[must_use]
    pub fn diagnostics(&self, metrics: &DisplayMetrics, chrome: &Chrome) -> LayoutDiagnostics {
        diagnose_screen(self, metrics, chrome, None)
    }

    /// Also reports picture handles absent from the runtime cache.
    #[must_use]
    pub fn diagnostics_with_pictures(
        &self,
        metrics: &DisplayMetrics,
        chrome: &Chrome,
        pictures: &dyn Pictures,
    ) -> LayoutDiagnostics {
        diagnose_screen(self, metrics, chrome, Some(pictures))
    }
}

fn diagnose_screen(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: &Chrome,
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
        for action in &top.actions {
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
        if nav.style == BarStyle::Navigation && nav.selected.is_none() {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Warning,
                node: Some(nav.id),
                kind: LayoutIssueKind::NavBarWithoutSelection,
                rect: None,
            });
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
    validate_composition(screen, &nodes, &layout, &mut issues);
    LayoutDiagnostics { layout, issues }
}

/// The rules that are about the shape of a screen rather than its arithmetic.
///
/// These are the ones that used to live in a design review nobody re-read. A
/// diagnostic is a poorer teacher than a reviewer but it is present at the
/// moment the mistake is made, which the reviewer is not.
fn validate_composition(
    screen: &Screen,
    nodes: &[&Node],
    layout: &Layout,
    issues: &mut Vec<LayoutIssue>,
) {
    let mut primaries = Vec::new();
    for node in nodes {
        match node {
            Node::Button {
                id,
                emphasis: Emphasis::Primary,
                ..
            } => primaries.push(*id),
            Node::Activity {
                id,
                progress: None,
                transferred: Some((_, Some(_))),
                ..
            } => issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Warning,
                node: Some(*id),
                kind: LayoutIssueKind::IndeterminateWithKnownTotal,
                rect: None,
            }),
            _ => {}
        }
        if let Some((id, label)) = stateful_label(node) {
            if state_written_into(label) {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Warning,
                    node: Some(id),
                    kind: LayoutIssueKind::StateInLabel,
                    rect: None,
                });
            }
        }
    }
    if primaries.len() > 1 {
        for id in primaries.iter().skip(1) {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Warning,
                node: Some(*id),
                kind: LayoutIssueKind::MultiplePrimaryActions,
                rect: None,
            });
        }
    }

    // The runtime's own Back is not in the node list, so it is counted here
    // rather than found: a screen that lends Back and then draws its own way
    // out has two, whatever the nodes say.
    let mut backs = usize::from(screen.owns_back);
    for node in nodes {
        if let Some(label) = going_back(node) {
            let _ = label;
            backs += 1;
        }
    }
    if backs > 1 {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: None,
            kind: LayoutIssueKind::AmbiguousBack,
            rect: None,
        });
    }

    if let Some(id) = orphaned_section(layout) {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: Some(id),
            kind: LayoutIssueKind::OrphanedSection,
            rect: None,
        });
    }

    let used = tones_used(layout);
    if used > MAX_TONES_PER_SCREEN {
        issues.push(LayoutIssue {
            severity: DiagnosticSeverity::Warning,
            node: None,
            kind: LayoutIssueKind::ToneBudget { used },
            rect: None,
        });
    }
}

/// A label belonging to a node that has somewhere better to put its state.
fn stateful_label(node: &Node) -> Option<(NodeId, &str)> {
    match node {
        Node::TileGrid { id, tiles, .. } => tiles
            .iter()
            .find(|tile| state_written_into(&tile.label))
            .map(|tile| (*id, tile.label.as_str())),
        Node::Rows { id, rows, .. } => rows
            .iter()
            .find(|row| state_written_into(&row.title))
            .map(|row| (*id, row.title.as_str())),
        _ => None,
    }
}

/// Whether a label ends in a parenthesised or bracketed word.
///
/// Deliberately a heuristic and deliberately narrow: it fires on `(kept)` and
/// `[done]` at the end of a label, which is how every instance of this in this
/// workspace was actually written, and not on a title that merely contains a
/// parenthesis.
fn state_written_into(label: &str) -> bool {
    let trimmed = label.trim_end();
    let Some(rest) = trimmed
        .strip_suffix(')')
        .or_else(|| trimmed.strip_suffix(']'))
    else {
        return false;
    };
    let Some(open) = rest.rfind(['(', '[']) else {
        return false;
    };
    // Only the last word, and only when there is a real title in front of it,
    // so "(1996)" standing alone is not a state and "Moby-Dick (annotated
    // edition)" is not either.
    let inside = &rest[open + 1..];
    !inside.is_empty()
        && !inside.contains(' ')
        && !inside.chars().all(|character| character.is_ascii_digit())
        && !rest[..open].trim().is_empty()
}

/// A control whose words mean "go back".
fn going_back(node: &Node) -> Option<&str> {
    let label = match node {
        Node::Button { label, .. } => label.as_str(),
        _ => return None,
    };
    let lowered = label.to_lowercase();
    (lowered.starts_with("back") || lowered.starts_with("return to")).then_some(label)
}

/// A section header drawn with nothing of its own below it.
fn orphaned_section(layout: &Layout) -> Option<NodeId> {
    let mut nodes = layout.nodes.iter().peekable();
    while let Some(node) = nodes.next() {
        if !matches!(node.kind, LayoutKind::Section) {
            continue;
        }
        match nodes.peek() {
            // Nothing after it at all, or another section immediately after
            // it: either way the header is standing on its own.
            None => return Some(node.id),
            Some(next) if matches!(next.kind, LayoutKind::Section) => return Some(node.id),
            _ => {}
        }
    }
    None
}

/// How many of the five inks this screen actually puts on the panel.
fn tones_used(layout: &Layout) -> usize {
    let mut inverted = false;
    let mut muted = false;
    let mut surface = false;
    let mut hairline = false;
    let mut ink = false;
    for node in &layout.nodes {
        match node.kind {
            LayoutKind::Divider
            | LayoutKind::RowRule
            | LayoutKind::TabRule
            | LayoutKind::Section => {
                hairline = true;
            }
            LayoutKind::Secondary
            | LayoutKind::TileSubtitle
            | LayoutKind::RowSummary
            | LayoutKind::FactLabel
            | LayoutKind::PagePosition
            | LayoutKind::ActivityBytes
            | LayoutKind::ActivityFailure => muted = true,
            LayoutKind::Card | LayoutKind::Tile(..) | LayoutKind::Overlay => surface = true,
            LayoutKind::Banner(BannerLevel::Attention)
            | LayoutKind::NavDestinationSelected(..)
            | LayoutKind::Chip(_, true)
            | LayoutKind::Button(_, ControlState::Enabled, Emphasis::Primary) => inverted = true,
            // Spacers and the scrim draw nothing at all, so they spend no ink.
            LayoutKind::Spacer | LayoutKind::Scrim { .. } | LayoutKind::Band => {}
            _ => ink = true,
        }
    }
    usize::from(inverted)
        + usize::from(muted)
        + usize::from(surface)
        + usize::from(hairline)
        + usize::from(ink)
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
        match node {
            Node::Card { children, .. } => {
                collect_nodes(children, depth + 1, collected, issues);
            }
            // A slot's contents are a node like any other and must be checked
            // like one. Walking only cards would let an undrawable glyph or a
            // missing picture ride into a band unexamined.
            Node::Band { slots, .. } => {
                for slot in slots.iter().take(MAX_BAND_SLOTS) {
                    collect_nodes(&slot.nodes, depth + 1, collected, issues);
                }
            }
            _ => {}
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
        | Node::Secondary { text, .. }
        | Node::Quote { text, .. }
        | Node::Banner { text, .. } => check_text_coverage(id, text, Face::Text, issues),
        Node::Section { title, value, .. } => {
            check_text_coverage(id, title, Face::Text, issues);
            if let Some(value) = value {
                check_text_coverage(id, value, Face::Text, issues);
            }
        }
        Node::Button { label, .. } => check_text_coverage(id, label, Face::Text, issues),
        Node::Field {
            value, placeholder, ..
        } => {
            check_text_coverage(id, value, Face::Text, issues);
            check_text_coverage(id, placeholder, Face::Text, issues);
        }
        Node::Chips { chips, .. } => {
            if chips.len() > MAX_CHIPS {
                issues.push(limit_issue(id, "chips", chips.len(), MAX_CHIPS));
            }
            for chip in chips.iter().take(MAX_CHIPS) {
                check_text_coverage(id, &chip.label, Face::Text, issues);
            }
        }
        Node::Tabs { tabs, .. } => {
            if tabs.len() > MAX_TABS {
                issues.push(limit_issue(id, "tabs", tabs.len(), MAX_TABS));
            }
            for tab in tabs.iter().take(MAX_TABS) {
                check_text_coverage(id, &tab.label, Face::Text, issues);
            }
        }
        Node::Splash { title, summary, .. } => {
            check_text_coverage(id, title, Face::Text, issues);
            check_text_coverage(id, summary, Face::Text, issues);
        }
        Node::Facts { entries, .. } => {
            if entries.len() > MAX_FACTS {
                issues.push(limit_issue(id, "facts", entries.len(), MAX_FACTS));
            }
            for (label, value) in entries.iter().take(MAX_FACTS) {
                check_text_coverage(id, label, Face::Text, issues);
                check_text_coverage(id, value, Face::Text, issues);
            }
        }
        Node::Card { .. }
        | Node::Band { .. }
        | Node::Divider { .. }
        | Node::Spacer { .. }
        | Node::Flex { .. }
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
        Node::Activity {
            label,
            cancel,
            failure,
            ..
        } => {
            check_text_coverage(id, label, Face::Text, issues);
            if let Some(cancel) = cancel {
                check_text_coverage(id, &cancel.label, Face::Text, issues);
            }
            if let Some(failure) = failure {
                check_text_coverage(id, &failure.reason, Face::Text, issues);
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
        // A flex draws nothing by design: it moves the cursor and leaves. So
        // does an empty list. Neither is content that layout hid.
        let expects_rect = !matches!(node, Node::Rows { rows, .. } if rows.is_empty())
            && !matches!(node, Node::Flex { .. });
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
        // A section keeps its title and its value in `text_lines`, but draws
        // them beside each other on one line with the hairline between. Counting
        // them as two lines reported every `section_with_value` on every screen
        // as text overflowing a rect it fits inside perfectly well.
        let rows = if matches!(node.kind, LayoutKind::Section) {
            1
        } else {
            i32::try_from(node.text_lines.len()).unwrap_or(i32::MAX)
        };
        let too_tall = rows.saturating_mul(size.line_height_in(face)) > node.rect.height;
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
        LayoutKind::Button(_, ControlState::Enabled, _)
            | LayoutKind::Back
            | LayoutKind::BarAction(_)
            | LayoutKind::BarGlyph(..)
            | LayoutKind::NavDestination(..)
            | LayoutKind::NavDestinationSelected(..)
            | LayoutKind::Row(_)
            | LayoutKind::RowMenu(_)
            | LayoutKind::Cell(..)
            | LayoutKind::Tile(..)
            | LayoutKind::Field(_)
            | LayoutKind::FieldClear(_)
            | LayoutKind::Chip(_, _)
            | LayoutKind::Tab(_, _)
            | LayoutKind::ChoiceOption(_, _)
            | LayoutKind::ChoiceFreeform(_)
            | LayoutKind::PagePrevious(_)
            | LayoutKind::PageNext(_)
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
        LayoutKind::TopBarTitle => BAR_TITLE,
        LayoutKind::OverlayTitle => FontSize::Title,
        LayoutKind::Secondary
        | LayoutKind::Section
        | LayoutKind::FactLabel
        | LayoutKind::RowTrailing
        | LayoutKind::RowSummary
        | LayoutKind::TileLabel
        | LayoutKind::TileSubtitle
        | LayoutKind::TileBadge
        | LayoutKind::Chip(_, _)
        | LayoutKind::Tab(_, _)
        | LayoutKind::PagePosition
        | LayoutKind::ActivityBytes
        | LayoutKind::ActivityFailure
        | LayoutKind::NavDestination(..)
        | LayoutKind::NavDestinationSelected(..) => FontSize::Caption,
        LayoutKind::Text
        | LayoutKind::FieldValue(_)
        | LayoutKind::FactValue
        | LayoutKind::Quote(..)
        | LayoutKind::Button(..)
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
    render_with(
        screen,
        &CLARA_BW_METRICS,
        &Chrome::default(),
        surface,
        dirty,
    );
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
    chrome: &Chrome,
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
    chrome: &Chrome,
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
            // Nothing. Written out rather than left to the catch-all so that
            // adding a dim later is a deliberate act and not an oversight: on
            // this panel, shading the whole screen costs a full refresh in and
            // a full refresh out.
            LayoutKind::Scrim { .. } => {}
            // Nothing either. A band is the extent of its columns, not a
            // surface: giving it a fill or a border would put a box around
            // every cover-and-metadata pair on the panel.
            LayoutKind::Band => {}
            // The border is the whole control: a field with only a baseline
            // under it reads as an underlined word, and one with a fill reads
            // as a button that has already been pressed.
            LayoutKind::Field(_) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            LayoutKind::FieldValue(empty) => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                if empty { tone::MUTED } else { tone::INK },
                clip,
            ),
            LayoutKind::FieldClear(_) => {
                let inset = node.rect.width / 3;
                draw_vector(
                    &mut *surface,
                    &vector::shapes(Glyph::Close),
                    Rect {
                        x: node.rect.x + inset,
                        y: node.rect.y + inset,
                        width: max_i32(1, node.rect.width - inset * 2),
                        height: max_i32(1, node.rect.height - inset * 2),
                    },
                    clip,
                    tone::INK,
                );
            }
            // Inverted when on. With two usable tones there is no third state
            // available, and an outline-versus-heavier-outline distinction is
            // not one anybody reads at arm's length on a reflective panel.
            LayoutKind::Chip(_, selected) => {
                if selected {
                    fill_clipped(surface, node.rect, tone::INK, clip);
                } else {
                    stroke_clipped(
                        surface,
                        node.rect,
                        tone::RULE,
                        metrics.rule_thickness(),
                        clip,
                    );
                }
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Caption,
                    if selected { tone::PAPER } else { tone::INK },
                    clip,
                );
            }
            // Underlined rather than inverted. A tab strip sits directly above
            // the content it filters, and a filled tab there reads as a heading
            // band across the top of the page.
            LayoutKind::Tab(_, selected) => {
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Caption,
                    if selected { tone::INK } else { tone::MUTED },
                    clip,
                );
                if selected {
                    let thickness = metrics.rule_thickness() * 2;
                    fill_clipped(
                        surface,
                        Rect {
                            x: node.rect.x,
                            y: node.rect.y + node.rect.height - thickness,
                            width: node.rect.width,
                            height: thickness,
                        },
                        tone::INK,
                        clip,
                    );
                }
            }
            LayoutKind::TabRule => fill_clipped(surface, node.rect, tone::RULE, clip),
            LayoutKind::ActivityBytes | LayoutKind::ActivityFailure => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::PagePosition => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::PagePrevious(_) => {
                draw_glyph_icon(surface, Glyph::Previous, bar_mark(node.rect), clip);
            }
            LayoutKind::PageNext(_) => {
                draw_glyph_icon(surface, Glyph::Next, bar_mark(node.rect), clip);
            }
            LayoutKind::RowTrailing => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::FactLabel => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::FactValue => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            // Paper, not surface, and a heavy border. An overlay has to look
            // like a separate sheet laid on the page, and the only two things
            // available to say so without shading everything else are the
            // brightest tone on the panel and a line thick enough to read as
            // an edge.
            LayoutKind::Overlay => {
                fill_clipped(surface, node.rect, tone::PAPER, clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness() * 2,
                    clip,
                );
            }
            // A plus or a minus at the trailing edge of the byline, and, when
            // shut, how much is behind it. Two strokes and one stroke: nothing
            // here needs a font, and a glyph at this size would be softer than
            // the rules the comment is already drawn with.
            LayoutKind::QuoteFold(_, collapsed) => {
                // Twice the weight of a rule and most of a line tall. At half
                // that it was a mark of about a millimetre and a half, which
                // on the panel is a speck: too small to read as a plus and far
                // too small to look like something worth pressing.
                let thickness = metrics.rule_thickness() * 2;
                let arm = FontSize::Caption.line_height();
                let pad = metrics.space(Space::Tight);
                let centre_x = node.rect.x + node.rect.width - pad - arm / 2;
                let centre_y = node.rect.y + node.rect.height / 2;
                fill_clipped(
                    surface,
                    Rect {
                        x: centre_x - arm / 2,
                        y: centre_y - thickness / 2,
                        width: arm,
                        height: max(1, thickness),
                    },
                    tone::INK,
                    clip,
                );
                if collapsed {
                    fill_clipped(
                        surface,
                        Rect {
                            x: centre_x - thickness / 2,
                            y: centre_y - arm / 2,
                            width: max(1, thickness),
                            height: arm,
                        },
                        tone::INK,
                        clip,
                    );
                }
                // Right up against the mark, so the two read as one label.
                if let Some(count) = node.text_lines.first() {
                    let (text_width, _) = measure_text(count, FontSize::Caption);
                    draw_lines(
                        surface,
                        &node.text_lines,
                        centre_x - arm / 2 - pad - text_width,
                        node.rect.y + (node.rect.height - FontSize::Caption.line_height()) / 2,
                        FontSize::Caption,
                        tone::MUTED,
                        clip,
                    );
                }
            }
            LayoutKind::OverlayCaret(side) => {
                draw_caret(surface, node.rect, side, clip);
            }
            LayoutKind::OverlayTitle => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                BAR_TITLE,
                tone::INK,
                clip,
            ),
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
            // The one filled control on the screen, if there is one. A fill is
            // the loudest thing this panel can draw and the slowest to clear,
            // so it is spent on the action the screen exists for.
            LayoutKind::Button(_, ControlState::Enabled, Emphasis::Primary) => {
                fill_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::INK,
                    clip,
                );
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Body,
                    tone::PAPER,
                    clip,
                );
            }
            // Outlined, in full-strength ink. This is the default, and it is
            // what makes a screen of controls read as a list of choices rather
            // than a stack of black bars. It is distinguished from a disabled
            // control by the weight of the rule and the tone of the label,
            // both of which are visible without a second control to compare
            // against.
            LayoutKind::Button(_, ControlState::Enabled, Emphasis::Normal) => {
                stroke_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::INK,
                    metrics.button_border(),
                    clip,
                );
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            LayoutKind::Button(_, ControlState::Disabled, _) => {
                stroke_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::RULE,
                    metrics.button_border(),
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
            // A board cell is outlined rather than filled, so a board reads as
            // ruled squares and an empty cell stays paper white. Filling would
            // make every move a full-cell change, which is slow on E Ink and
            // looks like a mistake.
            LayoutKind::Cell(_, CellStyle::Board) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            // A key is the field it is printed on, with no rule at all. The
            // gaps between the keys separate them, which is how a keyboard has
            // always been read, and it takes forty-five outlines off the panel.
            LayoutKind::Cell(_, CellStyle::Key) => fill_rounded_clipped(
                surface,
                node.rect,
                metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                tone::SURFACE,
                clip,
            ),
            LayoutKind::CellLabel => {
                // A short label in a tall cell is a mark rather than a word:
                // an X, an O or a letter key is the content of the cell and
                // should fill it. A short label in a *wide* cell is one word
                // among several, and setting it larger than its neighbours is
                // how a row of `esc tab ctrl up down left right` came out with
                // one enormous `up` in the middle of it.
                //
                // The cell's own shape is the signal, and the layout has
                // already decided it: square for a board, a touch target for
                // a key. Nothing else needs to be threaded through.
                let mark = node.rect.height >= node.rect.width;
                let size = if mark
                    && node
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
            LayoutKind::RowRule => fill_clipped(surface, node.rect, tone::RULE_LIGHT, clip),
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
            LayoutKind::Quote(depth, role) => {
                // One rule for every level, not one for the innermost. The
                // indent step is two millimetres, so between a reply and a
                // reply-to-a-reply there is otherwise nothing to see: on a
                // photograph of the real panel the two were indistinguishable.
                // Depth becomes something to count rather than to measure.
                let step = metrics.space(Space::Small);
                let thickness = metrics.rule_thickness();
                for back in 1..=i32::from(depth) {
                    fill_clipped(
                        surface,
                        Rect {
                            x: node.rect.x - step * back,
                            y: node.rect.y,
                            width: thickness,
                            height: node.rect.height,
                        },
                        tone::RULE,
                        clip,
                    );
                }
                // A byline sits on a tinted strip. Without it, "user 3 hours
                // ago" is one more line of text in a column of lines of text,
                // and on a page of nested replies the reader has to re-read
                // the first line of every comment to work out whether it is
                // the comment or the name of whoever wrote it. Tone alone did
                // not do it: muted grey text at caption size reads as a quiet
                // sentence, not as a label.
                //
                // The strip runs the width of the column and is inset only by
                // the padding, so it also draws the left edge of the comment
                // for the eye to run down.
                let top = if role == QuoteRole::Byline {
                    // Exactly the rectangle, because the height already
                    // includes the air. Painting outside it would run into the
                    // paragraph underneath.
                    fill_clipped(surface, node.rect, tone::SURFACE, clip);
                    metrics.space(Space::Tight)
                } else {
                    0
                };
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x,
                    node.rect.y + top,
                    role.size(),
                    role.tone(),
                    clip,
                );
            }
            // The face the layout wrapped these lines in, never a default.
            // Measuring in one face and drawing in another is what puts a line
            // past the margin it was fitted to.
            // Never in the prose face, even inside a reading screen: this is a
            // caption on the page, not part of the page.
            LayoutKind::Secondary => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            // Title, hairline, count: one node, because the three are one
            // thing and splitting them would let a repaint move the rule
            // without the words it belongs to.
            LayoutKind::Section => {
                let size = FontSize::Caption;
                let thickness = metrics.rule_thickness();
                let gap = metrics.space(Space::Tight);
                let title = node.text_lines.first().map_or("", String::as_str);
                let title_width = min(measure_text(title, size).0, node.rect.width);
                draw_text(
                    surface,
                    title,
                    node.rect.x,
                    node.rect.y,
                    size,
                    tone::MUTED,
                    clip,
                );
                let right = node.rect.x.saturating_add(node.rect.width);
                let rule_end = match node.text_lines.get(1) {
                    Some(value) => {
                        let value_width = min(measure_text(value, size).0, node.rect.width);
                        draw_text(
                            surface,
                            value,
                            right.saturating_sub(value_width),
                            node.rect.y,
                            size,
                            tone::MUTED,
                            clip,
                        );
                        right.saturating_sub(value_width).saturating_sub(gap)
                    }
                    None => right,
                };
                // Along the middle of the letters, which is where a rule beside
                // type belongs. Sitting it on the baseline turns the title into
                // something underlined.
                let rule_start = node.rect.x.saturating_add(title_width).saturating_add(gap);
                if rule_end > rule_start {
                    fill_clipped(
                        surface,
                        Rect {
                            x: rule_start,
                            y: node
                                .rect
                                .y
                                .saturating_add(size.line_height() / 2)
                                .saturating_sub(thickness / 2),
                            width: rule_end.saturating_sub(rule_start),
                            height: thickness,
                        },
                        tone::RULE,
                        clip,
                    );
                }
            }
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
            // Paper, not surface. The band is the quietest thing on the panel
            // and a tinted strip across the top of every screen is a permanent
            // horizontal line the eye has to learn to ignore.
            LayoutKind::StatusBand => fill_clipped(surface, node.rect, tone::PAPER, clip),
            LayoutKind::StatusClock => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y + (node.rect.height - FontSize::Caption.line_height()) / 2,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::StatusSignal(strength) => {
                draw_vector(surface, &vector::wifi(strength), node.rect, clip, tone::INK);
            }
            LayoutKind::StatusBluetooth => {
                draw_vector(surface, &vector::bluetooth(), node.rect, clip, tone::INK);
            }
            LayoutKind::StatusBattery(level, charging) => {
                // Nothing at all when it could not be read. An empty battery
                // and an unreadable one look identical and mean the opposite
                // things, so the honest drawing of "unknown" is no drawing.
                if let Some(level) = level {
                    draw_wide_vector(surface, &vector::battery(level, charging), node.rect, clip);
                }
            }
            LayoutKind::TopBarTitle => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                BAR_TITLE,
                tone::INK,
                clip,
            ),
            LayoutKind::Back => draw_back_arrow(surface, node.rect, clip),
            // A picture in the bar, drawn from the same geometry as every other
            // icon so it cannot arrive as a low-contrast bitmap that vanishes
            // on a grey panel.
            // Small, and drawn at half weight of a control, because it is the
            // way out of the panel rather than the thing the panel is for.
            LayoutKind::OverlayClose => {
                let side = min(node.rect.width, node.rect.height) / 2;
                draw_vector(
                    surface,
                    &vector::shapes(Glyph::Close),
                    Rect {
                        x: node.rect.x + (node.rect.width - side) / 2,
                        y: node.rect.y + (node.rect.height - side) / 2,
                        width: side,
                        height: side,
                    },
                    clip,
                    tone::INK,
                );
            }
            LayoutKind::SplashGlyph(glyph) => {
                draw_vector(surface, &vector::shapes(glyph), node.rect, clip, tone::INK);
            }
            LayoutKind::SplashTitle => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Title,
                tone::INK,
                clip,
            ),
            // Muted, so the name is the thing read first. On a panel with no
            // colour, weight is the only hierarchy there is.
            LayoutKind::SplashText => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Body,
                tone::MUTED,
                clip,
            ),
            LayoutKind::BarGlyph(_, glyph) => {
                draw_vector(
                    surface,
                    &vector::shapes(glyph),
                    bar_mark(node.rect),
                    clip,
                    tone::INK,
                );
            }
            LayoutKind::BarAction(_) => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::NavDestination(_, glyph) => {
                draw_nav_label(
                    surface,
                    &node.text_lines,
                    node.rect,
                    metrics,
                    false,
                    glyph,
                    clip,
                );
            }
            LayoutKind::NavDestinationSelected(_, glyph) => {
                draw_nav_label(
                    surface,
                    &node.text_lines,
                    node.rect,
                    metrics,
                    true,
                    glyph,
                    clip,
                );
            }
            LayoutKind::Tile(..) => stroke_clipped(
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
            LayoutKind::RowLead(lead) => draw_row_lead(surface, lead, node.rect, pictures, clip),
            // Inset from the finger-wide target it sits in, so the mark is the
            // size of a mark and the thing you press is the size of a finger.
            LayoutKind::RowMenu(_) => {
                let inset = node.rect.width / 4;
                draw_glyph_icon_in(
                    surface,
                    Glyph::MoreVertical,
                    Rect {
                        x: node.rect.x + inset,
                        y: node.rect.y + (node.rect.height - node.rect.width) / 2 + inset,
                        width: node.rect.width - inset * 2,
                        height: node.rect.width - inset * 2,
                    },
                    clip,
                    tone::MUTED,
                );
            }
            LayoutKind::TileGlyph(glyph) | LayoutKind::InlineGlyph(glyph) => {
                draw_glyph_icon(surface, glyph, node.rect, clip);
            }
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
            LayoutKind::TileSubtitle => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            // Paper first, then the border, then the mark. The corner a chip
            // sits in is very often a cover, and a tick drawn straight onto a
            // dark cover is a tick nobody can see.
            LayoutKind::TileState(state) => {
                if let Some(glyph) = state.glyph() {
                    fill_clipped(surface, node.rect, tone::PAPER, clip);
                    stroke_clipped(
                        surface,
                        node.rect,
                        tone::RULE,
                        metrics.rule_thickness(),
                        clip,
                    );
                    let inset = metrics.rule_thickness() * 2;
                    draw_glyph_icon(
                        surface,
                        glyph,
                        Rect {
                            x: node.rect.x + inset,
                            y: node.rect.y + inset,
                            width: max_i32(1, node.rect.width - inset * 2),
                            height: max_i32(1, node.rect.height - inset * 2),
                        },
                        clip,
                    );
                }
            }
            LayoutKind::TileBadge => {
                fill_clipped(surface, node.rect, tone::PAPER, clip);
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
                    FontSize::Caption,
                    tone::INK,
                    clip,
                );
            }
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
                // Both levels sit on the same quiet surface. An attention
                // banner used to be reversed out of solid black across the
                // full width of the panel, which on E Ink is a slab that has
                // to be cleared before anything near it can be redrawn, and
                // which shouted louder than the content it was warning about.
                // What separates the two levels now is a heavy bar down the
                // leading edge, the same mark a printed page uses to flag a
                // paragraph, readable at a glance and cheap to draw.
                fill_clipped(surface, node.rect, tone::SURFACE, clip);
                let mut text_x = node.rect.x + padding;
                if level == BannerLevel::Attention {
                    let bar = metrics.rule_thickness() * 3;
                    fill_clipped(
                        surface,
                        Rect {
                            width: bar,
                            ..node.rect
                        },
                        tone::INK,
                        clip,
                    );
                    text_x = text_x.saturating_add(bar);
                }
                draw_lines(
                    surface,
                    &node.text_lines,
                    text_x,
                    node.rect.y + padding,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            LayoutKind::Skeleton => {
                let count = node
                    .text_lines
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(1);
                // Drawn in the shape of the rows it stands in for: a mark in
                // the glyph column, a title, and a shorter line under it. The
                // columns are the `Node::Rows` arm's own.
                let padding = metrics.space(Space::Small);
                let band = skeleton_band(metrics);
                let icon = row_mark_column(metrics);
                let text_x = node.rect.x + icon + padding;
                let text_width = max(1, node.rect.width - icon - padding * 2);
                let lead = min(icon, metrics.tenth_mm(FontSize::Body.tenth_mm() * 6 / 5));
                let title_line = FontSize::Body.line_height();
                let under_line = FontSize::Caption.line_height();
                let title_bar = max(1, title_line - metrics.tenth_mm(14));
                let under_bar = max(1, under_line - metrics.tenth_mm(12));
                for index in 0..count {
                    let top = node.rect.y + index * (band + skeleton_gap(metrics));
                    let text_y = top + (band - title_line - under_line) / 2;
                    fill_clipped(
                        surface,
                        Rect {
                            x: node.rect.x + (icon - lead) / 2,
                            y: top + (band - lead) / 2,
                            width: lead,
                            height: lead,
                        },
                        tone::SURFACE,
                        clip,
                    );
                    let slot = index as usize % SKELETON_TITLE_WIDTHS.len();
                    fill_clipped(
                        surface,
                        Rect {
                            x: text_x,
                            y: text_y,
                            width: max(1, text_width * SKELETON_TITLE_WIDTHS[slot] / 100),
                            height: title_bar,
                        },
                        tone::SURFACE,
                        clip,
                    );
                    fill_clipped(
                        surface,
                        Rect {
                            x: text_x,
                            y: text_y + title_line,
                            width: max(1, text_width * SKELETON_UNDER_WIDTHS[slot] / 100),
                            height: under_bar,
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
    glyph: Option<Glyph>,
    clip: Rect,
) {
    // A mark sits above the word rather than instead of it. This band is a
    // finger wide and has the room, and it is often the only way off a screen,
    // so it is the last place to make somebody guess. The word drops to the
    // foot of the slot to make space, which is the shape both phone platforms
    // draw a bottom bar in.
    let mut text = rect;
    if let Some(glyph) = glyph {
        let line = FontSize::Caption.line_height();
        let gap = metrics.space(Space::Tight);
        let side = min(
            metrics.touch_target_minimum() / 2,
            max(0, rect.height - line - gap * 2),
        );
        if side > 0 {
            let block = side + gap + line;
            let top = rect.y + max(0, rect.height - block) / 2;
            draw_vector(
                surface,
                &vector::shapes(glyph),
                Rect {
                    x: rect.x + (rect.width - side) / 2,
                    y: top,
                    width: side,
                    height: side,
                },
                clip,
                tone::INK,
            );
            text = Rect {
                x: rect.x,
                y: top + side + gap,
                width: rect.width,
                height: line,
            };
        }
    }
    draw_centered(surface, lines, text, FontSize::Caption, tone::INK, clip);
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
/// Drawn at the size every other mark in the bar is drawn at.
///
/// It used to fill four fifths of its touch target while a bar glyph filled
/// three, so Back came out a third larger than the refresh mark beside it and
/// the bar looked like two different sets of chrome.
fn draw_back_arrow(surface: &mut Surface, rect: Rect, clip: Rect) {
    draw_vector(
        surface,
        &vector::back_arrow(),
        bar_mark(rect),
        clip,
        tone::INK,
    );
}

/// The square a mark occupies inside a bar control's touch target.
fn bar_mark(rect: Rect) -> Rect {
    let side = min(rect.width, rect.height) * 3 / 5;
    Rect {
        x: rect.x + (rect.width - side) / 2,
        y: rect.y + (rect.height - side) / 2,
        width: side,
        height: side,
    }
}

/// Draws whatever stands at the head of a row.
///
/// A number is set in caption size rather than body, because it is a label on
/// the row and not part of it, and centred in the same square the icon would
/// have occupied so that a list which numbers some rows and illustrates others
/// still lines up down its left edge.
fn draw_row_lead(
    surface: &mut Surface,
    lead: RowLead,
    rect: Rect,
    pictures: &dyn Pictures,
    clip: Rect,
) {
    match lead {
        RowLead::Icon(glyph) => draw_glyph_icon(surface, glyph, rect, clip),
        // The glyph is not a decoration to fall back to, it is the row still
        // working while the covers are arriving. A shelf that draws nothing
        // until every thumbnail has decoded is a shelf of empty squares.
        RowLead::Picture(picture, glyph) => match pictures.get(picture.handle) {
            Some(pixels) => {
                let (width, height) = fit_within(picture.source, rect.width, rect.height);
                let fitted = Rect {
                    x: rect.x + (rect.width - width) / 2,
                    y: rect.y + (rect.height - height) / 2,
                    width,
                    height,
                };
                draw_picture(surface, fitted, pixels, clip);
                stroke_clipped(surface, fitted, tone::RULE, 1, clip);
            }
            None => draw_glyph_icon(surface, glyph, rect, clip),
        },
        RowLead::Number(number) => {
            // Set against the right of its column rather than centred in it.
            // The text face's digits are proportional, so a one is two thirds
            // the width of a zero, and centring a mixed column of ranks left
            // every units digit in a different place. Right aligned they line
            // up whatever they are, which is what a tabular figure would have
            // bought if the face had one and the rasteriser applied OpenType
            // features. It has neither.
            let text = number.to_string();
            let size = FontSize::Caption;
            let (width, _) = measure_text(&text, size);
            let x = rect.x + max(0, rect.width - width);
            let y = rect.y + (rect.height - size.line_height()) / 2;
            draw_text(surface, &text, x, y, size, tone::MUTED, clip);
        }
    }
}

fn draw_glyph_icon(surface: &mut Surface, glyph: Glyph, rect: Rect, clip: Rect) {
    draw_vector(surface, &vector::shapes(glyph), rect, clip, tone::INK);
}

/// The same, in a chosen tone.
///
/// Only the muted tone has a second caller so far: a row's overflow mark is
/// not what the row is about, and drawn in full ink beside a title it competes
/// with the one thing the reader is looking for.
fn draw_glyph_icon_in(surface: &mut Surface, glyph: Glyph, rect: Rect, clip: Rect, tone: u8) {
    draw_vector(surface, &vector::shapes(glyph), rect, clip, tone);
}

/// Rasterises an icon into the largest square that fits `rect` and blends it.
///
/// Blended rather than thresholded: the panel resolves sixteen grey levels and
/// the renderer already picks a sixteen-level waveform when it sees grey, so a
/// stepped diagonal costs exactly as much to draw as a smooth one and looks
/// worse. This is the same reasoning that antialiases text.
/// Rasterises art that is authored wider than tall.
///
/// The design box is square and [`draw_vector`] fits it to the shorter side,
/// which is right for an icon and wrong for a battery: fitted to a status
/// band's height a battery comes out about three millimetres across and reads
/// as a dot. This fits the box to the width instead and centres it vertically.
/// The rows that fall outside the rect are empty in the art, so nothing is
/// lost, the geometry is authored to sit in the middle band of the box.
fn draw_wide_vector(surface: &mut Surface, shapes: &[vector::Shape], rect: Rect, clip: Rect) {
    if rect.width <= 0 {
        return;
    }
    blit_vector(surface, shapes, rect.width, rect, clip, tone::INK);
}

fn draw_vector(surface: &mut Surface, shapes: &[vector::Shape], rect: Rect, clip: Rect, tone: u8) {
    let size = min(rect.width, rect.height);
    if size <= 0 {
        return;
    }
    blit_vector(surface, shapes, size, rect, clip, tone);
}

/// Renders the design box at `size` and centres it on `rect`.
fn blit_vector(
    surface: &mut Surface,
    shapes: &[vector::Shape],
    size: i32,
    rect: Rect,
    clip: Rect,
    tone: u8,
) {
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
            surface.blend(x, y, tone, alpha);
        }
    }
}

/// How much of a byline is given over to the fold mark.
///
/// The mark itself, room for a four-figure count beside it, and the space
/// around both. Fixed rather than measured so that folding and unfolding
/// cannot change where the byline text wraps, which would make the line jump
/// under the finger that just tapped it -- and so a long byline can never run
/// underneath the count, because the count is never drawn outside the room
/// that was taken from the byline to begin with.
/// How tall a byline is: its line, plus a sixth of a line of air above and
/// below.
///
/// # Why the air is part of the height
///
/// The paginator has to agree with the layout to the pixel. A tint drawn
/// outside the measured rectangle would creep into the gap the paginator
/// allowed for the paragraph underneath, and a byline measured short is a
/// byline that lets one more line onto a page than will actually fit.
///
/// # Why it is a fraction and not a millimetre
///
/// A whole millimetre either side more than doubles a byline, and a byline as
/// tall as the sentence it introduces has stopped being a label. A fraction of
/// its own line keeps the proportion when the reader scales the text up, and
/// keeps a byline shorter than a paragraph, which is the thing being claimed.
fn byline_height(measured: i32, _metrics: &DisplayMetrics) -> i32 {
    measured + 2 * (measured / 6)
}

fn fold_mark_width(metrics: &DisplayMetrics) -> i32 {
    let (digits, _) = measure_text("8888", FontSize::Caption);
    FontSize::Caption.line_height() + digits + 3 * metrics.space(Space::Tight)
}

/// Draws the triangle joining a popover to the control that opened it.
///
/// Rows rather than a polygon fill: the shape is a handful of pixels, and
/// stepping the width per row is both exact and cheap.
///
/// Solid ink, and it does not try to punch a hole in the popover's border to
/// merge with it. Drawn hollow, with paper where it meets the box, it read as
/// a white notch bitten out of the edge -- and on a panel with no greys to
/// hide a seam in, an outline that does not line up exactly is worse than no
/// outline. A small filled triangle pointing at the control says the same
/// thing and cannot be got wrong.
fn draw_caret(surface: &mut Surface, rect: Rect, side: Side, clip: Rect) {
    let height = rect.height.max(1);
    let width = rect.width.max(1);
    let centre = rect.x + width / 2;
    for row in 0..height {
        // A point at the end away from the box, the full width where it meets
        // it, so the triangle grows out of the popover towards the control.
        let along = match side {
            Side::Up => height - 1 - row,
            Side::Down => row,
        };
        let half = (width * (height - along) / (2 * height)).max(0);
        if half == 0 {
            continue;
        }
        fill_clipped(
            surface,
            Rect {
                x: centre - half,
                y: rect.y + row,
                width: half * 2,
                height: 1,
            },
            tone::INK,
            clip,
        );
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

/// The horizontal run of a rounded rectangle on one of its rows, as a rect one
/// pixel tall, or `None` for a row outside the shape.
fn rounded_row(rect: Rect, radius: i32, row: i32) -> Option<Rect> {
    if row < 0 || row >= rect.height || rect.width <= 0 {
        return None;
    }
    let radius = radius.clamp(0, min(rect.width, rect.height) / 2);
    let inset = corner_inset(radius, min(row, rect.height - 1 - row));
    let width = rect.width - inset * 2;
    (width > 0).then(|| Rect {
        x: rect.x.saturating_add(inset),
        y: rect.y.saturating_add(row),
        width,
        height: 1,
    })
}

/// Fills a rectangle whose corners have been taken off.
fn fill_rounded_clipped(surface: &mut Surface, rect: Rect, radius: i32, tone: u8, clip: Rect) {
    for row in 0..rect.height {
        if let Some(span) = rounded_row(rect, radius, row) {
            fill_clipped(surface, span, tone, clip);
        }
    }
}

/// Outlines a rectangle whose corners have been taken off.
///
/// The border is the difference between the shape and the same shape inset by
/// the thickness, so it keeps its width around the curve instead of thickening
/// at the corners the way a per-row inset would. Only the ring is painted, so
/// whatever is inside the control is left alone.
fn stroke_rounded_clipped(
    surface: &mut Surface,
    rect: Rect,
    radius: i32,
    tone: u8,
    thickness: i32,
    clip: Rect,
) {
    let thickness = thickness
        .max(1)
        .min(rect.width.max(1) / 2)
        .min(rect.height.max(1) / 2);
    let inner = Rect {
        x: rect.x.saturating_add(thickness),
        y: rect.y.saturating_add(thickness),
        width: rect.width - thickness * 2,
        height: rect.height - thickness * 2,
    };
    for row in 0..rect.height {
        let Some(outer) = rounded_row(rect, radius, row) else {
            continue;
        };
        match rounded_row(inner, radius - thickness, row - thickness) {
            Some(hole) => {
                fill_clipped(
                    surface,
                    Rect {
                        width: hole.x - outer.x,
                        ..outer
                    },
                    tone,
                    clip,
                );
                let right = hole.x.saturating_add(hole.width);
                fill_clipped(
                    surface,
                    Rect {
                        x: right,
                        width: outer.x + outer.width - right,
                        ..outer
                    },
                    tone,
                    clip,
                );
            }
            None => fill_clipped(surface, outer, tone, clip),
        }
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
    fn a_cell_glyph_and_a_bottom_action_glyph_both_reach_the_layout() {
        // Both builders hand a glyph to a layout function that used to ignore
        // it: the cell payload had no glyph at all, and layout_bottom_action
        // read only the label. A builder whose argument is silently dropped is
        // worse than no builder, because the call site looks correct.
        let cells = vec![
            Cell::new(ActionId(11), "Back 30 sec").with_glyph(Glyph::Rewind30),
            Cell::new(ActionId(12), "Play").with_glyph(Glyph::Play),
        ];
        let screen = Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(1),
                columns: 2,
                square: false,
                cells,
            }],
        )
        .with_bottom_action(BottomAction::new(
            NodeId(2),
            BarAction::new(ActionId(20), "Bluetooth audio output").with_glyph(Glyph::Bluetooth),
        ));
        let layout = screen.layout();
        let drawn: Vec<Glyph> = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::InlineGlyph(glyph) => Some(glyph),
                _ => None,
            })
            .collect();
        assert_eq!(
            drawn,
            vec![Glyph::Rewind30, Glyph::Play, Glyph::Bluetooth],
            "a glyph was accepted by a builder and never drawn"
        );
        // The mark is decoration. Every one of them must sit inside a control
        // that owns the tap, or a reader who hits the icon gets nothing.
        for node in layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::InlineGlyph(_)))
        {
            assert!(
                layout.nodes.iter().any(|other| {
                    matches!(other.kind, LayoutKind::Cell(..) | LayoutKind::Button(..))
                        && other.rect.intersection(node.rect) == Some(node.rect)
                }),
                "a glyph at {:?} sits outside every control",
                node.rect
            );
        }
    }

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
            &Chrome::default(),
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
                emphasis: Emphasis::Normal,
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
                emphasis: Emphasis::Normal,
            }],
        );
        let layout = screen.layout();
        let button = &layout.nodes[0];
        assert_eq!(
            button.kind,
            LayoutKind::Button(ActionId(2), ControlState::Disabled, Emphasis::Normal)
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
                emphasis: Emphasis::Normal,
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
                emphasis: Emphasis::Normal,
            }],
        );
        let mut surface = Surface::new(128, 128);
        surface.clear(77);
        // On the button's left edge, halfway down it: the corners are rounded
        // now, so a point near one is outside the shape and would prove
        // nothing either way.
        let rect = screen
            .layout()
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(..)))
            .expect("a button")
            .rect;
        let (x, y) = (rect.x, rect.y + rect.height / 2);
        render(
            &screen,
            &mut surface,
            Some(Rect {
                x,
                y,
                width: 1,
                height: 1,
            }),
        );
        let at = |x: i32, y: i32| surface.pixels[usize::try_from(y * 128 + x).expect("inside")];
        assert_eq!(at(x, y), tone::INK);
        assert_eq!(at(x + 20, y), 77);
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
    fn frame_planner_cleans_after_eight_panels_worth_of_repainting() {
        // Eight full-panel repaints still clean on the eighth, which is what a
        // run of page turns does. That behaviour is the one being preserved.
        let mut planner = FramePlanner::new(2, 1);
        let mut frame = Surface::new(2, 1);
        let first = planner.plan(&frame).expect("first");
        assert!(planner.commit(&frame, first));
        for index in 0..PANEL_CLEAN_INTERVAL {
            let tone = if index % 2 == 0 {
                tone::INK
            } else {
                tone::PAPER
            };
            frame.pixels[0] = tone;
            frame.pixels[1] = tone;
            let partial = planner.plan(&frame).expect("partial");
            assert!(!partial.full, "repaint {index} should not flash");
            assert!(planner.commit(&frame, partial));
        }
        frame.pixels[0] = tone::MUTED;
        let cleaning = planner.plan(&frame).expect("cleaning refresh");
        assert_eq!(cleaning.waveform, PanelWaveform::Gc16);
        assert!(cleaning.full);
        assert_eq!(cleaning.region.width, 2);
        assert_eq!(cleaning.dirty, 0, "the budget resets when the panel clears");
    }

    #[test]
    fn typing_does_not_flash_the_panel_every_few_keystrokes() {
        // The reported fault: entering one address made the panel go black and
        // back several times. A keystroke changes a word and a key, so on a
        // real panel it is a rounding error against the cleaning budget, and
        // far more than eight of them must fit before anything flashes.
        let mut planner = FramePlanner::new(64, 64);
        let mut frame = Surface::new(64, 64);
        frame.clear(tone::PAPER);
        let first = planner.plan(&frame).expect("first");
        assert!(planner.commit(&frame, first));
        let mut flashes = 0;
        for keystroke in 0..64 {
            // One character somewhere near the top, one key near the bottom.
            frame.pixels[keystroke % 64] = tone::INK;
            frame.pixels[63 * 64 + keystroke % 64] = tone::INK;
            let update = planner.plan(&frame).expect("keystroke");
            if update.full {
                flashes += 1;
            }
            assert!(planner.commit(&frame, update));
        }
        assert_eq!(
            flashes, 0,
            "64 keystrokes flashed the panel {flashes} times"
        );
    }

    #[test]
    fn the_budget_is_spent_on_pixels_that_moved_not_on_the_box_around_them() {
        // A keystroke's changed box spans the panel, from the text at the top
        // to the key at the bottom. Charging the box rather than the pixels is
        // exactly what made typing flash, so the two must stay different.
        let mut planner = FramePlanner::new(32, 32);
        let mut frame = Surface::new(32, 32);
        frame.clear(tone::PAPER);
        let first = planner.plan(&frame).expect("first");
        assert!(planner.commit(&frame, first));
        frame.pixels[0] = tone::INK;
        frame.pixels[32 * 32 - 1] = tone::INK;
        let update = planner.plan(&frame).expect("two corners");
        assert_eq!(
            (update.region.width, update.region.height),
            (32, 32),
            "the controller is asked to repaint the whole box between them"
        );
        assert_eq!(update.dirty, 2, "but only two pixels are charged for");
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
                emphasis: Emphasis::Normal,
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
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
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
    fn the_bar_names_the_screen_without_shouting_over_it() {
        // The bar says which screen you are on. It is a label, not a headline,
        // and at title size it was the loudest thing on every page, larger
        // than the first heading of the content underneath it, which inverts
        // the hierarchy Kobo's own reader uses.
        assert!(
            layout_text_style(&LayoutNode {
                id: NodeId(1),
                rect: Rect::default(),
                kind: LayoutKind::TopBarTitle,
                text_lines: vec!["Cobalt".to_owned()],
            })
            .is_some_and(|(size, _)| size.tenth_mm() < FontSize::Title.tenth_mm()),
            "the bar title is still set at title size"
        );
    }

    #[test]
    fn back_is_absent_until_the_runtime_supplies_it() {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));

        let without = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(!kinds(&without).contains(&LayoutKind::Back));

        let with = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(kinds(&with).contains(&LayoutKind::Back));
    }

    #[test]
    fn back_is_reachable_by_touch_and_reports_the_reserved_action() {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
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
            let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
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
            let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
            let bar = metrics.top_bar_height();
            for node in &layout.nodes {
                if !matches!(
                    node.kind,
                    LayoutKind::Back | LayoutKind::BarAction(_) | LayoutKind::BarGlyph(..)
                ) {
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
        // the title yields instead. Long enough to overrun the widest panel at
        // the bar's own size, which is body size and not title size: the first
        // version of this title fitted once the bar stopped shouting.
        let long = "An extremely long screen title that could never fit across one line of \
                    any panel this system has ever been built for, however wide";
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
                        LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
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
                    LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
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
            .filter(|node| matches!(node.kind, LayoutKind::NavDestinationSelected(..)))
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
            .filter(|node| matches!(node.kind, LayoutKind::RowRule))
            .count();
        assert_eq!(rules, 2, "three rows need two separators");
    }

    #[test]
    fn a_rule_between_rows_starts_where_the_text_does() {
        // Full width they were one more identical line per row, indexed to the
        // panel rather than to the list, and the screen read as ruled paper.
        let layout = list(3, "Summary.").layout_for(&CLARA_BW_METRICS);
        let rule = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowRule))
            .expect("a separator");
        let title = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowTitle))
            .expect("a title");
        assert_eq!(
            rule.rect.x, title.rect.x,
            "the separator did not skip the lead column"
        );
        assert!(
            rule.rect.x + rule.rect.width <= CLARA_BW_METRICS.width,
            "the separator ran off the panel"
        );
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
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
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
    fn tile_rows_sit_no_further_apart_than_the_grid_sits_below_the_bar() {
        // A cell is much taller than the mark inside it, so a gap that is
        // right between two columns reads as roughly twice as much air between
        // two rows. On the panel the grid began one tight step under the top
        // bar's rule and then left two of them between its own rows, and the
        // second and third rows looked like a separate screen.
        for (name, metrics) in PANELS {
            let screen = grid(9).with_top_bar(TopBar::new(NodeId(9), "Cobalt"));
            let layout = screen.layout_for(&metrics);
            let mut tops = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
                .map(|node| (node.rect.y, node.rect.height))
                .collect::<Vec<_>>();
            tops.sort_unstable();
            tops.dedup();
            let rule = layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::Divider)
                .map(|node| node.rect.y + node.rect.height)
                .expect("a rule under the bar");
            let above = tops[0].0 - rule;
            for pair in tops.windows(2) {
                let [(top, height), (next, _)] = pair else {
                    continue;
                };
                let between = next - (top + height);
                assert!(
                    between <= above,
                    "{name}: {between}px between rows but only {above}px above the first"
                );
            }
        }
    }

    #[test]
    fn every_tile_is_large_enough_to_tap_on_every_panel() {
        for (name, metrics) in PANELS {
            let layout = grid(6).layout_for(&metrics);
            for node in &layout.nodes {
                if matches!(node.kind, LayoutKind::Tile(..)) {
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
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
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
            if let LayoutKind::Tile(action, _) = node.kind {
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
                if matches!(node.kind, LayoutKind::Tile(..)) {
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
            .any(|node| matches!(node.kind, LayoutKind::Tile(..))));
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
    fn a_skeleton_occupies_the_space_the_real_rows_will() {
        // The point of a skeleton is that nothing moves when data arrives.
        // Every caller in the tree stands one in front of a list, so the unit
        // it counts is a row, not a line of prose. Set in paragraph lines it
        // was under half the height of what followed it and the screen jumped.
        let lines = 5;
        let skeleton = Screen::new(
            1,
            vec![Node::Skeleton {
                id: NodeId(1),
                lines,
            }],
        )
        .layout();
        let real = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: (0..u32::from(lines))
                    .map(|index| {
                        Row::new(
                            ActionId(index + 1),
                            "A headline",
                            "news.example.com",
                            RowLead::Icon(Glyph::News),
                        )
                    })
                    .collect(),
            }],
        )
        .layout();
        let extent = |layout: &Layout| {
            layout
                .nodes
                .iter()
                .map(|node| node.rect.y + node.rect.height)
                .max()
                .unwrap_or(0)
                - layout.nodes[0].rect.y
        };
        assert_eq!(extent(&skeleton), extent(&real));
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
            12 * skeleton_band(&CLARA_BW_METRICS) + 11 * skeleton_gap(&CLARA_BW_METRICS)
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
                transferred: None,
                failure: None,
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
                transferred: None,
                failure: None,
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
                emphasis: Emphasis::Normal,
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
                transferred: None,
                failure: None,
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
                        emphasis: Emphasis::Normal,
                    },
                ],
            );
            let layout = screen.layout();
            let after = layout
                .nodes
                .iter()
                .find(|candidate| {
                    candidate.kind
                        == LayoutKind::Button(
                            ActionId(900),
                            ControlState::Enabled,
                            Emphasis::Normal,
                        )
                })
                .expect("the following button was laid out")
                .rect;
            for other in &layout.nodes {
                if other.kind
                    == LayoutKind::Button(ActionId(900), ControlState::Enabled, Emphasis::Normal)
                {
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
                LayoutKind::Button(action, ControlState::Enabled, _)
                | LayoutKind::BarAction(action)
                | LayoutKind::BarGlyph(action, _)
                | LayoutKind::Tile(action, _)
                | LayoutKind::Row(action)
                | LayoutKind::Cell(action, ..)
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
        let layout = screen.layout_with(metrics, &Chrome::with_back(true));
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
    fn drawn_quoted(page: &[(u8, QuoteRole, String)], metrics: &DisplayMetrics) -> (usize, i32) {
        let nodes = page
            .iter()
            .enumerate()
            .map(|(index, (depth, role, paragraph))| Node::Quote {
                id: NodeId(index as u32 + 1),
                depth: *depth,
                role: *role,
                text: paragraph.clone(),
                fold: None,
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
        let layout = screen.layout_with(metrics, &Chrome::with_back(true));
        let quotes = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Quote(..)))
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
                .map(|(index, paragraph)| ((index % 5) as u8, QuoteRole::Body, paragraph))
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
        let paragraphs = vec![
            (0_u8, QuoteRole::Body, "A short opening remark."),
            (1, QuoteRole::Body, reply.as_str()),
        ];
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
                .map(|(depth, _, text)| {
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
        let (depth, _, text) = last.last().expect("a paragraph");
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
            .map(|paragraph| (0, QuoteRole::Body, paragraph))
            .collect::<Vec<_>>();
        let nested = source
            .split("\n\n")
            .map(|paragraph| (3, QuoteRole::Body, paragraph))
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
                role: QuoteRole::Body,
                fold: None,
                text: "A reply, which answers the one above it.".to_owned(),
            })
            .collect();
        let layout = Screen::new(1, nodes).layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let quotes = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Quote(..)))
            .collect::<Vec<_>>();
        assert_eq!(quotes.len(), 6);
        for pair in quotes.windows(2) {
            let (shallow, deep) = (pair[0], pair[1]);
            let capped = matches!(deep.kind, LayoutKind::Quote(depth, _) if depth == MAX_QUOTE_DEPTH)
                && matches!(shallow.kind, LayoutKind::Quote(depth, _) if depth == MAX_QUOTE_DEPTH);
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

    /// Every pixel of a given tone inside a rect.
    fn tone_count(surface: &Surface, rect: Rect, want: u8, stride: usize) -> usize {
        let mut found = 0;
        for row in rect.y..rect.y + rect.height {
            for column in rect.x..rect.x + rect.width {
                let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
                    continue;
                };
                if surface.pixels[row * stride + column] == want {
                    found += 1;
                }
            }
        }
        found
    }

    fn paint(screen: &Screen) -> (Surface, usize) {
        let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
        let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
        let mut surface = Surface::new(stride, height);
        surface.clear(tone::PAPER);
        render(screen, &mut surface, None);
        (surface, stride)
    }

    fn a_status() -> Status {
        Status {
            clock: "09:41".to_owned(),
            signal: Signal::Fair,
            battery: Some(Percent::new(64)),
            charging: false,
            bluetooth: false,
        }
    }

    #[test]
    fn the_band_sits_above_the_bar_rather_than_over_it() {
        // Drawn without moving anything, the band would cover the title and
        // the way back -- the one control that must always work.
        let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Feeds"));
        let bare = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let banded = screen.layout_with(
            &CLARA_BW_METRICS,
            &Chrome::with_back(true).with_status(a_status()),
        );
        let find = |layout: &Layout, kind: LayoutKind| {
            layout
                .nodes
                .iter()
                .find(|node| node.kind == kind)
                .map(|node| node.rect)
        };
        let band = find(&banded, LayoutKind::StatusBand).expect("the band was laid out");
        assert_eq!(band.y, 0, "the band was not at the top of the panel");
        assert_eq!(band.height, CLARA_BW_METRICS.status_band_height());
        let bar = find(&banded, LayoutKind::TopBar).expect("the bar was laid out");
        assert_eq!(
            bar.y, band.height,
            "the top bar did not move down by the height of the band"
        );
        let back = find(&banded, LayoutKind::Back).expect("the way back was laid out");
        assert!(
            back.y >= band.y + band.height,
            "the band was drawn over the way back"
        );
        let before = find(&bare, LayoutKind::TopBarTitle).expect("a title");
        let after = find(&banded, LayoutKind::TopBarTitle).expect("a title");
        assert_eq!(
            after.y - before.y,
            band.height,
            "the title did not move with its bar"
        );
    }

    #[test]
    fn content_starts_below_the_band_rather_than_under_the_title() {
        // The fault this rules out reached real hardware: the top bar moved
        // down by the band and the content did not, so the launcher's first
        // row of tiles was laid out underneath its own title. It could not be
        // seen in simulation, because the simulator drew no band and a band of
        // zero height makes the wrong arithmetic right.
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A paragraph the reader is meant to be able to read.".to_owned(),
            }],
        )
        .with_top_bar(TopBar::new(NodeId(0), "Feeds"));
        let banded = screen.layout_with(
            &CLARA_BW_METRICS,
            &Chrome::with_back(true).with_status(a_status()),
        );
        let find = |layout: &Layout, kind: LayoutKind| {
            layout
                .nodes
                .iter()
                .find(|node| node.kind == kind)
                .map(|node| node.rect)
        };
        let bar = find(&banded, LayoutKind::TopBar).expect("the bar was laid out");
        let rule = find(&banded, LayoutKind::Divider).expect("the rule under the bar");
        let text = find(&banded, LayoutKind::Text).expect("the paragraph was laid out");
        assert!(
            text.y >= bar.y + bar.height,
            "content at y={} was drawn inside a bar ending at y={}",
            text.y,
            bar.y + bar.height
        );
        assert!(
            text.y >= rule.y + rule.height,
            "content was drawn over the rule under the bar"
        );
        // And the whole content area, not just the first node in it: the page
        // turn zones and every measurement of what fits are taken from this.
        assert!(
            banded.content.y >= bar.y + bar.height,
            "the content area began inside the top bar"
        );
    }

    #[test]
    fn a_book_is_not_drawn_with_a_clock_on_it() {
        // Withheld by the runtime rather than by the layout, so this checks
        // the layout honours an absent status rather than inventing one.
        let screen = Screen::new(1, vec![])
            .with_reading(true)
            .with_top_bar(TopBar::new(NodeId(0), "Moby-Dick"));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::StatusBand),
            "a band was drawn on a screen whose chrome asked for none"
        );
    }

    #[test]
    fn the_battery_is_drawn_at_the_level_it_was_read_at() {
        // The icon in the glyph set is a symbol: it means "battery" and says
        // nothing about charge. A band that showed it would tell the reader
        // their device has a battery, which they know.
        let mark = |percent: u8| {
            let shapes = vector::battery(Percent::new(percent), false);
            let coverage = vector::render(&shapes, 64);
            (0..64)
                .flat_map(|row| (0..64).map(move |column| (column, row)))
                .filter(|&(column, row)| coverage.at(column, row) > 128)
                .count()
        };
        let (empty, half, full) = (mark(0), mark(50), mark(100));
        assert!(
            half > empty && full > half,
            "the battery did not get darker as it filled: {empty}, {half}, {full}"
        );
        // A charging battery says so rather than showing a level, because the
        // level of a battery on a charger is the least interesting thing about
        // it and this panel cannot afford an animated fill.
        let charging = vector::battery(Percent::new(0), true);
        let coverage = vector::render(&charging, 64);
        assert!(
            (0..64)
                .flat_map(|row| (0..64).map(move |column| (column, row)))
                .any(|(column, row)| coverage.at(column, row) > 128),
            "a charging battery at zero percent was drawn as an empty shell"
        );
    }

    #[test]
    fn no_radio_is_a_different_mark_from_a_weak_one() {
        // Zero arcs and one arc differ by one small stroke, which at eight
        // pixels tall is not a difference a reader can act on -- and the
        // difference between "not connected" and "barely connected" is the
        // whole reason to look.
        let ink = |strength: Signal| {
            let coverage = vector::render(&vector::wifi(strength), 64);
            (0..64)
                .flat_map(|row| (0..64).map(move |column| (column, row)))
                .filter(|&(column, row)| coverage.at(column, row) > 128)
                .count()
        };
        let (weak, fair, strong) = (ink(Signal::Weak), ink(Signal::Fair), ink(Signal::Strong));
        assert!(
            weak < fair && fair < strong,
            "arcs did not accumulate with strength: {weak}, {fair}, {strong}"
        );
        // The off mark is struck through, so it carries ink where no arc does:
        // along the diagonal. Comparing quantity alone would pass for a mark
        // that was merely a fainter weak one.
        let off = vector::render(&vector::wifi(Signal::Off), 64);
        let weak_mark = vector::render(&vector::wifi(Signal::Weak), 64);
        let diagonal = (8..56)
            .filter(|&step| off.at(step, step) > 128 && weak_mark.at(step, step) == 0)
            .count();
        assert!(
            diagonal > 8,
            "the no-radio mark is not struck through, so it reads as a weak signal"
        );
    }

    #[test]
    fn the_strength_thresholds_are_the_ones_the_stock_reader_uses() {
        assert_eq!(Signal::from_dbm(-45), Signal::Strong);
        assert_eq!(Signal::from_dbm(-60), Signal::Strong);
        assert_eq!(Signal::from_dbm(-61), Signal::Fair);
        assert_eq!(Signal::from_dbm(-70), Signal::Fair);
        assert_eq!(Signal::from_dbm(-71), Signal::Weak);
        assert_eq!(Signal::from_dbm(-95), Signal::Weak);
    }

    #[test]
    fn an_unreadable_battery_is_drawn_as_nothing_rather_than_as_empty() {
        // An empty battery and an unreadable one look identical and mean the
        // opposite things, so the honest drawing of "unknown" is no drawing.
        let screen = Screen::new(1, vec![]).with_top_bar(TopBar::new(NodeId(0), "Cobalt"));
        let status = Status {
            battery: None,
            ..a_status()
        };
        let chrome = Chrome::with_back(false).with_status(status);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &chrome);
        let rect = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::StatusBattery(..)))
            .expect("the slot is still reserved so the marks do not shuffle")
            .rect;
        let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
        let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
        let mut surface = Surface::new(stride, height);
        surface.clear(tone::PAPER);
        render_with(&screen, &CLARA_BW_METRICS, &chrome, &mut surface, None);
        let drawn = (rect.y..rect.y + rect.height)
            .flat_map(|row| (rect.x..rect.x + rect.width).map(move |column| (column, row)))
            .filter(|&(column, row)| {
                let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column)) else {
                    return false;
                };
                surface.pixels[row * stride + column] != tone::PAPER
            })
            .count();
        assert_eq!(drawn, 0, "an unreadable battery was drawn as an empty one");
    }

    #[test]
    fn an_ordinary_control_is_outlined_and_only_the_primary_one_is_filled() {
        // Every enabled button used to be a filled black slab, so a screen of
        // three choices was three identical black bars with nothing to aim at,
        // and the panel had the most expensive mark it can draw repeated three
        // times. Only the action the screen exists for is filled now.
        let screen = Screen::new(
            1,
            vec![
                Node::Button {
                    id: NodeId(1),
                    action: ActionId(1),
                    label: "Read".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
                Node::Button {
                    id: NodeId(2),
                    action: ActionId(2),
                    label: "Details".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
            ],
        );
        let layout = screen.layout();
        let rect = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("the control was laid out")
                .rect
        };
        let (surface, stride) = paint(&screen);
        let filled = tone_count(&surface, rect(1), tone::INK, stride);
        let outlined = tone_count(&surface, rect(2), tone::INK, stride);
        let area = usize::try_from(rect(1).width * rect(1).height).expect("a real control");
        assert!(
            filled * 10 > area * 8,
            "the primary control was not filled: {filled} of {area}"
        );
        assert!(
            outlined * 4 < area,
            "the ordinary control was filled too: {outlined} of {area}"
        );
        assert!(
            outlined > 0,
            "the ordinary control left no mark at all, so it cannot be found"
        );
    }

    #[test]
    fn an_ordinary_control_still_reads_as_available_next_to_a_disabled_one() {
        // Both are outlined, so what separates them has to be the weight of
        // the rule and the tone of the label rather than the shape.
        let screen = Screen::new(
            1,
            vec![
                Node::Button {
                    id: NodeId(1),
                    action: ActionId(1),
                    label: "Subscribe".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
                Node::Button {
                    id: NodeId(2),
                    action: ActionId(2),
                    label: "Subscribe".to_owned(),
                    state: ControlState::Disabled,
                    emphasis: Emphasis::Normal,
                },
            ],
        );
        let layout = screen.layout();
        let rect = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
                .rect
        };
        let (surface, stride) = paint(&screen);
        assert!(
            tone_count(&surface, rect(1), tone::INK, stride)
                > tone_count(&surface, rect(2), tone::INK, stride),
            "an available control was no darker than one that cannot be used"
        );
        assert_eq!(
            tone_count(&surface, rect(1), tone::MUTED, stride),
            0,
            "an available control was drawn in the tone reserved for refusal"
        );
    }

    #[test]
    fn an_attention_banner_marks_its_edge_rather_than_blacking_out_the_panel() {
        // It used to be reversed out of solid black across the full width,
        // which shouted louder than whatever it was warning about and left the
        // panel a slab to clear before anything near it could be redrawn.
        let screen = Screen::new(
            1,
            vec![Node::Banner {
                id: NodeId(1),
                level: BannerLevel::Attention,
                text: "The network went away.".to_owned(),
            }],
        );
        let rect = screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| matches!(node.kind, LayoutKind::Banner(_)))
            .expect("the banner was laid out")
            .rect;
        let (surface, stride) = paint(&screen);
        let ink = tone_count(&surface, rect, tone::INK, stride);
        let area = usize::try_from(rect.width * rect.height).expect("a real banner");
        assert!(
            ink * 4 < area,
            "the banner is still mostly black: {ink} of {area}"
        );
        // The leading edge is the mark that distinguishes it, so it must be
        // solid ink for the full height of the banner.
        let bar = CLARA_BW_METRICS.rule_thickness() * 3;
        let edge = Rect { width: bar, ..rect };
        assert_eq!(
            tone_count(&surface, edge, tone::INK, stride),
            usize::try_from(bar * rect.height).expect("a real edge"),
            "the leading edge was not drawn solid"
        );
    }

    #[test]
    fn metadata_is_lighter_and_smaller_than_what_it_describes() {
        // A heading followed by an undifferentiated column is what made every
        // screen read as one block of prose. A caption is set like a caption.
        let screen = Screen::new(
            1,
            vec![
                Node::Text {
                    id: NodeId(1),
                    text: "Twenty Thousand Leagues Under the Sea".to_owned(),
                },
                Node::Secondary {
                    id: NodeId(2),
                    text: "Jules Verne, 1870".to_owned(),
                },
            ],
        );
        let layout = screen.layout();
        let node = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
                .clone()
        };
        assert_eq!(node(2).kind, LayoutKind::Secondary);
        assert!(
            node(2).rect.height < node(1).rect.height,
            "the caption took as much room as the line it describes"
        );
        let (surface, stride) = paint(&screen);
        assert_eq!(
            tone_count(&surface, node(2).rect, tone::INK, stride),
            0,
            "metadata was drawn in full-strength ink"
        );
        assert!(
            tone_count(&surface, node(2).rect, tone::MUTED, stride) > 0,
            "metadata was not drawn at all"
        );
    }

    #[test]
    fn depth_is_counted_in_rules_rather_than_left_to_the_indent() {
        // Two millimetres of indent per level is invisible on the panel: on a
        // photograph of a real thread a reply and a reply-to-a-reply looked
        // the same. So every ancestor gets a rule, and the count is readable
        // even when the measure is not.
        let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
        let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
        for depth in 0..=3u8 {
            let screen = Screen::new(
                1,
                vec![Node::Quote {
                    id: NodeId(1),
                    depth,
                    role: QuoteRole::Body,
                    fold: None,
                    text: "A reply, which answers the one above it.".to_owned(),
                }],
            );
            let quote = screen
                .layout()
                .nodes
                .into_iter()
                .find(|node| matches!(node.kind, LayoutKind::Quote(..)))
                .expect("the reply was laid out");
            let mut surface = Surface::new(stride, height);
            surface.clear(tone::PAPER);
            render(&screen, &mut surface, None);

            // Count columns of rule ink to the left of the text, one row down
            // into the paragraph so no ascender can be mistaken for a rule.
            let row = usize::try_from(quote.rect.y + quote.rect.height / 2).expect("inside");
            let left = usize::try_from(quote.rect.x).expect("inside the panel");
            let mut columns = 0;
            let mut inside_rule = false;
            for column in 0..left {
                let ink = surface.pixels[row * stride + column] == tone::RULE;
                if ink && !inside_rule {
                    columns += 1;
                }
                inside_rule = ink;
            }
            assert_eq!(
                columns,
                usize::from(depth),
                "a reply at depth {depth} was marked with {columns} rules"
            );
        }
    }

    #[test]
    fn a_byline_is_set_apart_from_what_it_introduces() {
        // The author line was emitted as an ordinary paragraph: same size,
        // same ink, so a thread read as one undifferentiated column and a page
        // could open on a comment with no visible author at all.
        let body = Screen::new(
            1,
            vec![Node::Quote {
                id: NodeId(1),
                depth: 1,
                role: QuoteRole::Body,
                fold: None,
                text: "patio11 2 hours ago".to_owned(),
            }],
        );
        let byline = Screen::new(
            1,
            vec![Node::Quote {
                id: NodeId(1),
                depth: 1,
                role: QuoteRole::Byline,
                fold: None,
                text: "patio11 2 hours ago".to_owned(),
            }],
        );
        assert_eq!(QuoteRole::Byline.size(), FontSize::Caption);
        assert_eq!(QuoteRole::Byline.tone(), tone::MUTED);
        assert!(
            QuoteRole::Byline.size().tenth_mm() < QuoteRole::Body.size().tenth_mm(),
            "the byline was not smaller than the comment it introduces"
        );
        let measure = |screen: &Screen| {
            screen
                .layout()
                .nodes
                .into_iter()
                .find(|node| matches!(node.kind, LayoutKind::Quote(..)))
                .expect("the line was laid out")
                .rect
                .height
        };
        assert!(
            measure(&byline) < measure(&body),
            "the byline took as much room as a paragraph"
        );
    }

    #[test]
    fn a_page_that_opens_mid_comment_says_whose_it_is() {
        // Turning the page used to lose the author: the byline was on the page
        // before, and what followed was an unattributed wall.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let long = book(DESCRIPTION, 40);
        let mut source = vec![(1u8, QuoteRole::Byline, "patio11 2 hours ago".to_owned())];
        source.extend(
            long.split("\n\n")
                .map(|paragraph| (1, QuoteRole::Body, paragraph.to_owned())),
        );
        let borrowed = source
            .iter()
            .map(|(depth, role, text)| (*depth, *role, text.as_str()))
            .collect::<Vec<_>>();
        let pages = paginate_quoted(&borrowed, &CLARA_BW_METRICS, area);
        assert!(pages.len() > 1, "one page cannot break mid-comment");
        for (index, page) in pages.iter().enumerate().skip(1) {
            let (_, role, text) = page.first().expect("a page is never empty");
            assert_eq!(
                *role,
                QuoteRole::Byline,
                "page {index} opened on a comment with no author"
            );
            assert!(
                text.starts_with("patio11"),
                "page {index} named somebody else: {text}"
            );
        }
    }

    #[test]
    fn the_repeated_byline_is_paid_for_out_of_the_page_it_opens() {
        // A continuation line that is drawn but not measured is how the last
        // paragraph of a page ends up below the panel.
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let long = book(DESCRIPTION, 40);
        let mut source = vec![(1u8, QuoteRole::Byline, "patio11 2 hours ago".to_owned())];
        source.extend(
            long.split("\n\n")
                .map(|paragraph| (1, QuoteRole::Body, paragraph.to_owned())),
        );
        let borrowed = source
            .iter()
            .map(|(depth, role, text)| (*depth, *role, text.as_str()))
            .collect::<Vec<_>>();
        let floor = CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height();
        for (index, page) in paginate_quoted(&borrowed, &CLARA_BW_METRICS, area)
            .iter()
            .enumerate()
        {
            let (shown, bottom) = drawn_quoted(page, &CLARA_BW_METRICS);
            assert_eq!(shown, page.len(), "page {index} lost a paragraph");
            assert!(
                bottom <= floor,
                "page {index} ran {} pixels under the page controls",
                bottom - floor
            );
        }
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
                emphasis: Emphasis::Normal,
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
                .find(|node| matches!(node.kind, LayoutKind::Tile(..)))
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

    fn a_byline(fold: Option<Fold>) -> Screen {
        Screen::new(
            1,
            vec![
                Node::Quote {
                    id: NodeId(1),
                    depth: 0,
                    role: QuoteRole::Byline,
                    text: "someone 3 hours ago".to_owned(),
                    fold,
                },
                Node::Quote {
                    id: NodeId(2),
                    depth: 0,
                    role: QuoteRole::Body,
                    text: "What they said.".to_owned(),
                    fold: None,
                },
            ],
        )
    }

    #[test]
    fn a_byline_is_set_on_a_tint_and_the_words_underneath_it_are_not() {
        // Muted caption text alone was not enough to tell a name apart from a
        // sentence, so the byline gets a band. The band has to stop at the
        // byline: a comment drawn entirely on a tint is a card, and a page of
        // cards is a page with no white space in it.
        let screen = a_byline(None);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let (surface, stride) = paint(&screen);
        let byline = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
            .expect("a byline");
        let body = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Body)))
            .expect("a body");
        assert!(
            tone_count(&surface, byline.rect, tone::SURFACE, stride) > 0,
            "the byline was drawn on bare paper, so it still reads as a sentence"
        );
        assert_eq!(
            tone_count(&surface, body.rect, tone::SURFACE, stride),
            0,
            "the tint ran on past the byline and under the comment"
        );
    }

    #[test]
    fn a_fold_does_not_move_the_words_when_it_opens_and_shuts() {
        // The mark and its count are drawn in room taken out of the byline up
        // front. Sizing that room to the state would re-wrap the line under
        // the finger that just tapped it, which on a panel that takes half a
        // second to repaint reads as the text running away.
        let shut = a_byline(Some(Fold {
            action: ActionId(7),
            collapsed: true,
            hidden: 12,
        }));
        let open = a_byline(Some(Fold {
            action: ActionId(7),
            collapsed: false,
            hidden: 12,
        }));
        let lines = |screen: &Screen| {
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
                .expect("a byline")
                .text_lines
                .clone()
        };
        assert_eq!(
            lines(&shut),
            lines(&open),
            "the byline was wrapped differently open and shut"
        );

        // The rectangle is the whole column either way, because that is the
        // band the byline is drawn on. What the mark takes is taken out of the
        // words, so the claim has to be made about the words: a byline long
        // enough to fill the column has to break earlier once it is foldable,
        // or the mark is drawn over the end of it.
        let long = "somebodywithaverylongname and then a great deal more of a byline than \
                    anybody would ever write, long enough to fill the column twice over";
        let wrapped = |fold| {
            let mut screen = a_byline(fold);
            let Node::Quote { text, .. } = &mut screen.nodes[0] else {
                unreachable!("the first node is the byline")
            };
            *text = long.to_owned();
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Quote(_, QuoteRole::Byline)))
                .expect("a byline")
                .text_lines[0]
                .clone()
        };
        let foldable = wrapped(Some(Fold {
            action: ActionId(7),
            collapsed: false,
            hidden: 3,
        }));
        assert!(
            foldable.len() < wrapped(None).len(),
            "a foldable byline used the full column, so the mark is drawn over its words"
        );
    }

    #[test]
    fn the_whole_byline_is_the_handle_not_just_the_mark() {
        // A plus sign is about three millimetres across. Asking a finger to
        // find it is asking for a miss, and a miss on a thread turns the page.
        let screen = a_byline(Some(Fold {
            action: ActionId(7),
            collapsed: false,
            hidden: 3,
        }));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let strip = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::QuoteFold(..)))
            .expect("a fold handle")
            .rect;
        assert_eq!(
            layout.hit_test(strip.x + 2, strip.y + strip.height / 2),
            Some(ActionId(7)),
            "the leading edge of the byline did not fold it"
        );
        assert_eq!(
            layout.hit_test(strip.x + strip.width - 2, strip.y + strip.height / 2),
            Some(ActionId(7)),
            "the trailing edge of the byline did not fold it"
        );
    }

    #[test]
    fn a_shut_fold_says_how_much_is_behind_it_and_an_open_one_does_not() {
        let count = |collapsed| {
            a_byline(Some(Fold {
                action: ActionId(7),
                collapsed,
                hidden: 12,
            }))
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::QuoteFold(..)))
            .expect("a fold handle")
            .text_lines
            .clone()
        };
        assert_eq!(count(true), vec!["12".to_owned()]);
        assert!(
            count(false).is_empty(),
            "an open comment counted replies that are already on the page"
        );
    }

    fn with_an_overlay(kind: OverlayKind) -> Screen {
        Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(5),
                label: "Underneath".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        )
        .with_overlay(Overlay {
            id: NodeId(40),
            kind,
            title: "Delete this?".to_owned(),
            nodes: vec![Node::Button {
                id: NodeId(41),
                action: ActionId(6),
                label: "Delete".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            }],
        })
    }

    /// The width of the overlay card on a Clara.
    fn overlay_width(screen: &Screen) -> i32 {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Overlay)
            .expect("an overlay")
            .rect
            .width
    }

    /// A popover holding a menu of rows, anchored to the button underneath.
    fn with_a_menu(items: &[&str]) -> Screen {
        let mut screen = with_an_overlay(OverlayKind::Popover {
            anchor: ActionId(5),
        });
        let overlay = screen.overlay.as_mut().expect("an overlay");
        overlay.title = String::new();
        overlay.nodes = vec![Node::Rows {
            id: NodeId(41),
            rows: items
                .iter()
                .enumerate()
                .map(|(index, label)| Row {
                    action: ActionId(6 + index as u32),
                    title: (*label).to_owned(),
                    summary: String::new(),
                    lead: RowLead::Icon(Glyph::Trash),
                    state: RowState::Open,
                    trailing: None,
                    menu: None,
                })
                .collect(),
        }];
        screen
    }

    /// The rect of the first row lead on a Clara.
    fn first_lead(screen: &Screen) -> Rect {
        screen
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
            .expect("a row lead")
            .rect
    }

    /// One row carrying the given lead.
    fn row_with(lead: RowLead) -> Screen {
        Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![Row {
                    action: ActionId(2),
                    title: "A title".to_owned(),
                    summary: String::new(),
                    lead,
                    state: RowState::Open,
                    trailing: None,
                    menu: None,
                }],
            }],
        )
    }

    #[test]
    fn an_icon_leading_a_row_is_drawn_at_the_size_of_the_type() {
        // Drawn at the full width of its column an icon is about nine
        // millimetres tall beside a three and a half millimetre title, which
        // is what made a list of icons read as clip art next to its own text.
        let icon = first_lead(&row_with(RowLead::Icon(Glyph::Rss)));
        let title = CLARA_BW_METRICS.tenth_mm(FontSize::Body.tenth_mm());
        assert!(
            icon.height <= title * 3 / 2,
            "an icon {} tall leads a title {title} tall",
            icon.height
        );
    }

    #[test]
    fn the_interface_is_not_set_larger_than_the_books_beside_it() {
        // The scale is bracketed, because the failure it came from was a slow
        // one: every size was individually defensible and the set of them read
        // as a children's book. The ceiling is the largest title a mainstream
        // platform ships, 5.3 mm on iOS. The floor is a phone's caption, 1.9 mm.
        // A reader who wants more has TextScale, which is what it is for.
        assert!(FontSize::Heading.tenth_mm() <= 55);
        assert!(FontSize::Caption.tenth_mm() >= 19);
        assert!(FontSize::Caption.tenth_mm() < FontSize::Body.tenth_mm());
        assert!(FontSize::Body.tenth_mm() < FontSize::Title.tenth_mm());
        assert!(FontSize::Title.tenth_mm() < FontSize::Heading.tenth_mm());
    }

    #[test]
    fn the_large_text_setting_lands_where_the_default_used_to_be() {
        // The point of coming down is that it restores the range above. If the
        // default ever climbs back, Large stops being a step and starts being
        // the only readable setting again.
        let large = FontSize::Body.tenth_mm() * TextScale::Large.percent() / 100;
        assert!((34..=38).contains(&large), "large body is {large} tenths");
    }

    #[test]
    fn a_rank_sits_on_the_first_line_of_its_title() {
        // Centred against the row, a rank beside a two line title floated a
        // whole line below the ranks either side of it, and a column of
        // numbers at four different heights is what made a numbered list look
        // unfinished.
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![
                    Row::new(
                        ActionId(1),
                        "A headline long enough that it has to wrap onto a second line \
                         before it is done",
                        "news.example.com",
                        RowLead::Number(1),
                    ),
                    Row::new(ActionId(2), "Short", "news.example.com", RowLead::Number(2)),
                ],
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let pairs: Vec<(i32, i32)> = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::RowLead(RowLead::Number(_)) => Some((node.rect.y, node.rect.height)),
                _ => None,
            })
            .collect();
        let titles: Vec<i32> = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::RowTitle))
            .map(|node| node.rect.y)
            .collect();
        assert_eq!(pairs.len(), 2);
        assert_eq!(titles.len(), 2);
        for ((rank_y, rank_height), title_y) in pairs.into_iter().zip(titles) {
            let bottom = rank_y + rank_height;
            let first_line = title_y + FontSize::Body.line_height();
            assert_eq!(
                bottom,
                first_line,
                "a rank sat {} from the foot of its title's first line",
                first_line - bottom
            );
        }
    }

    #[test]
    fn a_cover_still_fills_the_column_it_leads_with() {
        // The size rule is about labels, not content. A cover is the row.
        let cover = first_lead(&row_with(RowLead::Picture(
            TilePicture {
                handle: PictureHandle(1),
                source: (60, 90),
            },
            Glyph::Book,
        )));
        assert_eq!(cover.width, CLARA_BW_METRICS.touch_target_default());
    }

    #[test]
    fn a_mark_fills_the_column_a_list_of_marks_is_given() {
        // The column is sized to the mark rather than to a finger, so there is
        // nothing left over to centre the mark in. What matters is that it
        // starts at the margin: a gutter wider than its contents is a tenth of
        // this panel spent on nothing, which is what it was.
        let icon = first_lead(&row_with(RowLead::Icon(Glyph::Rss)));
        assert_eq!(icon.x, CLARA_BW_METRICS.screen_margin());
        assert_eq!(icon.width, row_mark_column(&CLARA_BW_METRICS));
        assert!(
            icon.width < CLARA_BW_METRICS.touch_target_default(),
            "a mark column of {} is still a finger wide",
            icon.width
        );
    }

    #[test]
    fn a_list_with_a_cover_in_it_keeps_one_text_margin() {
        // A cover needs the wide column and a mark does not, but a list where
        // some titles start further left than others is worse than a wide
        // gutter, so one cover widens the column for every row in the list.
        let mixed = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![
                    Row::new(
                        ActionId(1),
                        "With a cover",
                        "",
                        RowLead::Picture(
                            TilePicture {
                                handle: PictureHandle(1),
                                source: (60, 90),
                            },
                            Glyph::Book,
                        ),
                    ),
                    Row::new(ActionId(2), "With a mark", "", RowLead::Icon(Glyph::Book)),
                ],
            }],
        );
        let starts: Vec<i32> = mixed
            .layout_with(&CLARA_BW_METRICS, &Chrome::default())
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::RowTitle)
            .map(|node| node.rect.x)
            .collect();
        assert_eq!(starts.len(), 2);
        assert_eq!(starts[0], starts[1]);
        assert_eq!(
            starts[0],
            CLARA_BW_METRICS.screen_margin()
                + CLARA_BW_METRICS.touch_target_default()
                + CLARA_BW_METRICS.space(Space::Small)
        );
    }

    #[test]
    fn a_list_of_marks_starts_its_text_at_the_mark_column() {
        let with_icon = row_with(RowLead::Icon(Glyph::Rss));
        let title = |screen: &Screen| {
            screen
                .layout_with(&CLARA_BW_METRICS, &Chrome::default())
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::RowTitle)
                .expect("a title")
                .rect
        };
        let expected = CLARA_BW_METRICS.screen_margin()
            + row_mark_column(&CLARA_BW_METRICS)
            + CLARA_BW_METRICS.space(Space::Small);
        assert_eq!(title(&with_icon).x, expected);
    }

    #[test]
    fn a_popover_is_only_as_wide_as_what_is_in_it() {
        // A menu of one short word used to be given the same width as a
        // dialogue, so pressing a three dot mark produced a band most of the
        // way across the panel and read as the screen having changed.
        let narrow = overlay_width(&with_a_menu(&["Delete"]));
        let wide = overlay_width(&with_a_menu(&[
            "Stop following this feed and forget everything it ever said",
        ]));
        assert!(
            narrow < wide,
            "a one word menu was as wide as a sentence: {narrow} against {wide}"
        );
        assert!(
            narrow < CLARA_BW_METRICS.width / 2,
            "a one word menu took half the panel: {narrow}"
        );
    }

    #[test]
    fn a_popover_never_gets_narrower_than_a_finger() {
        let width = overlay_width(&with_a_menu(&["No"]));
        assert!(
            width >= 3 * CLARA_BW_METRICS.touch_target_default(),
            "a two letter menu was {width}, too narrow to press or to carry a caret"
        );
    }

    #[test]
    fn a_popover_never_gets_wider_than_a_modal() {
        let sentence = "Stop following this feed and forget everything it ever said, \
                        including the parts nobody read, at once and for good";
        let width = overlay_width(&with_a_menu(&[sentence]));
        let modal = overlay_width(&with_an_overlay(OverlayKind::Modal));
        assert_eq!(
            width, modal,
            "a long menu item pushed the popover past the width a modal is allowed"
        );
    }

    #[test]
    fn a_modal_still_takes_the_room_a_dialogue_needs() {
        // Deliberately not measured from its contents. A dialogue asks a
        // question in prose, and prose set in a box the width of its longest
        // button is a column of two words.
        let terse = overlay_width(&with_an_overlay(OverlayKind::Modal));
        let mut wordy = with_an_overlay(OverlayKind::Modal);
        wordy.overlay.as_mut().expect("an overlay").title =
            "Delete this feed and everything in it?".to_owned();
        assert_eq!(terse, overlay_width(&wordy));
    }

    #[test]
    fn an_overlay_leaves_the_screen_it_covers_alone() {
        // Dimming the backdrop is the usual way to focus attention on a
        // dialogue and the one thing this panel cannot afford: shading every
        // pixel forces a full refresh to put it up and a second one to take it
        // down. The overlay has to earn its separation with a border instead.
        let screen = with_an_overlay(OverlayKind::Modal);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let card = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Overlay)
            .expect("an overlay")
            .rect;
        let (covered, stride) = paint(&screen);
        let mut bare = screen.clone();
        bare.overlay = None;
        let (bare, _) = paint(&bare);
        // Everything above the card, pixel for pixel. Not "all paper": the
        // button underneath is drawn up there and has to still be drawn,
        // which is the whole claim.
        let above = card.y.max(1) as usize * stride;
        assert_eq!(
            covered.pixels[..above],
            bare.pixels[..above],
            "the screen under the overlay was altered, so putting the overlay up and \
             taking it down each cost a full refresh"
        );
    }

    #[test]
    fn a_tap_beside_a_modal_answers_nothing() {
        // "Somewhere else" is not one of the choices, and a reader who brushes
        // the panel has not agreed to delete anything.
        let screen = with_an_overlay(OverlayKind::Modal);
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.hit_test(2, CLARA_BW_METRICS.height - 2),
            None,
            "a tap outside a modal reached a control the reader could not see"
        );
    }

    #[test]
    fn a_tap_beside_a_popover_puts_it_away() {
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(5),
                label: "More".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        )
        .with_overlay(Overlay::popover(
            NodeId(40),
            ActionId(5),
            vec![Node::Button {
                id: NodeId(41),
                action: ActionId(6),
                label: "Share".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.hit_test(2, CLARA_BW_METRICS.height - 2),
            Some(ActionId::BACK),
            "a tap outside a popover did not dismiss it"
        );
    }

    #[test]
    fn a_popover_points_at_the_control_that_opened_it() {
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(5),
                label: "More".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        )
        .with_overlay(Overlay::popover(
            NodeId(40),
            ActionId(5),
            vec![Node::Button {
                id: NodeId(41),
                action: ActionId(6),
                label: "Share".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }],
        ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let anchor = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(ActionId(5), _, _)))
            .expect("the anchor")
            .rect;
        let caret = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::OverlayCaret(_)))
            .expect("a caret")
            .rect;
        let centre = caret.x + caret.width / 2;
        assert!(
            centre >= anchor.x && centre <= anchor.x + anchor.width,
            "the caret pointed at {centre}, which is not anywhere on the control at \
             {}..{}",
            anchor.x,
            anchor.x + anchor.width
        );
    }

    #[test]
    fn an_overlay_with_nothing_to_point_at_is_still_shown() {
        // Naming an anchor that is not on the screen is an application bug.
        // Dropping the overlay loses whatever it was going to ask, which is a
        // worse answer than centring it.
        let screen = Screen::new(1, vec![])
            .with_overlay(Overlay::popover(NodeId(40), ActionId(999), vec![]))
            .with_overlay(Overlay::popover(
                NodeId(40),
                ActionId(999),
                vec![Node::Text {
                    id: NodeId(41),
                    text: "Something".to_owned(),
                }],
            ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::Overlay),
            "an overlay with a missing anchor was dropped"
        );
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::OverlayCaret(_))),
            "a caret was drawn pointing at nothing"
        );
    }

    #[test]
    fn an_overlay_takes_the_page_turns_away() {
        // The zones are whatever is left of the content area, and what is left
        // must not include the thing drawn on top of it: a reader answering a
        // question would otherwise turn the page underneath instead.
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A page of a book.".to_owned(),
            }],
        )
        .with_page_turns(ActionId(2), ActionId(3))
        .with_overlay(Overlay::modal(
            NodeId(40),
            "Leave?",
            vec![Node::Button {
                id: NodeId(41),
                action: ActionId(6),
                label: "Leave".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            }],
        ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(layout.hit_page_turn(4, CLARA_BW_METRICS.height / 2), None);
    }

    /// A modal is deliberately not dismissed by a tap that misses it, so
    /// without a cross it is a screen with no way off it unless the
    /// application remembered to put one in a row. The frame draws it, so the
    /// application cannot forget.
    #[test]
    fn a_modal_always_carries_a_way_out() {
        for (name, metrics) in PANELS {
            for title in ["", "Delete this book?"] {
                let screen = Screen::new(1, Vec::new()).with_overlay(Overlay {
                    kind: OverlayKind::Modal,
                    id: NodeId(2),
                    title: title.into(),
                    nodes: vec![Node::Text {
                        id: NodeId(3),
                        text: "This cannot be undone.".into(),
                    }],
                });
                let layout = screen.layout_with(&metrics, &Chrome::with_back(false));
                let cross = layout
                    .nodes
                    .iter()
                    .find(|node| node.kind == LayoutKind::OverlayClose)
                    .unwrap_or_else(|| panic!("{name}: a modal with no way out"));
                assert_eq!(
                    layout.hit_control(
                        cross.rect.x + cross.rect.width / 2,
                        cross.rect.y + cross.rect.height / 2
                    ),
                    Some(ActionId::BACK),
                    "{name}: the cross does not answer"
                );
                let target = metrics.touch_target_default();
                assert!(
                    cross.rect.width >= target && cross.rect.height >= target,
                    "{name}: the cross is smaller than a finger"
                );
                // Under the cross rather than beside it is how a title ends up
                // unreadable, so the two never share pixels.
                if let Some(heading) = layout
                    .nodes
                    .iter()
                    .find(|node| node.kind == LayoutKind::OverlayTitle)
                {
                    assert!(
                        heading.rect.x + heading.rect.width <= cross.rect.x,
                        "{name}: the title runs under the cross"
                    );
                }
            }
        }
    }

    /// A popover needs no cross: a tap anywhere off it puts it away, and a
    /// second way out beside the control that opened it is noise.
    #[test]
    fn a_popover_carries_no_cross() {
        let screen = Screen::new(1, Vec::new())
            .with_top_bar(TopBar::new(NodeId(1), "Title").action(ActionId(9), "Aa"))
            .with_overlay(Overlay {
                kind: OverlayKind::Popover {
                    anchor: ActionId(9),
                },
                id: NodeId(2),
                title: "Type size".into(),
                nodes: vec![Node::Text {
                    id: NodeId(3),
                    text: "Standard".into(),
                }],
            });
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::OverlayClose),
            "a popover drew a cross it does not need"
        );
    }

    /// A popover hung off a picture control has to find that control, and the
    /// control has to stay pressable so a second tap puts the panel away.
    ///
    /// Both failed together and neither was visible in the screen: a bar glyph
    /// was added to the hit test and to the renderer but not to
    /// `LayoutKind::acts_on`, so the anchor lookup came back empty, the
    /// popover quietly fell back to a centred modal, and the modal's scrim sat
    /// over the control that opened it. On the panel that read as a front
    /// light menu in the middle of the page that could not be dismissed, while
    /// the type menu beside it -- a word, not a picture -- worked.
    #[test]
    fn a_popover_hung_off_a_picture_control_finds_it_and_leaves_it_pressable() {
        let glyph = ActionId(31);
        let screen = Screen::new(1, Vec::new())
            .with_top_bar(
                TopBar::new(NodeId(1), "3 of 40")
                    .with_action(BarAction::new(glyph, "Front light").with_glyph(Glyph::Light)),
            )
            .with_overlay(Overlay {
                kind: OverlayKind::Popover { anchor: glyph },
                id: NodeId(2),
                title: "Front light 20%".into(),
                nodes: vec![Node::Button {
                    id: NodeId(3),
                    action: ActionId(32),
                    label: "Brighter".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                }],
            });
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
        let anchor = layout
            .rect_of_action(glyph)
            .expect("the control the panel hangs off");
        let box_rect = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Overlay)
            .map(|node| node.rect)
            .expect("the panel");
        // Under the control it belongs to, not adrift in the middle of the
        // page, which is what a silent fall back to a modal looks like.
        assert!(
            box_rect.y >= anchor.y + anchor.height,
            "the panel did not hang off its control: anchor {anchor:?}, panel {box_rect:?}"
        );
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::OverlayCaret(_))),
            "a panel that hangs off a control draws the mark that says so"
        );
        // A tap on the control that opened it lands on the scrim, and a
        // popover's scrim answers a miss with Back, which is what puts it
        // away. That is the whole mechanism, and it is the half the fall back
        // to a modal silently disabled -- a modal is deliberately *not*
        // dismissed by a miss, so the panel became something that could only
        // be left by tapping one of its own rows.
        assert_eq!(
            layout.hit_control(anchor.x + anchor.width / 2, anchor.y + anchor.height / 2),
            Some(ActionId::BACK),
            "a tap on the control that opened the panel does not put it away"
        );
    }

    #[test]
    fn a_popover_too_tall_for_either_side_draws_no_caret() {
        // On a Clara the reading panel could not fit below its control in the
        // top bar or above it, so it was clamped into the middle of the screen
        // -- covering the control -- and still drew a caret, at the far bottom
        // corner, pointing down at the page. A mark that points at nothing is
        // read as pointing at whatever is nearest.
        let tall = (0..14)
            .map(|index| Node::Button {
                id: NodeId(100 + index),
                action: ActionId(100 + index),
                label: format!("Choice {index}"),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            })
            .collect::<Vec<_>>();
        let screen = Screen::new(1, Vec::new())
            .with_top_bar(TopBar::new(NodeId(1), "Title").action(ActionId(9), "Aa"))
            .with_overlay(Overlay::popover(NodeId(40), ActionId(9), tall));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
        let anchor = layout
            .rect_of_action(ActionId(9))
            .expect("the control the panel hangs off");
        let box_rect = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Overlay)
            .map(|node| node.rect)
            .expect("the panel");
        let caret = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::OverlayCaret(_)));
        let overlaps =
            box_rect.y < anchor.y + anchor.height && anchor.y < box_rect.y + box_rect.height;
        assert!(overlaps, "this case is only interesting when they overlap");
        assert!(
            caret.is_none(),
            "a caret was drawn on a panel that covers its own anchor"
        );
    }

    #[test]
    fn a_hold_is_the_content_area_and_never_a_control() {
        // A reader resting a thumb on a button must get the button. The whole
        // point of the gesture is that a page of prose has nowhere else to put
        // one, which is an argument about empty space, not about controls.
        let screen = Screen::new(
            1,
            vec![
                Node::Text {
                    id: NodeId(1),
                    text: "A page of a book.".to_owned(),
                },
                Node::Button {
                    id: NodeId(2),
                    action: ActionId(7),
                    label: "Notes".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
            ],
        )
        .with_hold(ActionId(9));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let button = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(ActionId(7), _, _)))
            .expect("the button");
        assert_eq!(
            layout.hit_hold(
                button.rect.x + button.rect.width / 2,
                button.rect.y + button.rect.height / 2
            ),
            None,
            "holding a control was taken as a hold on the page"
        );
        assert_eq!(
            layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y + 4),
            Some(ActionId(9))
        );
        // Above the content is the top bar's business, not the page's.
        assert_eq!(
            layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y - 1),
            None
        );
    }

    #[test]
    fn an_overlay_takes_the_hold_away() {
        // Same argument as the page turns: what is left over must not include
        // what is drawn on top of it, or holding a finger on a panel would
        // reach through it into the book.
        let screen = Screen::new(
            1,
            vec![Node::Text {
                id: NodeId(1),
                text: "A page of a book.".to_owned(),
            }],
        )
        .with_hold(ActionId(9))
        .with_overlay(Overlay::popover(
            NodeId(40),
            ActionId(999),
            vec![Node::Text {
                id: NodeId(41),
                text: "Type size".to_owned(),
            }],
        ));
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert_eq!(
            layout.hit_hold(CLARA_BW_METRICS.width / 2, layout.content.y + 4),
            None
        );
    }

    #[test]
    fn a_row_cover_falls_back_to_its_glyph_until_the_picture_arrives() {
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![Row::new(
                    ActionId(1),
                    "Bleak House",
                    "Charles Dickens",
                    RowLead::Picture(TilePicture::new(PictureHandle(7), 190, 300), Glyph::Book),
                )],
            }],
        );
        let lead = screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| matches!(node.kind, LayoutKind::RowLead(_)))
            .expect("a row lead");
        let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
        let height = usize::try_from(CLARA_BW_METRICS.height).expect("a positive height");
        let inked = |cache: &PictureCache| {
            let mut surface = Surface::new(stride, height);
            surface.clear(tone::PAPER);
            render_all(
                &screen,
                &CLARA_BW_METRICS,
                &Chrome::with_back(false),
                cache,
                &mut surface,
                None,
            );
            (lead.rect.y..lead.rect.y + lead.rect.height)
                .flat_map(|row| {
                    (lead.rect.x..lead.rect.x + lead.rect.width).map(move |column| (column, row))
                })
                .filter(|&(column, row)| {
                    let (Ok(row), Ok(column)) = (usize::try_from(row), usize::try_from(column))
                    else {
                        return false;
                    };
                    surface.pixels[row * stride + column] != tone::PAPER
                })
                .count()
        };
        // Nothing in the cache: the glyph still draws, so the row is usable
        // while the covers are on their way.
        let empty = PictureCache::default();
        assert!(inked(&empty) > 0, "a row with no cover yet drew nothing");
        let mut cache = PictureCache::default();
        assert!(cache.put(PictureHandle(7), 19, 30, vec![0; 19 * 30]));
        assert!(
            inked(&cache) > inked(&empty),
            "the cover was not drawn once it had arrived"
        );
    }

    #[test]
    fn a_screen_with_no_bottom_bar_still_keeps_the_bezel_margin() {
        // The sides get a screen margin and the bottom got nothing, so the
        // last control on a screen with no bar under it was drawn onto the
        // final rows of the panel: on the reader the Add button's border was
        // the bottom edge of the glass.
        let screen = Screen::new(
            1,
            vec![
                Node::Text {
                    id: NodeId(1),
                    text: "Nothing to do.".into(),
                },
                Node::Button {
                    id: NodeId(2),
                    action: ActionId(1),
                    label: "Add".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
            ],
        )
        .with_top_bar(TopBar::new(NodeId(0), "Todo"));
        let layout = screen.layout();
        let floor = CLARA_BW_METRICS.height - CLARA_BW_METRICS.screen_margin();
        for node in &layout.nodes {
            assert!(
                node.rect.y + node.rect.height <= floor,
                "{:?} ran to {}, past the bezel margin at {floor}",
                node.kind,
                node.rect.y + node.rect.height
            );
        }
    }

    #[test]
    fn a_page_position_is_drawn_and_takes_its_room_from_the_content() {
        let build = |position: Option<(u16, u16)>| {
            let mut screen = Screen::new(
                1,
                vec![Node::Text {
                    id: NodeId(1),
                    text: "A page of prose.".into(),
                }],
            )
            .with_page_turns(ActionId(1), ActionId(2));
            if let Some((page, of)) = position {
                screen.page_turns = screen.page_turns.map(|turns| turns.with_position(page, of));
            }
            screen.layout()
        };
        let without = build(None);
        let with = build(Some((4, 12)));
        let shown = with
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::PagePosition)
            .expect("a page position");
        assert_eq!(shown.text_lines[0], "4 of 12");
        assert!(
            !without
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::PagePosition),
            "a screen that asked for no position got one anyway"
        );
        assert!(
            with.content.height < without.content.height,
            "the position band was drawn over the content instead of reserved"
        );
        assert!(
            shown.rect.y >= with.content.y + with.content.height,
            "the position was drawn inside the page-turn zone it describes"
        );
    }

    #[test]
    fn each_page_turn_is_offered_only_where_there_is_a_page_to_turn_to() {
        // The side-tap zones are invisible, so a paginated screen used to say
        // which page it was on and nothing about how to leave it.
        let turns = |page: u16| {
            let mut screen = Screen::new(
                1,
                vec![Node::Text {
                    id: NodeId(1),
                    text: "A page of prose.".into(),
                }],
            )
            .with_page_turns(ActionId(7), ActionId(9));
            screen.page_turns = screen.page_turns.map(|turns| turns.with_position(page, 3));
            let layout = screen.layout();
            let seen: Vec<LayoutKind> = layout
                .nodes
                .iter()
                .map(|node| node.kind)
                .filter(|kind| {
                    matches!(kind, LayoutKind::PagePrevious(_) | LayoutKind::PageNext(_))
                })
                .collect();
            (layout, seen)
        };
        let (_, first) = turns(1);
        assert_eq!(first, vec![LayoutKind::PageNext(ActionId(9))]);
        let (middle_layout, middle) = turns(2);
        assert_eq!(
            middle,
            vec![
                LayoutKind::PagePrevious(ActionId(7)),
                LayoutKind::PageNext(ActionId(9))
            ]
        );
        let (_, last) = turns(3);
        assert_eq!(last, vec![LayoutKind::PagePrevious(ActionId(7))]);

        // And they are controls, not decoration: a tap lands on the action
        // rather than falling through to the zone underneath.
        let previous = middle_layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::PagePrevious(_)))
            .expect("a previous control");
        assert_eq!(
            middle_layout.hit_test(
                previous.rect.x + previous.rect.width / 2,
                previous.rect.y + previous.rect.height / 2,
            ),
            Some(ActionId(7))
        );
        assert!(
            previous.rect.height >= DisplayMetrics::default().touch_target_minimum(),
            "the page controls are too small to hit: {:?}",
            previous.rect
        );
    }

    #[test]
    fn a_page_position_nobody_can_compute_is_left_unsaid() {
        for position in [(0, 0), (1, 0), (13, 12)] {
            let mut screen = Screen::new(1, Vec::new()).with_page_turns(ActionId(1), ActionId(2));
            screen.page_turns = screen
                .page_turns
                .map(|turns| turns.with_position(position.0, position.1));
            assert!(
                !screen
                    .layout()
                    .nodes
                    .iter()
                    .any(|node| node.kind == LayoutKind::PagePosition),
                "{position:?} was drawn rather than left unsaid"
            );
        }
    }

    #[test]
    fn an_empty_field_shows_its_placeholder_muted_and_a_filled_one_shows_ink() {
        let laid_out = |value: &str| {
            Screen::new(
                1,
                vec![Node::Field {
                    id: NodeId(1),
                    action: ActionId(1),
                    value: value.into(),
                    placeholder: "Search the library".into(),
                    clear: None,
                }],
            )
            .layout()
        };
        let empty = laid_out("");
        let filled = laid_out("dickens");
        let value = |layout: &Layout| {
            layout
                .nodes
                .iter()
                .find_map(|node| match node.kind {
                    LayoutKind::FieldValue(empty) => Some((empty, node.text_lines[0].clone())),
                    _ => None,
                })
                .expect("a field value")
        };
        assert_eq!(value(&empty), (true, "Search the library".to_owned()));
        assert_eq!(value(&filled), (false, "dickens".to_owned()));
    }

    #[test]
    fn a_field_cross_never_overlaps_the_text_it_would_clear() {
        let layout = Screen::new(
            1,
            vec![Node::Field {
                id: NodeId(1),
                action: ActionId(1),
                value: "a query long enough to run the whole width of the panel".into(),
                placeholder: String::new(),
                clear: Some(ActionId(2)),
            }],
        )
        .layout();
        let find = |wanted: fn(LayoutKind) -> bool| {
            layout
                .nodes
                .iter()
                .find(|node| wanted(node.kind))
                .expect("a field part")
                .rect
        };
        let value = find(|kind| matches!(kind, LayoutKind::FieldValue(_)));
        let clear = find(|kind| matches!(kind, LayoutKind::FieldClear(_)));
        assert!(
            value.x + value.width <= clear.x,
            "the query ran underneath the cross that clears it"
        );
        assert_eq!(
            layout.hit_test(clear.x + clear.width / 2, clear.y + clear.height / 2),
            Some(ActionId(2)),
            "the cross did not answer a tap"
        );
    }

    #[test]
    fn chips_wrap_onto_another_row_rather_than_off_the_panel() {
        let chips: Vec<Chip> = (0..MAX_CHIPS)
            .map(|index| {
                Chip::new(
                    ActionId(index as u32 + 1),
                    format!("Subject number {index}"),
                )
            })
            .collect();
        let screen = Screen::new(
            1,
            vec![Node::Chips {
                id: NodeId(1),
                chips,
            }],
        );
        let layout = screen.layout();
        let placed: Vec<_> = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Chip(..)))
            .map(|node| node.rect)
            .collect();
        assert_eq!(placed.len(), MAX_CHIPS, "chips went missing");
        for chip in &placed {
            assert!(
                chip.x >= layout.content.x
                    && chip.x + chip.width <= layout.content.x + layout.content.width,
                "a chip ran off the side of the panel"
            );
        }
        let rows = placed
            .iter()
            .map(|rect| rect.y)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(rows.len() > 1, "sixteen chips were crammed onto one row");
    }

    #[test]
    fn chips_past_the_cap_are_reported_rather_than_quietly_clipped() {
        let chips: Vec<Chip> = (0..=MAX_CHIPS)
            .map(|index| Chip::new(ActionId(index as u32 + 1), "Subject"))
            .collect();
        let screen = Screen::new(
            1,
            vec![Node::Chips {
                id: NodeId(1),
                chips,
            }],
        );
        assert!(
            screen
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| matches!(issue.kind, LayoutIssueKind::CollectionTruncated { .. })),
            "a chip run past its cap was silently truncated"
        );
    }

    #[test]
    fn a_tab_strip_fills_the_panel_exactly_and_marks_one_tab() {
        let screen = Screen::new(
            1,
            vec![Node::Tabs {
                id: NodeId(1),
                tabs: vec![
                    Chip::new(ActionId(1), "Discover"),
                    Chip::new(ActionId(2), "Popular"),
                    Chip::new(ActionId(3), "Subjects"),
                ],
                selected: 2,
            }],
        );
        let layout = screen.layout();
        let tabs: Vec<_> = layout
            .nodes
            .iter()
            .filter_map(|node| match node.kind {
                LayoutKind::Tab(action, selected) => Some((action, selected, node.rect)),
                _ => None,
            })
            .collect();
        assert_eq!(tabs.len(), 3);
        // Measured against a full-width node on the same panel rather than
        // against the content rect, because the strip is placed inside the
        // screen margin and the content rect is outside it.
        let rule = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TabRule)
            .expect("a strip rule")
            .rect;
        let first = tabs[0].2;
        let last = tabs[2].2;
        assert_eq!(first.x, rule.x, "the strip did not start at the margin");
        assert_eq!(
            last.x + last.width,
            rule.x + rule.width,
            "the rounding remainder left a gap at the end of the strip"
        );
        assert_eq!(
            tabs[1].2.x,
            first.x + first.width,
            "the tabs were not laid edge to edge"
        );
        let marked: Vec<_> = tabs.iter().filter(|(_, selected, _)| *selected).collect();
        assert_eq!(marked.len(), 1, "exactly one tab must ever be current");
        assert_eq!(marked[0].0, ActionId(3));
    }

    #[test]
    fn a_tab_selection_nobody_named_falls_back_to_the_first() {
        let screen = Screen::new(
            1,
            vec![Node::Tabs {
                id: NodeId(1),
                tabs: vec![Chip::new(ActionId(1), "One"), Chip::new(ActionId(2), "Two")],
                selected: 99,
            }],
        );
        let marked = screen
            .layout()
            .nodes
            .into_iter()
            .find_map(|node| match node.kind {
                LayoutKind::Tab(action, true) => Some(action),
                _ => None,
            })
            .expect("a current tab");
        assert_eq!(
            marked,
            ActionId(1),
            "an out-of-range selection left the strip with no current tab"
        );
    }

    #[test]
    fn an_unavailable_tile_keeps_its_place_and_stops_answering_taps() {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape: TileShape::Square,
                tiles: vec![
                    Tile::new(ActionId(1), "Here", Glyph::Book),
                    Tile::new(ActionId(2), "Gone", Glyph::Book).with_state(TileState::Unavailable),
                ],
            }],
        );
        let layout = screen.layout();
        let cells: Vec<_> = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .collect();
        assert_eq!(cells.len(), 2, "an unavailable tile lost its place");
        let gone = cells[1].rect;
        assert_eq!(
            layout.hit_test(gone.x + gone.width / 2, gone.y + gone.height / 2),
            None,
            "an unavailable tile answered a tap"
        );
        let here = cells[0].rect;
        assert_eq!(
            layout.hit_test(here.x + here.width / 2, here.y + here.height / 2),
            Some(ActionId(1)),
            "an available tile beside it stopped answering too"
        );
    }

    #[test]
    fn one_subtitle_gives_every_tile_in_the_grid_the_same_extra_room() {
        let heights = |subtitled: bool| {
            let mut second = Tile::new(ActionId(2), "Second", Glyph::Book);
            if subtitled {
                second = second.with_subtitle("Charles Dickens");
            }
            let screen = Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    shape: TileShape::Square,
                    tiles: vec![Tile::new(ActionId(1), "First", Glyph::Book), second],
                }],
            );
            let layout = screen.layout();
            layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
                .map(|node| node.rect.height)
                .collect::<Vec<_>>()
        };
        let plain = heights(false);
        let tall = heights(true);
        assert_eq!(plain[0], plain[1], "a plain grid was already ragged");
        assert_eq!(
            tall[0], tall[1],
            "one subtitle made the grid ragged: cells in a grid are the same height"
        );
        assert!(
            tall[0] > plain[0],
            "a subtitle was given no room to be drawn in"
        );
    }

    #[test]
    fn a_state_and_a_badge_take_opposite_corners_of_their_own_tile() {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape: TileShape::Square,
                tiles: vec![Tile::new(ActionId(1), "Bleak House", Glyph::Book)
                    .with_state(TileState::Held)
                    .with_badge("12")],
            }],
        );
        let layout = screen.layout();
        let find = |wanted: fn(LayoutKind) -> bool| {
            layout
                .nodes
                .iter()
                .find(|node| wanted(node.kind))
                .expect("a corner chip")
                .rect
        };
        let cell = find(|kind| matches!(kind, LayoutKind::Tile(..)));
        let state = find(|kind| matches!(kind, LayoutKind::TileState(_)));
        let badge = find(|kind| kind == LayoutKind::TileBadge);
        assert!(
            badge.x < state.x,
            "the badge and the state marker piled into the same corner"
        );
        for chip in [state, badge] {
            assert!(
                chip.x >= cell.x
                    && chip.y >= cell.y
                    && chip.x + chip.width <= cell.x + cell.width
                    && chip.y + chip.height <= cell.y + cell.height,
                "a corner chip hung outside its own tile"
            );
        }
    }

    #[test]
    fn a_badge_is_clamped_rather_than_allowed_to_become_a_label() {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape: TileShape::Square,
                tiles: vec![Tile::new(ActionId(1), "Shelf", Glyph::Book)
                    .with_badge("far too many characters")],
            }],
        );
        let badge = screen
            .layout()
            .nodes
            .into_iter()
            .find(|node| node.kind == LayoutKind::TileBadge)
            .expect("a badge");
        assert_eq!(
            badge.text_lines[0].chars().count(),
            TILE_BADGE_LIMIT,
            "a badge was allowed to grow into a label"
        );
    }

    #[test]
    fn a_tile_with_nothing_extra_to_say_emits_no_chips_at_all() {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape: TileShape::Square,
                tiles: vec![Tile::new(ActionId(1), "Plain", Glyph::Book)],
            }],
        );
        assert!(
            !screen.layout().nodes.iter().any(|node| matches!(
                node.kind,
                LayoutKind::TileState(_) | LayoutKind::TileBadge | LayoutKind::TileSubtitle
            )),
            "an unadorned tile still paid for decoration it did not ask for"
        );
    }

    #[test]
    fn pagination_leaves_room_for_a_trailing_value_it_will_have_to_draw() {
        let metrics = CLARA_BW_METRICS;
        let area = ProseArea {
            width: 900,
            height: 900,
            gap: metrics.space(Space::Small),
            face: Face::Text,
        };
        let title = "A long headline about a small program that reads the news slowly \
                     and turns its pages by itself";
        let summary = "example.com, 4 hours ago, 12 comments";
        let plain = vec![(title, summary); 12];
        let scored = vec![(title, summary, "1,284 points and 312 comments"); 12];
        let without = paginate_rows(&plain, &metrics, area);
        let with = paginate_rows_with_trailing(&scored, &metrics, area);
        assert!(
            with[0].len() < without[0].len(),
            "the score took no room from the page: {} rows either way",
            with[0].len()
        );
        // What the row will really be: the same measurement the layout engine
        // makes, which is the thing the old pagination disagreed with.
        assert!(
            measured_row_height(
                &metrics,
                area,
                title,
                summary,
                "1,284 points and 312 comments",
                false,
                row_mark_column(&metrics)
            ) > measured_row_height(
                &metrics,
                area,
                title,
                summary,
                "",
                false,
                row_mark_column(&metrics)
            ),
            "a row with a value at its trailing edge measured no taller"
        );
    }

    #[test]
    fn a_rows_trailing_value_keeps_its_room_and_the_title_gives_up_its_own() {
        let laid_out = |title: &str, trailing: Option<&str>| {
            let mut row = Row::new(ActionId(1), title, "Charles Dickens", Glyph::Book);
            if let Some(value) = trailing {
                row = row.with_trailing(value);
            }
            let screen = Screen::new(
                1,
                vec![Node::Rows {
                    id: NodeId(1),
                    rows: vec![row],
                }],
            );
            screen.layout()
        };
        let long = "A title long enough that it would happily take the whole row";
        let without = laid_out(long, None);
        let with = laid_out(long, Some("18,204"));
        let title_width = |layout: &Layout| {
            layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::RowTitle)
                .expect("a title")
                .rect
                .width
        };
        assert!(
            title_width(&with) < title_width(&without),
            "a trailing value did not take any room from the title"
        );
        let value = with
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowTrailing)
            .expect("a trailing value");
        assert_eq!(
            value.text_lines.first().map(String::as_str),
            Some("18,204"),
            "the trailing value was clamped away"
        );
        let title = with
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowTitle)
            .expect("a title");
        assert!(
            title.rect.x + title.rect.width <= value.rect.x,
            "the title ran underneath its own trailing value"
        );
    }

    #[test]
    fn an_empty_trailing_value_draws_nothing_at_all() {
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![Row::new(ActionId(1), "Counter", "", Glyph::Note).with_trailing("")],
            }],
        );
        assert!(
            !screen
                .layout()
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::RowTrailing),
            "an empty trailing value still reserved a column"
        );
    }

    /// A tap on the dots must never also be a tap on the row. The two are the
    /// same rectangle otherwise, and opening a feed while asking to remove it
    /// is the worst possible reading of one press.
    #[test]
    fn a_row_menu_takes_the_tap_before_the_row_it_sits_in() {
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![
                    Row::new(ActionId(1), "Ars Technica", "arstechnica.com", Glyph::Rss)
                        .with_menu(ActionId(2)),
                ],
            }],
        );
        let layout = screen.layout();
        let mark = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::RowMenu(ActionId(2)))
            .expect("an overflow mark");
        let row = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Row(ActionId(1)))
            .expect("a row");
        assert_eq!(
            layout.hit_test(
                mark.rect.x + mark.rect.width / 2,
                mark.rect.y + mark.rect.height / 2
            ),
            Some(ActionId(2)),
            "the row swallowed a tap meant for its own overflow mark"
        );
        assert_eq!(
            layout.hit_test(row.rect.x + 10, row.rect.y + row.rect.height / 2),
            Some(ActionId(1)),
            "the mark swallowed a tap on the row"
        );
        assert_eq!(
            layout.pressed_control(
                mark.rect.x + mark.rect.width / 2,
                mark.rect.y + mark.rect.height / 2
            ),
            Some(mark.rect),
            "pressing the mark inverted the whole row"
        );
    }

    /// The mark keeps its column and the title wraps into what is left, on the
    /// same reasoning as a trailing value: a title measured at the full row
    /// width is drawn under the dots.
    #[test]
    fn a_row_menu_takes_its_room_from_the_title() {
        let long = "A headline long enough that it has to wrap somewhere on this panel";
        let laid_out = |menu: bool| {
            let row = Row::new(ActionId(1), long, "", Glyph::Rss);
            Screen::new(
                1,
                vec![Node::Rows {
                    id: NodeId(1),
                    rows: vec![if menu {
                        row.with_menu(ActionId(2))
                    } else {
                        row
                    }],
                }],
            )
            .layout()
        };
        let title = |layout: &Layout| {
            *layout
                .nodes
                .iter()
                .find(|node| node.kind == LayoutKind::RowTitle)
                .map(|node| &node.rect)
                .expect("a title")
        };
        let with = laid_out(true);
        assert!(
            title(&with).width < title(&laid_out(false)).width,
            "an overflow mark did not take any room from the title"
        );
        let mark = with
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::RowMenu(_)))
            .expect("an overflow mark");
        let title = title(&with);
        assert!(
            title.x + title.width <= mark.rect.x,
            "the title ran underneath its own overflow mark"
        );
        assert!(
            mark.rect.width >= CLARA_BW_METRICS.touch_target_default(),
            "the overflow mark was smaller than a finger"
        );
    }

    /// Pagination that measures a row at the full width fits one line more per
    /// row than the row will get, which puts the last row under the bottom bar.
    #[test]
    fn pagination_reserves_the_overflow_column() {
        let area = CLARA_BW_METRICS.prose_area(true, true);
        let full = row_title_width(&CLARA_BW_METRICS, area, "", false);
        assert_eq!(
            row_title_width(&CLARA_BW_METRICS, area, "", true),
            full - CLARA_BW_METRICS.touch_target_default(),
            "the measured title width did not give up a finger to the mark"
        );
        // A title that fits one line at the full width and needs two beside a
        // mark, found by measurement rather than picked by eye, so this stays
        // true if the face or the panel changes.
        let words = "feed ".repeat(40);
        let title = (1..=words.len())
            .filter(|end| words.is_char_boundary(*end))
            .map(|end| words[..end].trim_end().to_owned())
            .find(|candidate| {
                wrap_text(candidate, full, FontSize::Body).len() == 1
                    && wrap_text(
                        candidate,
                        full - CLARA_BW_METRICS.touch_target_default(),
                        FontSize::Body,
                    )
                    .len()
                        == 2
            })
            .expect("a title that wraps only once the mark takes its column");
        assert!(
            measured_row_height(
                &CLARA_BW_METRICS,
                area,
                &title,
                "a summary",
                "",
                true,
                row_mark_column(&CLARA_BW_METRICS)
            ) > measured_row_height(
                &CLARA_BW_METRICS,
                area,
                &title,
                "a summary",
                "",
                false,
                row_mark_column(&CLARA_BW_METRICS)
            ),
            "the overflow column cost the title nothing"
        );
        let summary = "a summary that is itself long enough to wrap onto a second line here";
        let rows = vec![(&title[..], summary); 40];
        let with = paginate_rows_with_menu(&rows, &CLARA_BW_METRICS, area);
        let without = paginate_rows(&rows, &CLARA_BW_METRICS, area);
        assert!(
            with.first().map_or(0, Vec::len) < without.first().map_or(0, Vec::len),
            "a list with overflow marks paginated as though it had none"
        );
    }

    #[test]
    fn a_row_without_a_menu_reserves_no_column_for_one() {
        let screen = Screen::new(
            1,
            vec![Node::Rows {
                id: NodeId(1),
                rows: vec![Row::new(ActionId(1), "Counter", "", Glyph::Note)],
            }],
        );
        assert!(
            !screen
                .layout()
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::RowMenu(_))),
            "a row with no overflow action still drew one"
        );
    }

    #[test]
    fn every_fact_value_starts_in_the_same_column() {
        let screen = Screen::new(
            1,
            vec![Node::Facts {
                id: NodeId(1),
                entries: vec![
                    ("Downloads".to_owned(), "94,206".to_owned()),
                    ("A much longer label".to_owned(), "2701".to_owned()),
                    ("ID".to_owned(), "Public domain in the USA".to_owned()),
                ],
            }],
        );
        let layout = screen.layout();
        let values = layout
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::FactValue)
            .map(|node| node.rect.x)
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 3, "a fact was dropped");
        assert!(
            values.iter().all(|x| *x == values[0]),
            "fact values did not share one column: {values:?}"
        );
    }

    #[test]
    fn one_long_label_cannot_squeeze_every_value_into_a_gutter() {
        let screen = Screen::new(
            1,
            vec![Node::Facts {
                id: NodeId(1),
                entries: vec![(
                    "A label so long that it would take the whole panel if it were allowed to"
                        .to_owned(),
                    "94,206".to_owned(),
                )],
            }],
        );
        let layout = screen.layout();
        let value = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::FactValue)
            .expect("laid out");
        assert!(
            value.rect.width * 2 > layout.content.width,
            "the label column took more than half the panel from its value"
        );
    }

    #[test]
    fn facts_past_the_cap_are_reported_rather_than_quietly_clipped() {
        let entries = (0..MAX_FACTS + 4)
            .map(|index| (format!("Label {index}"), format!("{index}")))
            .collect();
        let screen = Screen::new(
            1,
            vec![Node::Facts {
                id: NodeId(1),
                entries,
            }],
        );
        let issues = screen.validate(&CLARA_BW_METRICS);
        assert!(
            issues.iter().any(|issue| matches!(
                issue.kind,
                LayoutIssueKind::CollectionTruncated {
                    collection: "facts",
                    ..
                }
            )),
            "a facts block over the cap reported nothing: {issues:?}"
        );
    }

    #[test]
    fn a_band_puts_its_slots_beside_each_other() {
        let screen = Screen::new(
            1,
            vec![Node::Band {
                id: NodeId(1),
                align: BandAlign::Top,
                slots: vec![
                    BandSlot::fill(vec![Node::Text {
                        id: NodeId(2),
                        text: "Left".to_owned(),
                    }]),
                    BandSlot::fill(vec![Node::Text {
                        id: NodeId(3),
                        text: "Right".to_owned(),
                    }]),
                ],
            }],
        );
        let layout = screen.layout();
        let rect = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
                .rect
        };
        assert_eq!(rect(2).y, rect(3).y, "slots did not share a top edge");
        assert!(
            rect(3).x > rect(2).x + rect(2).width,
            "the second slot overlapped the first instead of sitting beside it"
        );
    }

    #[test]
    fn a_band_too_narrow_for_its_slots_stacks_itself() {
        let narrow = DisplayMetrics {
            width: metrics_width_for_one_column(),
            ..CLARA_BW_METRICS
        };
        let screen = Screen::new(
            1,
            vec![Node::Band {
                id: NodeId(1),
                align: BandAlign::Top,
                slots: vec![
                    BandSlot::fill(vec![Node::Text {
                        id: NodeId(2),
                        text: "Left".to_owned(),
                    }]),
                    BandSlot::fill(vec![Node::Text {
                        id: NodeId(3),
                        text: "Right".to_owned(),
                    }]),
                ],
            }],
        );
        let layout = screen.layout_with(&narrow, &Chrome::default());
        let rect = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
                .rect
        };
        assert_eq!(
            rect(2).x,
            rect(3).x,
            "a band that could not fit its columns did not fall back to stacking"
        );
        assert!(
            rect(3).y > rect(2).y,
            "a stacked band left its slots on top of each other"
        );
    }

    #[test]
    fn a_band_slot_can_be_centred_against_a_taller_neighbour() {
        let band = |align: BandAlign| {
            let screen = Screen::new(
                1,
                vec![Node::Band {
                    id: NodeId(1),
                    align,
                    slots: vec![
                        BandSlot::fill(vec![
                            Node::Heading {
                                id: NodeId(2),
                                text: "Tall".to_owned(),
                            },
                            Node::Heading {
                                id: NodeId(4),
                                text: "Still tall".to_owned(),
                            },
                        ]),
                        BandSlot::fill(vec![Node::Secondary {
                            id: NodeId(3),
                            text: "Short".to_owned(),
                        }]),
                    ],
                }],
            );
            screen
                .layout()
                .nodes
                .iter()
                .find(|node| node.id == NodeId(3))
                .expect("laid out")
                .rect
                .y
        };
        assert!(
            band(BandAlign::Middle) > band(BandAlign::Top),
            "a centred slot was not moved down against its taller neighbour"
        );
        assert!(
            band(BandAlign::Bottom) > band(BandAlign::Middle),
            "a bottom aligned slot was not moved below a centred one"
        );
    }

    #[test]
    fn a_natural_slot_takes_only_what_it_measures() {
        let screen = Screen::new(
            1,
            vec![Node::Band {
                id: NodeId(1),
                align: BandAlign::Top,
                slots: vec![
                    BandSlot::fill(vec![Node::Text {
                        id: NodeId(2),
                        text: "A title that wants the whole line to itself".to_owned(),
                    }]),
                    BandSlot::natural(vec![Node::Secondary {
                        id: NodeId(3),
                        text: "32".to_owned(),
                    }]),
                ],
            }],
        );
        let layout = screen.layout();
        let rect = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
                .rect
        };
        assert!(
            rect(3).width < rect(2).width,
            "a natural slot took as much room as the fill beside it"
        );
        assert!(
            rect(3).width >= measure_text("32", FontSize::Caption).0,
            "a natural slot was clamped below what its content measures"
        );
    }

    /// A panel narrow enough that two columns cannot both stay readable.
    fn metrics_width_for_one_column() -> i32 {
        CLARA_BW_METRICS.tenth_mm(MIN_BAND_SLOT_TENTH_MM) * 2
    }

    #[test]
    fn a_section_is_quieter_than_the_heading_it_sits_under() {
        let screen = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Moby Dick".to_owned(),
                },
                Node::Section {
                    id: NodeId(2),
                    title: "Details".to_owned(),
                    value: None,
                },
            ],
        );
        let layout = screen.layout();
        let node = |id: u32| {
            layout
                .nodes
                .iter()
                .find(|node| node.id == NodeId(id))
                .expect("laid out")
        };
        assert!(
            node(2).rect.height < node(1).rect.height,
            "a section was not set smaller than the screen's own heading"
        );
        assert_eq!(node(2).kind, LayoutKind::Section);
    }

    #[test]
    fn a_section_draws_a_rule_in_the_room_its_title_leaves() {
        let paint_section = |title: &str| {
            let screen = Screen::new(
                1,
                vec![Node::Section {
                    id: NodeId(1),
                    title: title.to_owned(),
                    value: None,
                }],
            );
            let rect = screen
                .layout()
                .nodes
                .iter()
                .find(|node| node.id == NodeId(1))
                .expect("laid out")
                .rect;
            let (surface, stride) = paint(&screen);
            tone_count(&surface, rect, tone::RULE, stride)
        };
        assert!(paint_section("About") > 0, "a section drew no hairline");
        assert!(
            paint_section("About") > paint_section("A rather longer section name"),
            "a longer title did not give up any of its own hairline"
        );
    }

    #[test]
    fn a_sections_value_is_measured_before_its_title_is_clamped() {
        let screen = Screen::new(
            1,
            vec![Node::Section {
                id: NodeId(1),
                title: "A section title long enough to want the whole line".to_owned(),
                value: Some("32".to_owned()),
            }],
        );
        let node = screen
            .layout()
            .nodes
            .iter()
            .find(|node| node.id == NodeId(1))
            .expect("laid out")
            .clone();
        let title = node.text_lines.first().expect("a title");
        let value = node.text_lines.get(1).expect("a value");
        assert_eq!(value, "32", "the value was clamped away");
        let together =
            measure_text(title, FontSize::Caption).0 + measure_text(value, FontSize::Caption).0;
        assert!(
            together <= node.rect.width,
            "a title and its value together ran past the right margin"
        );
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
                emphasis: Emphasis::Normal,
            }],
        );
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        let button = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(_, _, _)))
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
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
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

    /// A screen with one outlined button, rendered, and the button's rect.
    fn rendered_button(emphasis: Emphasis) -> (Surface, Rect) {
        let screen = Screen::new(
            1,
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(1),
                label: "Add".to_owned(),
                state: ControlState::Enabled,
                emphasis,
            }],
        );
        let rect = screen
            .layout_for(&CLARA_BW_METRICS)
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(..)))
            .expect("a button")
            .rect;
        let mut surface = Surface::new(
            usize::try_from(CLARA_BW_METRICS.width).expect("a panel"),
            usize::try_from(CLARA_BW_METRICS.height).expect("a panel"),
        );
        surface.fill_rect(
            Rect {
                x: 0,
                y: 0,
                width: CLARA_BW_METRICS.width,
                height: CLARA_BW_METRICS.height,
            },
            tone::PAPER,
        );
        render(&screen, &mut surface, None);
        (surface, rect)
    }

    /// A dev aid: renders a shelf and a pair of buttons to a PGM so the
    /// weights can be looked at without a reader on the desk. Run with
    /// `cargo test -p kobo-ui control_sheet -- --ignored`.
    #[test]
    #[ignore = "writes a sheet to look at rather than asserting anything"]
    fn control_sheet() {
        let screen = Screen::new(
            1,
            vec![
                Node::Rows {
                    id: NodeId(1),
                    rows: vec![
                        Row::new(ActionId(1), "Scroll.in", "feeds.feedburner.com", Glyph::Rss),
                        Row::new(
                            ActionId(2),
                            "Humor, Satire, and Cartoons",
                            "newyorker.com",
                            Glyph::Rss,
                        ),
                        Row::new(
                            ActionId(3),
                            "Culture: TV, Movies, Music",
                            "newyorker.com",
                            Glyph::Rss,
                        ),
                    ],
                },
                Node::Spacer {
                    id: NodeId(2),
                    space: Space::Medium,
                },
                Node::Button {
                    id: NodeId(3),
                    action: ActionId(4),
                    label: "Add a feed".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
                Node::Button {
                    id: NodeId(4),
                    action: ActionId(5),
                    label: "Save".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
            ],
        );
        let width = CLARA_BW_METRICS.width;
        let height = 700;
        let mut surface = Surface::new(
            usize::try_from(width).unwrap(),
            usize::try_from(height).unwrap(),
        );
        surface.clear(tone::PAPER);
        render(&screen, &mut surface, None);
        let path = std::env::temp_dir().join("cobalt-control-sheet.pgm");
        let mut out = format!("P5\n{width} {height}\n255\n").into_bytes();
        out.extend_from_slice(&surface.pixels);
        std::fs::write(&path, out).unwrap();
        println!("written to {}", path.display());
    }

    #[test]
    fn a_button_has_its_corners_taken_off() {
        // Square corners on a full-width outlined rectangle is what an HTML
        // form looked like in 1996, and it is most of why these read as
        // wireframes rather than as controls.
        for emphasis in [Emphasis::Normal, Emphasis::Primary] {
            let (surface, rect) = rendered_button(emphasis);
            let at = |x: i32, y: i32| {
                surface.pixels[usize::try_from(y * CLARA_BW_METRICS.width + x).expect("inside")]
            };
            assert_eq!(
                at(rect.x, rect.y),
                tone::PAPER,
                "{emphasis:?} kept a square corner"
            );
            assert_ne!(
                at(rect.x + rect.width / 2, rect.y),
                tone::PAPER,
                "{emphasis:?} has no top edge"
            );
        }
    }

    #[test]
    fn a_buttons_outline_is_heavier_than_a_rule() {
        // A rule separates two things and wants to be quiet. An outline says
        // where a finger may go and wants to be found. At the same weight the
        // button read as one more line on the page.
        let (surface, rect) = rendered_button(Emphasis::Normal);
        let column = rect.x + rect.width / 2;
        let ink = (rect.y..rect.y + rect.height)
            .take_while(|y| {
                surface.pixels
                    [usize::try_from(y * CLARA_BW_METRICS.width + column).expect("inside")]
                    != tone::PAPER
            })
            .count();
        let expected = usize::try_from(CLARA_BW_METRICS.button_border()).expect("a border");
        assert_eq!(ink, expected, "the top edge is {ink} pixels");
    }

    #[test]
    fn an_outlined_button_is_not_filled_in() {
        // The border is the difference between the shape and the same shape
        // inset, so it must leave the middle of the control alone.
        let (surface, rect) = rendered_button(Emphasis::Normal);
        let middle = rect.y + rect.height / 2;
        let at = |x: i32| {
            surface.pixels[usize::try_from(middle * CLARA_BW_METRICS.width + x).expect("inside")]
        };
        assert_eq!(at(rect.x + rect.width / 4), tone::PAPER);
        assert_ne!(at(rect.x), tone::PAPER, "the left edge went missing");
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
    fn a_press_mark_stays_inside_the_control_it_marks() {
        // The failure this replaced: a row's hit rectangle is the full width of
        // the panel, so a tap turned an eighth of the page solid black between
        // the rules above and below it.
        let metrics = CLARA_BW_METRICS;
        let mut surface = Surface::new(200, 60);
        let before = surface.pixels.clone();
        let rect = Rect {
            x: 0,
            y: 10,
            width: 200,
            height: 40,
        };
        surface.invert_press(rect, &metrics);

        let marked = |x: usize, y: usize| surface.pixels[y * 200 + x] != before[y * 200 + x];
        assert!(!marked(0, 10), "the mark reached the corner of the row");
        assert!(!marked(100, 10), "the mark reached the top edge of the row");
        assert!(!marked(0, 30), "the mark reached the side of the panel");
        assert!(marked(100, 30), "the mark did not cover what was touched");
    }

    #[test]
    fn a_press_mark_has_its_corners_taken_off() {
        // Asserted by measuring the mark rather than by predicting where its
        // corner lands: the inset and the radius are both clamped against the
        // control's own size, so a test that guessed a coordinate would be
        // testing its own arithmetic.
        let metrics = CLARA_BW_METRICS;
        let mut surface = Surface::new(200, 60);
        let before = surface.pixels.clone();
        surface.invert_press(
            Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 60,
            },
            &metrics,
        );
        let run = |y: usize| {
            (0..200)
                .filter(|x| surface.pixels[y * 200 + x] != before[y * 200 + x])
                .count()
        };
        let rows: Vec<usize> = (0..60).map(run).collect();
        let first = rows.iter().position(|&n| n > 0).expect("a mark");
        assert!(
            rows[first] < rows[30],
            "the mark's first row is {} wide against {} in the middle, so its \
             corners are square",
            rows[first],
            rows[30]
        );
        assert!(rows[first] > 0, "the mark has no top edge at all");
    }

    #[test]
    fn a_press_mark_undoes_itself_exactly() {
        // The mark is removed by drawing it again, which is only true if its
        // shape comes from the rectangle and the metrics and nothing else.
        let metrics = CLARA_BW_METRICS;
        let mut surface = Surface::new(200, 60);
        for (i, pixel) in surface.pixels.iter_mut().enumerate() {
            *pixel = u8::try_from(i % 251).unwrap_or(0);
        }
        let before = surface.pixels.clone();
        let rect = Rect {
            x: 3,
            y: 4,
            width: 150,
            height: 44,
        };
        surface.invert_press(rect, &metrics);
        assert_ne!(surface.pixels, before, "the press was never drawn");
        surface.invert_press(rect, &metrics);
        assert_eq!(surface.pixels, before, "releasing left the row marked");
    }

    #[test]
    fn a_small_control_still_shows_it_was_touched() {
        // A checkbox is smaller than twice the inset. Taken literally the mark
        // would be nothing at all, which is the state this whole mechanism
        // exists to avoid.
        let metrics = CLARA_BW_METRICS;
        let mut surface = Surface::new(40, 40);
        let rect = Rect {
            x: 8,
            y: 8,
            width: 24,
            height: 24,
        };
        let before = surface.pixels.clone();
        surface.invert_press(rect, &metrics);
        let marked = surface
            .pixels
            .iter()
            .zip(&before)
            .filter(|(now, then)| now != then)
            .count();
        assert!(
            marked >= 24 * 24 / 4,
            "a small control's press mark covered only {marked} pixels"
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

    #[test]
    fn a_section_header_never_ends_a_page_without_its_first_row() {
        let area = CLARA_BW_METRICS.prose_area(true, true);
        // Enough rows to overflow, with a section opening partway down, placed
        // by binary search at every position so no single lucky offset is
        // being asserted.
        let plain: Vec<(&str, &str)> = (0..40)
            .map(|_| ("A row with an ordinary length of title", "and a summary"))
            .collect();
        for opener in 1..plain.len() {
            let rows: Vec<(Option<&str>, &str, &str)> = plain
                .iter()
                .enumerate()
                .map(|(index, (title, summary))| {
                    (
                        (index == opener).then_some("Everything else"),
                        *title,
                        *summary,
                    )
                })
                .collect();
            let pages = paginate_rows_in_sections(&rows, &CLARA_BW_METRICS, area);
            let page = pages
                .iter()
                .find(|page| page.contains(&opener))
                .expect("every row lands on a page");
            let heights: i32 = page
                .iter()
                .map(|index| {
                    let (section, title, summary) = rows[*index];
                    let text_width = row_text_width(&CLARA_BW_METRICS, area);
                    let body = wrap_text(title, text_width, FontSize::Body).len() as i32
                        * FontSize::Body.line_height()
                        + wrap_text(summary, text_width, FontSize::Caption).len() as i32
                            * FontSize::Caption.line_height()
                        + CLARA_BW_METRICS.space(Space::Small) * 2;
                    max(CLARA_BW_METRICS.touch_target_default(), body)
                        + if section.is_some() {
                            section_height(&CLARA_BW_METRICS)
                        } else {
                            0
                        }
                })
                .sum();
            let separators = (page.len() as i32 - 1) * (area.gap * 2);
            assert!(
                heights + separators <= area.height || page.len() == 1,
                "a page carrying a section header and its row overflowed"
            );
        }
    }

    #[test]
    fn an_action_bar_marks_nothing_and_a_nav_bar_is_asked_to() {
        let actions = Screen::new(1, Vec::new()).with_nav_bar(NavBar::actions(
            NodeId(9),
            vec![
                BarAction::new(ActionId(1), "Back"),
                BarAction::new(ActionId(2), "Next"),
            ],
        ));
        let layout = actions.layout_for(&CLARA_BW_METRICS);
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| matches!(node.kind, LayoutKind::NavDestinationSelected(..))),
            "none of these is a place the reader could be standing"
        );
        assert!(
            !actions
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::NavBarWithoutSelection),
            "a bar of verbs has nothing to mark and must not be nagged about it"
        );

        let unmarked = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(9),
            vec![
                BarAction::new(ActionId(1), "Stories"),
                BarAction::new(ActionId(2), "Saved"),
            ],
            None,
        ));
        assert!(
            unmarked
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::NavBarWithoutSelection),
            "a bar of places that marks none of them is a bar that lies"
        );
    }

    /// Every composition lint, each with a screen that fires it and one that
    /// does not. The pairs matter more than the cases: a lint that fires on
    /// everything is noise, and noise is turned off.
    #[test]
    fn state_written_into_a_label_is_caught_where_a_state_field_exists() {
        let fires = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: vec![Tile::new(ActionId(1), "Moby-Dick (kept)", Glyph::Book)],
                shape: TileShape::Portrait,
            }],
        );
        assert!(
            fires
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::StateInLabel),
            "this is the exact string the shelf drew for a year"
        );

        let quiet = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: vec![
                    Tile::new(ActionId(1), "Moby-Dick", Glyph::Book).with_state(TileState::Held),
                    Tile::new(ActionId(2), "Ulysses (1922)", Glyph::Book),
                    Tile::new(ActionId(3), "Emma (annotated edition)", Glyph::Book),
                ],
                shape: TileShape::Portrait,
            }],
        );
        assert!(
            !quiet
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::StateInLabel),
            "a year, an edition and a real state field are all legitimate"
        );
    }

    #[test]
    fn a_second_way_back_is_caught() {
        let back = |owns: bool, extra: Vec<Node>| {
            Screen::new(1, extra)
                .with_own_back(owns)
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::AmbiguousBack)
        };
        let button = |label: &str| {
            vec![Node::Button {
                id: NodeId(1),
                action: ActionId(1),
                label: label.to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            }]
        };
        assert!(
            back(true, button("Back to the results")),
            "the runtime already drew Back; this is the second one"
        );
        assert!(
            !back(true, button("Read")),
            "a verb that is not a retreat is not a second way out"
        );
        assert!(
            !back(false, button("Back to the results")),
            "a screen that did not take Back may certainly draw its own"
        );
    }

    #[test]
    fn a_second_primary_action_is_caught() {
        let primary = |emphasis: Emphasis| Node::Button {
            id: NodeId(2),
            action: ActionId(2),
            label: "Delete".to_owned(),
            state: ControlState::Enabled,
            emphasis,
        };
        let first = Node::Button {
            id: NodeId(1),
            action: ActionId(1),
            label: "Read".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Primary,
        };
        let fired = |emphasis: Emphasis| {
            Screen::new(1, vec![first.clone(), primary(emphasis)])
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::MultiplePrimaryActions)
        };
        assert!(fired(Emphasis::Primary), "two primaries is no decision");
        assert!(!fired(Emphasis::Normal), "one primary is the whole point");
    }

    #[test]
    fn a_section_header_with_nothing_under_it_is_caught() {
        let section = |id: u32, title: &str| Node::Section {
            id: NodeId(id),
            title: title.to_owned(),
            value: None,
        };
        let orphan = Screen::new(1, vec![section(1, "Details")]);
        assert!(
            orphan
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::OrphanedSection),
            "a header with its content overleaf is the classic broken page"
        );

        let filled = Screen::new(
            1,
            vec![
                section(1, "Details"),
                Node::Text {
                    id: NodeId(2),
                    text: "Published in 1851.".to_owned(),
                },
            ],
        );
        assert!(
            !filled
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::OrphanedSection),
            "a header that keeps its first line is exactly right"
        );
    }

    #[test]
    fn a_spinner_that_knows_its_own_total_is_caught() {
        let activity = |transferred: Option<(u64, Option<u64>)>| Node::Activity {
            id: NodeId(1),
            label: "Downloading".to_owned(),
            progress: None,
            cancel: None,
            transferred,
            failure: None,
        };
        let fired = |transferred| {
            Screen::new(1, vec![activity(transferred)])
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::IndeterminateWithKnownTotal)
        };
        assert!(
            fired(Some((1024, Some(8192)))),
            "it has counted the work and is still refusing to say"
        );
        assert!(
            !fired(Some((1024, None))),
            "a server that admitted no total leaves nothing to draw a bar from"
        );
        assert!(!fired(None), "a plain wait is a plain wait");
    }

    #[test]
    fn a_screen_that_spends_every_ink_is_caught() {
        let over_budget = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Shelf".to_owned(),
                },
                Node::Secondary {
                    id: NodeId(2),
                    text: "Nine books".to_owned(),
                },
                Node::Divider { id: NodeId(3) },
                Node::Card {
                    id: NodeId(4),
                    children: vec![Node::Text {
                        id: NodeId(5),
                        text: "A card is the surface tone.".to_owned(),
                    }],
                },
                Node::Button {
                    id: NodeId(6),
                    action: ActionId(1),
                    label: "Read".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
            ],
        );
        let issue = over_budget
            .validate(&CLARA_BW_METRICS)
            .into_iter()
            .find(|issue| matches!(issue.kind, LayoutIssueKind::ToneBudget { .. }));
        assert_eq!(
            issue.map(|issue| issue.kind),
            Some(LayoutIssueKind::ToneBudget { used: 5 }),
            "ink, muted, hairline, surface and inverted is all five"
        );

        let within_budget = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Shelf".to_owned(),
                },
                Node::Secondary {
                    id: NodeId(2),
                    text: "Nine books".to_owned(),
                },
                Node::Divider { id: NodeId(3) },
                Node::Button {
                    id: NodeId(4),
                    action: ActionId(1),
                    label: "Read".to_owned(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
            ],
        );
        assert!(
            !within_budget
                .validate(&CLARA_BW_METRICS)
                .iter()
                .any(|issue| matches!(issue.kind, LayoutIssueKind::ToneBudget { .. })),
            "four inks is the budget, not a warning"
        );
    }

    #[test]
    fn an_action_bar_stops_at_three_verbs() {
        let bar = NavBar::actions(
            NodeId(1),
            (1..=5)
                .map(|index| BarAction::new(ActionId(index), format!("Do {index}")))
                .collect(),
        );
        assert_eq!(bar.visible(&CLARA_BW_METRICS).len(), MAX_ACTION_BAR_ACTIONS);
    }

    #[test]
    fn byte_counts_read_the_way_a_person_says_them() {
        assert_eq!(byte_size(0), "0 B");
        assert_eq!(byte_size(512), "512 B");
        assert_eq!(byte_size(1024), "1.0 KB");
        assert_eq!(byte_size(4_404_019), "4.2 MB");
        assert_eq!(byte_size(11_534_336), "11 MB");
    }

    #[test]
    fn a_transfer_without_a_total_reports_bytes_and_draws_no_bar() {
        let screen = Screen::new(
            1,
            vec![Node::Activity {
                id: NodeId(1),
                label: "Downloading".into(),
                progress: None,
                cancel: None,
                transferred: Some((4_404_019, None)),
                failure: None,
            }],
        );
        let layout = screen.layout_for(&CLARA_BW_METRICS);
        assert!(
            !layout
                .nodes
                .iter()
                .any(|node| node.kind == LayoutKind::ActivityProgress),
            "a bar with no denominator invents its own position"
        );
        let bytes = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::ActivityBytes)
            .expect("the count is the whole report when there is no bar");
        assert_eq!(bytes.text_lines, vec!["4.2 MB".to_string()]);
    }

    #[test]
    fn a_transfer_with_a_total_says_how_far_through_it_is() {
        let layout = Screen::new(
            1,
            vec![Node::Activity {
                id: NodeId(1),
                label: "Downloading".into(),
                progress: Some(Percent::new(38)),
                cancel: None,
                transferred: Some((4_404_019, Some(11_534_336))),
                failure: None,
            }],
        )
        .layout_for(&CLARA_BW_METRICS);
        let bytes = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::ActivityBytes)
            .expect("a byte caption");
        assert_eq!(bytes.text_lines, vec!["4.2 MB of 11 MB".to_string()]);
    }

    #[test]
    fn a_failed_transfer_keeps_what_was_around_it() {
        let layout = Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "Moby Dick".into(),
                },
                Node::Activity {
                    id: NodeId(2),
                    label: "Downloading".into(),
                    progress: None,
                    cancel: None,
                    transferred: Some((512, None)),
                    failure: Some(TransferFailure {
                        reason: "The connection was reset".into(),
                        resumable: true,
                    }),
                },
            ],
        )
        .layout_for(&CLARA_BW_METRICS);
        assert!(
            layout
                .nodes
                .iter()
                .any(|node| node.text_lines.iter().any(|line| line == "Moby Dick")),
            "a failure must not take the screen away from the reader"
        );
        let failure = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::ActivityFailure)
            .expect("the reason");
        assert_eq!(
            failure.text_lines,
            vec!["The connection was reset".to_string()]
        );
    }

    #[test]
    fn a_ranked_list_pages_with_the_column_its_digits_really_take() {
        // The layout engine sizes the lead column to what sits in it, so a
        // ranked row's title is wider than a marked row's by the difference
        // between digits and a mark. Paginated against a mark, a headline that
        // is drawn on one line is measured as one line too many, and the page
        // comes back a row short with a row's worth of white under it.
        let metrics = CLARA_BW_METRICS;
        let area = metrics.prose_area(true, true);
        assert!(
            row_rank_column(&metrics, 30) < row_mark_column(&metrics),
            "digits took at least as much room as a mark"
        );

        let marked = row_title_width_beside(
            &metrics,
            area,
            "40 points",
            false,
            row_mark_column(&metrics),
        );
        let ranked = row_title_width_beside(
            &metrics,
            area,
            "40 points",
            false,
            row_rank_column(&metrics, 30),
        );
        assert!(ranked > marked, "a ranked title was no wider");

        // A headline that straddles the two measures: it wraps one line
        // further beside a mark than beside digits. Built out of single
        // letters so the wrap can land between them, and long enough that the
        // row is taller than the finger-sized floor either way, which is what
        // makes the difference show up in the paging rather than be absorbed.
        let title = (1..)
            .map(|words| (0..words).map(|_| "n ").collect::<String>())
            .take(400)
            .find(|title| {
                let ranked_lines = wrap_text(title, ranked, FontSize::Body).len();
                let marked_lines = wrap_text(title, marked, FontSize::Body).len();
                ranked_lines >= 4 && marked_lines > ranked_lines
            })
            .expect("no headline straddled the two measures");

        let rows = (0..40)
            .map(|_| (title.as_str(), "someone, an hour ago", "40 points"))
            .collect::<Vec<_>>();
        let ranked_pages = paginate_ranked_rows_with_trailing(&rows, &metrics, area, 30);
        let marked_pages = paginate_rows_with_trailing(&rows, &metrics, area);
        assert!(
            ranked_pages[0].len() > marked_pages[0].len(),
            "the ranked page held no more: {} either way",
            ranked_pages[0].len()
        );
    }
}
