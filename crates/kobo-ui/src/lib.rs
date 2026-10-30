#![forbid(unsafe_code)]
#![allow(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::match_same_arms,
    clippy::only_used_in_recursion,
    clippy::too_many_lines
)]

//! A small retained UI tree and grayscale rasterizer for the Kobo display.

mod board;
pub mod keyboard;
mod pencil;
pub use board::{BoardCell, BoardClue, BoardMark, BoardSurface};
pub use pencil::{PencilBoard, PencilEdge, PencilMark, PencilMarkKind};

use std::cmp::{max, min};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub const DISPLAY_WIDTH: i32 = 1072;
pub const DISPLAY_HEIGHT: i32 = 1448;
const MAX_LAYOUT_NODES: usize = 512;
const MAX_LAYOUT_DEPTH: usize = 16;
const MAX_TEXT_HITS: usize = 1024;
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
/// Folio's physical composition scale.  Components use [`Space`] for their
/// existing rhythm; these named values are available to new compositions that
/// need to state their physical intent without inventing pixel gaps.
pub const SPACE_2MM_TENTH_MM: i32 = 20;
pub const SPACE_4MM_TENTH_MM: i32 = 40;
pub const SPACE_8MM_TENTH_MM: i32 = 80;
pub const SPACE_12MM_TENTH_MM: i32 = 120;
/// Horizontal paper around a content-width button label.
pub const BUTTON_HORIZONTAL_PADDING_TENTH_MM: i32 = SPACE_4MM_TENTH_MM;

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

/// The narrowest a table column is allowed to be before the table stacks.
///
/// Twelve millimetres holds about four characters of body text at the default
/// size. A column narrower than that wraps every cell to one word a line and
/// turns a three column table into a tall grey block nobody can read across.
pub const MIN_TABLE_COLUMN_TENTH_MM: i32 = 120;

/// How wide each column of a table is drawn, and whether it stacks instead.
///
/// `wants` is how wide each column's widest cell would like to be, `usable`
/// the room left for columns once the gaps between them are taken out, and
/// `minimum` the narrowest a column of prose may be squeezed to.
///
/// Every column is first given the least it can live with, which is what it
/// asked for or `minimum`, whichever is smaller: a column whose widest cell
/// is four characters wide is not broken at five characters of room, it is
/// finished. Whatever room is left over is then shared out in proportion to
/// how much more each column wanted, so a sentence takes the slack and the
/// numbers beside it keep their digits.
///
/// Squeezing every column in proportion instead, as this once did, starved
/// the narrow ones: a table of eight short numbers had each of them squeezed
/// under the width of the number itself, so the whole table was declared
/// unfittable and stacked into a ladder of a hundred figures with nothing to
/// say which column any of them came from. Every one of them fitted.
///
/// The table stacks only when the columns cannot hold even that least width.
///
/// Both the layout that draws a table and the pagination that measures one
/// ask this, because the two must agree on whether a table stacked before
/// either can say how tall a row is.
/// Whether a table's first row names its columns rather than holding data.
///
/// `LaTeXML`, which is what renders a paper on arXiv, writes every cell as a
/// `<td>` and marks none of them as a heading, so a table's own header row
/// arrives looking exactly like its data. The row that names columns is
/// words; the rows underneath are numbers. A table whose top row is already
/// figures is left alone rather than having its first measurement turned
/// into a set of labels for the rest.
#[must_use]
pub fn row_names_the_columns(cells: &[String]) -> bool {
    let named = cells
        .iter()
        .filter(|cell| cell.chars().any(char::is_alphabetic))
        .count();
    let filled = cells.iter().filter(|cell| !cell.trim().is_empty()).count();
    filled > 1 && named * 2 > filled
}

#[cfg(test)]
mod folio_tests;

#[cfg(test)]
mod responsive_profile_tests;

/// A stacked cell written with the heading its column was under.
///
/// A table's meaning is entirely in which value sits under which heading, so
/// a stacked table that kept every figure and dropped the headings kept
/// nothing: a page of a paper's results read as a ladder of eighty bare
/// numbers. Returns `None` for the header row itself, which is read as
/// written, and for a cell whose own column has no name.
#[must_use]
pub fn stacked_cell(labels: &[String], row: usize, column: usize, cell: &str) -> Option<String> {
    if row == 0 {
        return None;
    }
    let label = labels.get(column)?.trim();
    if label.is_empty() || cell.trim().is_empty() {
        return None;
    }
    Some(format!("{label}: {cell}"))
}

#[must_use]
pub fn table_column_widths(wants: &[i32], usable: i32, minimum: i32) -> (Vec<i32>, bool) {
    if wants.is_empty() {
        return (Vec::new(), true);
    }
    let floors: Vec<i32> = wants
        .iter()
        .map(|want| minimum.min((*want).max(1)))
        .collect();
    let total: i32 = wants.iter().copied().fold(0, i32::saturating_add);
    if total <= usable {
        return (wants.to_vec(), false);
    }
    let base: i32 = floors.iter().copied().fold(0, i32::saturating_add);
    if base > usable {
        return (floors, true);
    }
    let spare = usable - base;
    let slack: i32 = wants
        .iter()
        .zip(&floors)
        .map(|(want, floor)| (want - floor).max(0))
        .fold(0, i32::saturating_add);
    if slack <= 0 {
        return (floors, false);
    }
    let widths = wants
        .iter()
        .zip(&floors)
        .map(|(want, floor)| floor + (want - floor).max(0).saturating_mul(spare) / slack)
        .collect();
    (widths, false)
}

/// The most rows one [`Node::Table`] will draw.
///
/// A page holds a few dozen lines and a table row is at least one of them, so
/// a table longer than this cannot be on one page anyway. The reader splits a
/// long table across pages before it reaches here.
pub const MAX_TABLE_ROWS: usize = 64;

/// The most columns one [`Node::Table`] will draw.
pub const MAX_TABLE_COLUMNS: usize = 12;

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
/// The measured font and physical panel width determine the visible columns.
/// This upper bound limits each row; anything past the grid is clipped, never wrapped.
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

fn terminal_row_prefix(row: &str, columns: usize) -> String {
    let mut used = 0_usize;
    let mut clipped = String::new();
    for character in row.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            if !clipped.is_empty() {
                clipped.push(character);
            }
            continue;
        }
        if used.saturating_add(width) > columns {
            break;
        }
        clipped.push(character);
        used = used.saturating_add(width);
    }
    clipped
}

fn terminal_cell_at(row: &str, column: usize) -> (char, usize) {
    let mut used = 0_usize;
    for character in row.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width == 0 {
            continue;
        }
        if column == used {
            return (character, width);
        }
        if column < used.saturating_add(width) {
            return (' ', 1);
        }
        used = used.saturating_add(width);
    }
    (' ', 1)
}

/// Terminal text is set at the smallest size, because a terminal's value is in
/// how much of it can be seen at once and a shell's output is read in glances
/// rather than at length.
const TERMINAL_SIZE: FontSize = FontSize::Terminal;

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

/// An application's logical panel direction.
///
/// Landscape is a software rotation: the physical panel remains portrait and
/// the runtime rotates the completed logical surface clockwise for display.
/// Portrait is the zero-copy compatibility default.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Orientation {
    #[default]
    Portrait = 0,
    Landscape = 1,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum LandscapeTurn {
    #[default]
    Clockwise,
    CounterClockwise,
}

impl Orientation {
    #[must_use]
    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Portrait),
            1 => Some(Self::Landscape),
            _ => None,
        }
    }
}

/// A small, deliberate accessibility scale rather than an arbitrary zoom.
///
/// Applications continue to ask for semantic sizes such as [`FontSize::Body`].
/// The runtime applies this preference to every face, so pagination performed
/// in an application and rendering performed on the device remain identical.
///
/// The steps are close enough together that a reader who finds one size a
/// little too small has somewhere to go: three sizes meant the only move from
/// "slightly too small" was a fifth larger, which is a different book. The
/// wire values of the original three are unchanged, because they are written
/// into every reading position already saved on every device.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum TextScale {
    Smallest = 3,
    Smaller = 4,
    #[default]
    Default = 0,
    Medium = 5,
    Large = 1,
    Larger = 6,
    ExtraLarge = 2,
    Huge = 7,
    Largest = 8,
}

impl TextScale {
    /// Every step, smallest first, which is the order a stepper walks.
    pub const STEPS: [Self; 9] = [
        Self::Smallest,
        Self::Smaller,
        Self::Default,
        Self::Medium,
        Self::Large,
        Self::Larger,
        Self::ExtraLarge,
        Self::Huge,
        Self::Largest,
    ];

    /// Percentage applied to the physical type size.
    #[must_use]
    pub const fn percent(self) -> i32 {
        match self {
            Self::Smallest => 80,
            Self::Smaller => 90,
            Self::Default => 100,
            Self::Medium => 110,
            Self::Large => 120,
            Self::Larger => 130,
            Self::ExtraLarge => 140,
            Self::Huge => 155,
            Self::Largest => 170,
        }
    }

    /// Where this size sits among [`Self::STEPS`].
    #[must_use]
    pub fn step(self) -> usize {
        Self::STEPS
            .iter()
            .position(|scale| *scale == self)
            .unwrap_or(2)
    }

    /// The next size up, or `None` at the top of the range.
    #[must_use]
    pub fn larger(self) -> Option<Self> {
        Self::STEPS.get(self.step().saturating_add(1)).copied()
    }

    /// The next size down, or `None` at the bottom of the range.
    #[must_use]
    pub fn smaller(self) -> Option<Self> {
        self.step()
            .checked_sub(1)
            .and_then(|step| Self::STEPS.get(step).copied())
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
            3 => Some(Self::Smallest),
            4 => Some(Self::Smaller),
            5 => Some(Self::Medium),
            6 => Some(Self::Larger),
            7 => Some(Self::Huge),
            8 => Some(Self::Largest),
            _ => None,
        }
    }

    /// Parses values accepted by the runtime's `KOBO_TEXT_SCALE` setting.
    ///
    /// A bare percentage is matched against the steps rather than rounded to
    /// one, so a setting naming a size that no longer exists is refused and
    /// falls back to the default instead of silently becoming a neighbour.
    #[must_use]
    pub fn from_name(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        match value.as_str() {
            "default" | "standard" => return Some(Self::Default),
            "large" => return Some(Self::Large),
            "extra-large" | "extra_large" | "xl" => return Some(Self::ExtraLarge),
            _ => {}
        }
        let digits = value.strip_suffix('%').unwrap_or(&value);
        let percent = digits.parse::<i32>().ok()?;
        Self::STEPS
            .into_iter()
            .find(|step| step.percent() == percent)
    }
}

/// Default Clara BW metrics used by host-side helpers and the simulator.
pub const CLARA_BW_METRICS: DisplayMetrics = DisplayMetrics {
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
    text_scale: TextScale::Default,
};

/// Returns the default Clara BW metrics with the process-level text scale.
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
    /// The logical viewport for this orientation.
    #[must_use]
    pub const fn oriented(mut self, orientation: Orientation) -> Self {
        if matches!(orientation, Orientation::Landscape) {
            (self.width, self.height) = (self.height, self.width);
        }
        self
    }

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

    /// The width inside the bezel margins.
    #[must_use]
    pub const fn content_width(&self) -> i32 {
        let width = self.width - 2 * self.screen_margin();
        if width < 0 {
            0
        } else {
            width
        }
    }

    /// The widest comfortable measure for interface prose.
    ///
    /// Wide readers should add columns or leave paper rather than turn one
    /// paragraph into a line the eye has to track across the whole panel.
    #[must_use]
    pub const fn readable_width(&self) -> i32 {
        let physical_limit = self.tenth_mm(1_050);
        let content = self.content_width();
        if content < physical_limit {
            content
        } else {
            physical_limit
        }
    }

    /// The widest ordinary control should become while retaining its target.
    #[must_use]
    pub const fn control_width(&self) -> i32 {
        let physical_limit = self.tenth_mm(1_200);
        let content = self.content_width();
        if content < physical_limit {
            content
        } else {
            physical_limit
        }
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

    /// How many tile columns fit inside an actual allocated region.
    ///
    /// Tile grids can sit inside bands and dialogs, where the panel width is
    /// not the width they receive. Using the whole panel there produces tiny
    /// cells and overlapping labels on wide devices.
    #[must_use]
    pub const fn grid_columns_for_width(&self, shape: TileShape, width: i32) -> usize {
        let minimum = shape.minimum_cell_tenth_mm();
        let width = if width < 0 { 0 } else { width };
        let usable_tenth_mm = (width * 254) / self.pixels_per_inch;
        let columns = (usable_tenth_mm / minimum) as usize;
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

mod colour;
mod environment;
pub mod vector;

pub use colour::{
    Accent, Appearance, Paint, Palette, ScreenAccent, Shades, LOW_BATTERY_PERCENT, PANEL_LEVEL,
};

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
    /// Pixels using the default Clara BW metrics.
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
    /// Stable action identity shared by app builders and simulator tooling.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let mut hash = 0x811c_9dc5_u32;
        for byte in name.bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
        Self(hash.max(1))
    }

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
    /// Whether the panel being painted can show colour.
    ///
    /// Known to the runtime from the device profile and to nothing else, so it
    /// lives here with the rest of what an application cannot say about the
    /// screen it is drawn on. Grey by default: a caller that does not know, or
    /// is only measuring, gets the frame the grey design has always produced.
    pub appearance: Appearance,
}

impl Chrome {
    /// Composes shell decoration for the screen that will be displayed.
    #[must_use]
    pub fn for_screen(screen: &Screen, at_home: bool, status: Option<Status>) -> Self {
        Self {
            back: !at_home || screen.owns_back,
            status: if screen.reading { None } else { status },
            appearance: Appearance::Grey,
        }
    }

    #[must_use]
    pub const fn with_back(back: bool) -> Self {
        Self {
            back,
            status: None,
            appearance: Appearance::Grey,
        }
    }

    #[must_use]
    pub fn with_status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    /// Says which kind of panel this chrome will be painted on.
    #[must_use]
    pub const fn with_appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
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
            // The busiest strip, so nothing measured against this comes out
            // narrower than the panel it will be drawn on.
            status: Some(Status::standard(
                "00:00",
                Some(Percent::new(50)),
                false,
                Signal::Strong,
                true,
            )),
            appearance: Appearance::Grey,
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
    /// The radio is switched off or has no interface. Drawn struck through,
    /// because an empty mark reads as a weak signal.
    #[default]
    Off,
    /// The radio is on but cannot load a page: still searching, refused, or
    /// associated with no route yet. Drawn as the outline of the full mark,
    /// so it reads as "Wi-Fi, nothing through it" rather than as "Wi-Fi off",
    /// which would send somebody to the switch that is already on.
    Disconnected,
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
    /// The marks at the trailing edge, outermost first.
    ///
    /// Also the order they give way in: when the band is too narrow for all of
    /// them, the innermost are dropped first, so the first mark in the list is
    /// the last one standing. Put what a reader most needs to see first.
    pub marks: Vec<StatusMark>,
}

impl Status {
    /// The band every screen shows: battery outermost, then the radio, then
    /// Bluetooth while something is connected.
    ///
    /// One constructor, so the daemon, the simulator and every test lay the
    /// marks out in the same order rather than each choosing its own.
    #[must_use]
    pub fn standard(
        clock: impl Into<String>,
        battery: Option<Percent>,
        charging: bool,
        signal: Signal,
        bluetooth: bool,
    ) -> Self {
        let mut marks = vec![
            StatusMark::Battery {
                level: battery,
                charging,
            },
            StatusMark::Signal(signal),
        ];
        if bluetooth {
            marks.push(StatusMark::Bluetooth);
        }
        Self {
            clock: clock.into(),
            marks,
        }
    }
}

/// One mark in the status band.
///
/// The band lays out whatever list of these it is given, so a new mark is a
/// variant here, its width in [`StatusMark::width`], and its drawing in
/// [`draw_status_mark`]. Nothing else about the band changes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StatusMark {
    /// The level as a number beside a battery, and whether it is on a cable.
    ///
    /// `None` when the gauge could not be read, which draws nothing at all
    /// rather than an empty battery: an empty battery and an unreadable one
    /// look the same and mean opposite things. The room is still taken, so
    /// the marks beside it do not shuffle when a reading fails once.
    Battery {
        level: Option<Percent>,
        charging: bool,
    },
    /// The radio, at the state the runtime measured.
    Signal(Signal),
    /// Something is connected over Bluetooth right now, not merely powered.
    /// Drawn as headphones, so that "where is the sound going" is answered
    /// on every screen.
    Bluetooth,
}

impl StatusMark {
    /// How wide the mark is, given the height of a square mark.
    ///
    /// The battery's number is given the width of "100%" whatever it says, so
    /// the radio beside it does not step sideways as the charge falls through
    /// ten percent.
    fn width(self, mark: i32, metrics: &DisplayMetrics) -> i32 {
        match self {
            Self::Battery { .. } => {
                battery_glyph_width(mark)
                    + metrics.space(Space::Tight)
                    + figures_width("100%", FontSize::Caption, Face::Text)
            }
            Self::Signal(_) | Self::Bluetooth => mark,
        }
    }
}

/// Wider than tall: the design box is square, so a battery drawn into a
/// square would come out about three millimetres across and read as a dot.
const fn battery_glyph_width(mark: i32) -> i32 {
    mark * 2
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
pub fn ensure_way_back(screen: Screen, chrome: &Chrome, name: &str) -> Screen {
    ensure_way_back_revealed(screen, chrome, name, TopBarState::Shown)
}

/// Whether the shell is currently showing an auto-hiding top bar.
///
/// Held by the shell, never by the application, and never persisted: a screen
/// is first drawn with its bar hidden, and every way of leaving the screen
/// starts it hidden again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TopBarState {
    /// Out of sight, waiting for a touch on the band it would occupy.
    #[default]
    Hidden,
    /// Drawn, and the reader can leave from it.
    Shown,
}

/// [`ensure_way_back`], for a shell that can hide the bar it guarantees.
///
/// A screen that asked to auto-hide and is not currently shown loses its bar
/// entirely, including one the application drew itself: a half-hidden bar is
/// furniture with no purpose. Every other screen is untouched, so the
/// guarantee that nothing is ever drawn without a way back is unchanged for
/// everything that did not opt in.
#[must_use]
pub fn ensure_way_back_revealed(
    mut screen: Screen,
    chrome: &Chrome,
    name: &str,
    state: TopBarState,
) -> Screen {
    // Never while the shell is drawing its status band. The band sits above
    // the bar and shifts it down, so hiding underneath one would put the way
    // back somewhere other than where the reader is told to reach for it.
    // Full-bleed art is a reading screen, and a reading screen has no band.
    if screen.auto_hide_top_bar && state == TopBarState::Hidden && chrome.status.is_none() {
        screen.top_bar = None;
        return screen;
    }
    if chrome.back && screen.top_bar.is_none() {
        screen = screen.with_top_bar(TopBar::new(NodeId(0), name));
    }
    screen
}

/// What a touch at `y` means for an auto-hiding top bar, if anything.
///
/// The band is exactly where the bar is drawn, so "touch the top bar to get
/// it back" is literally true, and a reader who has seen the bar once knows
/// where to reach. A touch on a bar that is already shown is not handled
/// here: it belongs to the bar, so Back keeps working.
#[must_use]
pub fn top_bar_touch(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: &Chrome,
    state: TopBarState,
    y: i32,
) -> Option<TopBarState> {
    // The same condition the drawing side uses, so the band is only ever
    // claimed on a screen whose bar really is at the top of the panel.
    if !screen.auto_hide_top_bar || chrome.status.is_some() {
        return None;
    }
    let within_band = y >= 0 && y < metrics.top_bar_height();
    match (state, within_band) {
        // Asking for it back.
        (TopBarState::Hidden, true) => Some(TopBarState::Shown),
        // Anywhere else puts it away again, the way a reader dismisses it by
        // carrying on reading.
        (TopBarState::Shown, false) => Some(TopBarState::Hidden),
        _ => None,
    }
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
    /// Solid crossword block, never a letter square.
    CrosswordBlock,
    /// Joined white crossword square with a restrained active-word highlight.
    Crossword,
    #[default]
    Board,
    /// A shaded playable square on a conventional draughts board.
    BoardDark,
    /// A light square on a chess or draughts board, where selection means
    /// the square in hand and is shown as a tinted field. Kept apart from
    /// [`Self::Board`] because a plain grid's selection means whatever its
    /// application says it means, Minesweeper's covered squares among them,
    /// and a field of colour under fifteen covered squares is not a mark.
    BoardLight,
    /// A point on a conventional backgammon board, broad at the panel edge.
    BackgammonTop,
    /// A point on a conventional backgammon board, broad at the panel edge.
    BackgammonBottom,
    Key,
    /// A cell that is nothing but the picture in it.
    ///
    /// For a row of actions everybody already knows by their drawing. A key
    /// needs its field because a keyboard is forty-five targets packed edge to
    /// edge and the eye has to be told where one ends; four icons with a
    /// finger's width of paper between them are already separate, and putting
    /// each on a grey slab turns a quiet row into four boxes.
    Plain,
    /// A recessed hardware pad: rounded square, thick ink bezel, paper face.
    ///
    /// Fifteen of these in three rows of five is a command deck. Empty pads
    /// stay as blank keys so the grid does not collapse into a list.
    Pad,
    /// A place where a pad could be, with nothing assigned to it.
    ///
    /// The same square in a hairline rather than the bezel. A deck of fifteen
    /// places with three assigned drew twelve boxes as heavy as the three
    /// that did something, which is a panel mostly made of controls that do
    /// nothing.
    EmptyPad,
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent thing a screen declares about itself, and folding them into one state would make screens that combine them unrepresentable"
)]
pub struct Screen {
    pub id: u32,
    /// Optional fixed top bar. Structurally outside the node list so a screen
    /// cannot carry two of them, bury one inside a card, or place one halfway
    /// down the page.
    pub top_bar: Option<TopBar>,
    /// Whether the shell may keep the top bar out of sight until the reader
    /// asks for it by touching the band it would occupy.
    ///
    /// A screen whose whole point is the picture on it says so here; the
    /// showing and hiding belong to the shell, never to the application,
    /// because a way out that an application can lose is not a way out. The
    /// shell repaints from the screen it already holds, so the band answers
    /// even when the application that drew it has stopped answering.
    pub auto_hide_top_bar: bool,
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
    /// Protocol-11 screens keep the Atkinson display metrics they measured
    /// against. This is runtime-local compatibility state, never wire data.
    pub legacy_typography: bool,

    /// A publisher font already held by the runtime for this application.
    ///
    /// Only reading prose uses it; chrome and controls remain in the system
    /// face so an EPUB cannot make the way out illegible. Missing handles fall
    /// back to the approved reading face without losing any text.
    pub reading_font: Option<FontHandle>,

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

    /// The screen's colour, when it has one. See [`Accent`] and
    /// [`ScreenAccent`].
    ///
    /// Carried by the screen rather than agreed once per session, so the
    /// runtime can draw a screen it is holding for a background application
    /// without remembering anything else about it, and so a reading page can
    /// leave its accent off while the list that opened it keeps one. On a
    /// greyscale panel it changes nothing at all. A screen made with
    /// [`Screen::new`] is plain; the SDK's builder leaves the choice to the
    /// application unless the screen makes one.
    pub accent: ScreenAccent,
}

/// What paging means on a laid-out screen right now.
///
/// The screen's own [`Screen::page_turns`] says what it declares; the overlay
/// says what is on top of it at this moment. Those two facts make four cases,
/// and collapsing them into an `Option` loses the one that matters:
///
/// | Declares turns | Overlay up | This is | What acts on it |
/// |---|---|---|---|
/// | no | no | `None` | nothing here; a physical press may reach the application raw |
/// | yes | no | `Declared` | a tap on a zone, or a press, sends the declared action |
/// | yes | yes | `SuppressedByOverlay` | the press is dropped: the dialog is what the reader is answering |
/// | no | yes | `SuppressedByOverlay` | likewise, and not the raw press either |
///
/// The last two used to be indistinguishable from the first, which meant a
/// press while a dialog was up fell through to the raw intent and an
/// application that handled it paged the content underneath.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PagingState {
    /// The screen declares no page turns and nothing is covering it.
    #[default]
    None,
    /// The screen declares these, and they are live.
    Declared(PageTurns),
    /// The screen is covered, so paging means nothing until it is not.
    SuppressedByOverlay,
}

impl PagingState {
    /// The live turns, if paging means anything at all here.
    #[must_use]
    pub const fn declared(self) -> Option<PageTurns> {
        match self {
            Self::Declared(turns) => Some(turns),
            Self::None | Self::SuppressedByOverlay => None,
        }
    }
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
            auto_hide_top_bar: false,
            nodes,
            nav_bar: None,
            bottom_action: None,
            page_turns: None,
            owns_back: false,
            reading: false,
            legacy_typography: false,
            reading_font: None,
            text_scale: None,
            hold: None,
            overlay: None,
            accent: ScreenAccent::Plain,
        }
    }

    /// Decides this screen's colour: an accent of its own, or none at all.
    /// See [`Accent`]. Either answer is final, where a screen from the SDK's
    /// builder that has not made one inherits its application's.
    #[must_use]
    pub const fn with_accent(mut self, accent: Option<Accent>) -> Self {
        self.accent = ScreenAccent::decided(accent);
        self
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

    /// Lets the shell keep the top bar out of sight until it is asked for.
    ///
    /// For a screen that is one picture edge to edge, where a bar is a strip
    /// of somebody else's furniture across the top of it. The reader gets the
    /// bar back by touching where it would be. Nothing about leaving moves
    /// into the application: see [`Screen::auto_hide_top_bar`].
    #[must_use]
    pub const fn with_auto_hidden_top_bar(mut self, auto_hide: bool) -> Self {
        self.auto_hide_top_bar = auto_hide;
        self
    }

    /// Uses the pre-Folio Atkinson display cuts for a decoded legacy frame.
    #[must_use]
    pub const fn with_legacy_typography(mut self, legacy: bool) -> Self {
        self.legacy_typography = legacy;
        self
    }

    /// Selects a runtime-held publisher face for book prose.
    #[must_use]
    pub const fn with_reading_font(mut self, font: Option<FontHandle>) -> Self {
        self.reading_font = font;
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

    /// Lays the screen out using the default Clara BW metrics.
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
        let _environment = environment::TextEnvironment::enter(self, metrics);
        with_legacy_typography(self.legacy_typography, || {
            with_reading_font(self.reading_font, || {
                self.layout_with_selected_font(metrics, chrome)
            })
        })
    }

    fn layout_with_selected_font(&self, metrics: &DisplayMetrics, chrome: &Chrome) -> Layout {
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
        // A rail belongs to the collection it precedes, not to the whole
        // screen. Reserve its gutter from every following flow node so it
        // cannot be painted through by an unrelated card or row.
        let rail_reserved = if !self.legacy_typography
            && self
                .nodes
                .iter()
                .any(|node| matches!(node, Node::PageRail { of, .. } if *of > 1))
        {
            metrics.tenth_mm(4).max(1) + gap
        } else {
            0
        };
        let flow_width = metrics.width - 2 * margin - rail_reserved;

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
                    flow_width,
                    content_bottom,
                    metrics,
                    prose,
                    gap,
                );
                // Fills share the room between them, so content set between
                // two of them is centred on the page: a twelve line poem in
                // the middle of the paper rather than hung from the top with
                // half a panel of white beneath it. A single fill still takes
                // all of it, which is the foot-of-the-panel case.
                let later = self.nodes[position + 1..]
                    .iter()
                    .filter(|node| matches!(node, Node::Flex { .. }))
                    .count();
                let free = content_bottom
                    .saturating_sub(trailing)
                    .saturating_sub(cursor);
                let share = free / (i32::try_from(later).unwrap_or(0) + 1);
                cursor = max(cursor, cursor.saturating_add(share));
                layout.flow_height = cursor.saturating_sub(content_top).max(0);
                continue;
            }
            let bottom = flow_node_bottom(
                node,
                &self.nodes[position + 1..],
                margin,
                flow_width,
                content_bottom,
                metrics,
                prose,
                gap,
            );
            cursor = with_flow_height_bound(bottom < content_bottom, || {
                layout_flow_node(
                    node,
                    margin,
                    cursor,
                    flow_width,
                    bottom,
                    0,
                    metrics,
                    prose,
                    &mut layout,
                )
            });
            layout.flow_height = cursor.saturating_sub(content_top).max(0);
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
        layout.page_turns = match self.page_turns {
            Some(turns) => PagingState::Declared(turns),
            None => PagingState::None,
        };
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
        let panel = Rect {
            x: 0,
            y: 0,
            width: metrics.width,
            height: metrics.height,
        };
        let content_bounds = layout.content;
        let pinned_bottom = self.bottom_action.as_ref().map(|action| action.id);
        layout.nodes.retain(|node| {
            if !is_enabled_interactive(node.kind) {
                return true;
            }
            let bounds =
                if interaction_uses_panel_bounds(node.kind) || pinned_bottom == Some(node.id) {
                    panel
                } else {
                    content_bounds
                };
            rect_is_inside(node.rect, bounds)
        });
        // Last, so it is on top of everything -- including the bars, which a
        // popover hanging off a top-bar control has to cover to be readable --
        // and so hit testing, which walks the list backwards, reaches it first.
        if let Some(overlay) = &self.overlay {
            layout_overlay(overlay, metrics, prose, &mut layout);
            // A popover cannot also be a page turn: the zones are whatever is
            // left of the content area, and "left over" must not include the
            // thing drawn over it. Said as suppression rather than absence, so
            // that whatever reads this can tell "covered" from "never asked".
            layout.page_turns = PagingState::SuppressedByOverlay;
            layout.hold = None;
            layout.context_actions.clear();
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
    let widest = if legacy_typography() {
        widest
    } else {
        widest.min(metrics.readable_width())
    };
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
    // Measured before anything is placed, because how much room the header
    // needs depends on how many lines the title turned out to be. Narrowed by
    // whatever the cross took, so a title is never set underneath it.
    let title_width = max(1, width - 2 * padding - close);
    // A dialogue's title is its question, and a question cut in half is worse
    // than a dialogue one line taller: "Download Mrs Dalloway?" was reaching
    // the reader as "Download Mrs" at the larger reader text settings. A
    // popover's title is a label over a short menu, and stays on one line.
    let title_lines: Vec<String> = if overlay.title.is_empty() {
        Vec::new()
    } else {
        wrap_text_in(&overlay.title, title_width, FontSize::Title, prose)
            .into_iter()
            .take(match overlay.kind {
                OverlayKind::Modal => 2,
                OverlayKind::Popover { .. } => 1,
            })
            .collect()
    };
    let mut scratch = Layout::default();
    let mut cursor = padding;
    let title_height =
        FontSize::Title.line_height() * i32::try_from(title_lines.len()).unwrap_or(0);
    // The cross and the title share one band, so a modal with no title still
    // has room for the cross and one with a short title does not overlap it.
    let header = max(title_height, close);
    if header > 0 {
        cursor = cursor.saturating_add(header).saturating_add(gap);
    }
    let overlay_bottom = metrics.height.saturating_sub(2 * margin + padding);
    for (position, node) in overlay.nodes.iter().enumerate() {
        if scratch.nodes.len() >= MAX_LAYOUT_NODES {
            break;
        }
        let bottom = flow_node_bottom(
            node,
            &overlay.nodes[position + 1..],
            padding,
            inner,
            overlay_bottom,
            metrics,
            prose,
            gap,
        );
        cursor = with_flow_height_bound(bottom < overlay_bottom, || {
            layout_flow_node(
                node,
                padding,
                cursor,
                inner,
                bottom,
                0,
                metrics,
                prose,
                &mut scratch,
            )
        });
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
    if !title_lines.is_empty() {
        layout.nodes.push(LayoutNode {
            id: overlay.id,
            rect: Rect {
                x: x + padding,
                y: y + padding + (header - title_height) / 2,
                width: title_width,
                height: title_height,
            },
            kind: LayoutKind::OverlayTitle,
            text_lines: title_lines,
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
    let clock_width = if status.clock.is_empty() {
        0
    } else {
        min(
            metrics.width / 3,
            figures_width(&status.clock, FontSize::Caption, Face::Text),
        )
    };
    if clock_width > 0 {
        layout.nodes.push(LayoutNode {
            id: NodeId(0),
            rect: Rect {
                x: margin,
                y: 0,
                // As wide as the figure, not a third of the panel. The clock
                // is redrawn every minute and the box it claims is the box
                // that gets repainted, so a generous one costs a third of the
                // band in ink each time. It can be sized exactly because the
                // digits go on a fixed advance, so this width is the same at
                // every minute of the day.
                width: clock_width,
                height,
            },
            kind: LayoutKind::StatusClock,
            text_lines: vec![status.clock.clone()],
        });
    }
    // Marks are placed from the trailing edge inwards, in the order given,
    // and stop before they would reach the clock. Dropping the innermost is
    // better than overlapping: a mark drawn over the time is two marks nobody
    // can read.
    let mark = height * 7 / 10;
    let gap = metrics.space(Space::Small);
    let floor = margin + clock_width + gap;
    let mut right = metrics.width - margin;
    for status_mark in &status.marks {
        let width = status_mark.width(mark, metrics);
        if right - width < floor {
            break;
        }
        right -= width;
        layout.nodes.push(LayoutNode {
            id: NodeId(0),
            rect: Rect {
                x: right,
                y: 0,
                width,
                height,
            },
            kind: LayoutKind::StatusMark(*status_mark),
            text_lines: Vec::new(),
        });
        right -= gap;
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
        let wanted = if action.glyph.is_some() {
            // Square. A picture has no natural width to measure, and a target
            // narrower than the bar is tall is one a thumb misses.
            control
        } else {
            let size = if FontSize::Body.line_height() <= control {
                FontSize::Body
            } else {
                FontSize::Caption
            };
            let (text_width, _) = measure_text(&action.label, size);
            max(
                control,
                text_width.saturating_add(metrics.space(Space::Medium)),
            )
        };
        let available = action_right.saturating_sub(margin);
        let action_width = if legacy_typography() {
            wanted
        } else {
            if available <= 0 {
                break;
            }
            wanted.min(available)
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
    let inset = if legacy_typography() {
        16
    } else {
        metrics.tenth_mm(BUTTON_HORIZONTAL_PADDING_TENTH_MM)
    };
    let label_width = if legacy_typography() {
        width - 32
    } else {
        width
            .saturating_sub(inset.saturating_mul(2))
            .min(metrics.readable_width())
    };
    let label = one_line(&bottom.action.label, label_width, FontSize::Body);
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
            kind: LayoutKind::InlineGlyph(glyph, false),
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

/// A run of characters inside a paragraph that goes somewhere.
///
/// A footnote marker, a cross-reference, an address: all of them are set into
/// the line exactly like the words around them, and before this the reader drew
/// them as text and nothing else. A book's own cross-references were decoration.
///
/// Byte offsets into the paragraph's own string, and half-open. Offsets rather
/// than a copy of the words because the same words often appear twice in a
/// paragraph and only one of them is the link, and because the layout has to
/// measure the run against what precedes it on its line to know where it is.
///
/// The whole run gets a tap target, which is what makes this workable on a
/// panel that cannot resolve a superscript: the words of a footnote marker are
/// a few millimetres wide even where the marker itself is not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextLink {
    pub action: ActionId,
    /// Where the run starts, as a byte offset into the paragraph.
    pub start: usize,
    /// Where it ends, exclusive.
    pub end: usize,
}

/// The most links one paragraph may carry.
///
/// An annotated edition links every other sentence, and each link is a node in
/// the layout and a tap target in the hit test. Sixteen is far past any
/// paragraph anybody reads and well short of a paragraph that costs anything.
pub const MAX_TEXT_LINKS: usize = 16;
pub const MAX_RICH_TEXT_SPANS: usize = 256;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct TextPresentation {
    pub strong: bool,
    pub emphasis: bool,
    pub underline: bool,
    pub superscript: bool,
    pub subscript: bool,
    /// A reader annotation behind this exact run, rendered as a light ink
    /// wash that remains distinct from selection focus in grayscale.
    pub highlighted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RichTextSpan {
    pub start: usize,
    pub end: usize,
    pub presentation: TextPresentation,
}

/// A formula set into a sentence, drawn rather than written.
///
/// Mathematics does not survive being written out in a line: a fraction
/// stacks, an index sits above the letter it belongs to, and a square root
/// reaches over what it covers. None of that is expressible as a run of
/// characters, so the run is typeset into a picture elsewhere and this says
/// where it goes.
///
/// `start..end` is not a hole in the text. It is the best linear reading of
/// the formula the writer could manage, and it stays in the string: it is
/// what a search matches, what a selection copies, and what a reader gets
/// when the picture has not arrived or will not decode. The picture is laid
/// over it, not instead of it, which is why nothing outside layout and
/// drawing has to know that this list exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineFormula {
    pub start: usize,
    pub end: usize,
    pub handle: PictureHandle,
    /// The typeset picture's own size, in pixels, as handed over.
    pub source: (u32, u32),
}

/// How many formulas one paragraph may carry.
///
/// A dense page of mathematics runs to a few dozen; beyond that the extras
/// fall back to their written form, which reads poorly but reads.
pub const MAX_INLINE_FORMULAE: usize = 64;

/// How big a formula is drawn on the line it sits on.
///
/// At the size it was handed over, because the application scaled it against
/// the reading em when it decoded it, and that is what makes an `x` in a
/// formula the same size as an `x` in the sentence around it.
///
/// The one thing decided here is the ceiling. A fraction stands taller than a
/// line, and left alone a tall one would push the lines apart and take the
/// paragraph with it. Two lines is as tall as anything set in a sentence may
/// be; beyond that it is scaled down, keeping its shape, which is what a
/// typesetter does with an inline fraction too.
#[must_use]
fn inline_formula_size(formula: &InlineFormula, line_height: i32) -> (i32, i32) {
    let width = i32::try_from(formula.source.0).unwrap_or(i32::MAX);
    let height = i32::try_from(formula.source.1).unwrap_or(i32::MAX);
    if width <= 0 || height <= 0 {
        return (0, 0);
    }
    let ceiling = line_height.saturating_mul(2).max(1);
    if height <= ceiling {
        return (width, height);
    }
    let narrowed = i64::from(width) * i64::from(ceiling) / i64::from(height);
    (i32::try_from(narrowed).unwrap_or(i32::MAX).max(1), ceiling)
}

/// The formula covering an offset, if one does.
fn formula_at(offset: usize, formulae: &[InlineFormula]) -> Option<&InlineFormula> {
    formulae
        .iter()
        .take(MAX_INLINE_FORMULAE)
        .find(|formula| formula.start <= offset && offset < formula.end)
}

/// How wide `text[from..to]` is once its formulas are drawn rather than read.
///
/// The written form of a formula and its typeset picture are rarely the same
/// width, so a paragraph carrying formulas cannot be measured as a string.
/// This walks the range, measuring the prose between formulas as prose and
/// each formula at the width it will actually be drawn.
fn measure_range_in(
    text: &str,
    from: usize,
    to: usize,
    size: FontSize,
    face: Face,
    formulae: &[InlineFormula],
    line_height: i32,
) -> i32 {
    if formulae.is_empty() {
        return measure_text_in(&text[from..to], size, face).0;
    }
    let mut total = 0i32;
    let mut cursor = from;
    while cursor < to {
        if let Some(formula) = formula_at(cursor, formulae) {
            // Only from its own start: a formula cut by a line break is
            // measured on the line that begins it, and the remainder of it
            // is not measured twice.
            if formula.start == cursor {
                total = total.saturating_add(inline_formula_size(formula, line_height).0);
            }
            cursor = formula.end.min(to).max(cursor + 1);
            continue;
        }
        let next = formulae
            .iter()
            .take(MAX_INLINE_FORMULAE)
            .map(|formula| formula.start)
            .filter(|start| *start > cursor)
            .min()
            .unwrap_or(to)
            .min(to);
        if text.is_char_boundary(cursor) && text.is_char_boundary(next) {
            total = total.saturating_add(measure_text_in(&text[cursor..next], size, face).0);
        }
        cursor = next.max(cursor + 1);
    }
    total
}

/// Stable document coordinates attached to publisher-styled reading text.
///
/// The UI layer deliberately treats `context` as opaque. A reading
/// application can use it as a block, resource, or document identifier while
/// the runtime adds the byte offsets resolved from the touched word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextSelection {
    pub context: u64,
    pub offset: u32,
}

/// One word under a finger, expressed in the reading application's logical
/// coordinate system rather than in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextHit {
    pub context: u64,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ParagraphAlignment {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParagraphPresentation {
    pub alignment: ParagraphAlignment,
    pub line_height_percent: u16,
    pub margin_before_em: u16,
    pub margin_after_em: u16,
    pub first_line_indent_em: i16,
}

impl Default for ParagraphPresentation {
    fn default() -> Self {
        Self {
            alignment: ParagraphAlignment::Start,
            line_height_percent: 100,
            margin_before_em: 0,
            margin_after_em: 0,
            first_line_indent_em: 0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Node {
    Heading {
        id: NodeId,
        text: String,
        /// How deep in the document's hierarchy this heading sits, counting
        /// from one.
        ///
        /// A screen's own heading is always the first level and does not set
        /// this. A book carries real hierarchy, and drawing every level of it
        /// as display type gave a page of a paper four titles and no
        /// hierarchy at all: the section heading over a paragraph was set
        /// larger than the paper's own name. Levels below the first are set
        /// at [`FontSize::Title`], which is bold enough to lead the prose
        /// under it without competing with the title above it.
        level: u8,
    },
    Text {
        id: NodeId,
        text: String,
        /// Runs of this paragraph that go somewhere when they are touched.
        ///
        /// Empty for almost every paragraph in the system, which is why this
        /// is a list on the node rather than a node of its own: a link is a
        /// property of a few characters inside prose, and a paragraph that had
        /// to be split into three nodes to carry one would wrap, measure and
        /// paginate as three separate paragraphs.
        links: Vec<TextLink>,
    },
    /// EPUB prose retaining bounded inline emphasis and paragraph styling.
    RichText {
        id: NodeId,
        text: String,
        spans: Vec<RichTextSpan>,
        links: Vec<TextLink>,
        presentation: ParagraphPresentation,
        selection: Option<TextSelection>,
        /// Runs of this paragraph that are typeset pictures rather than words.
        ///
        /// Empty for all prose, which is why this is a list on the node in
        /// the same way links are: a formula is a property of a few
        /// characters inside a sentence, and a paragraph split into three
        /// nodes to carry one would wrap and paginate as three paragraphs.
        formulae: Vec<InlineFormula>,
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
        /// An optional caption-sized destination at the trailing edge.
        link: Option<BarAction>,
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
        /// What the field is for, set on one line above it.
        ///
        /// On the field rather than in a heading beside it, because every
        /// application laid that heading out differently and none of them
        /// could say which field it named once two were stacked.
        label: Option<String>,
        /// How to fill it in, set below it in the muted tone.
        hint: Option<String>,
        /// Why what was typed was refused. Drawn in the hint's place rather
        /// than under it, so a field that fails does not push everything
        /// below it down the panel.
        error: Option<String>,
        kind: FieldKind,
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
    /// A bounded board viewport with explicit marks and aligned clue gutters.
    PencilBoard {
        id: NodeId,
        board: PencilBoard,
    },
    Board {
        id: NodeId,
        surface: BoardSurface,
    },
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
    /// A table, drawn as columns that line up.
    ///
    /// The one arrangement in the reader that cannot be expressed as a
    /// downward flow of full-width blocks. A table's meaning is entirely in
    /// which value sits under which heading, and a table read out as prose --
    /// which is what flattening its cells into a sentence amounts to -- keeps
    /// every word and loses all of it.
    ///
    /// Column widths are shared by every row and worked out from the widest
    /// cell in each column, then squeezed proportionally to fit the panel.
    /// When the columns cannot each keep [`MIN_TABLE_COLUMN_TENTH_MM`], the
    /// table stacks each row as its own short block instead, the way
    /// [`Node::Band`] does: a stacked row is always readable and a four
    /// character column never is.
    Table {
        id: NodeId,
        rows: Vec<TableRow>,
        /// What each column wants to be, measured across the whole table
        /// rather than across the rows sent here.
        ///
        /// A table taller than a page is split, and each part would otherwise
        /// be measured on its own and come out with different columns -- so a
        /// reader turning the page would watch the table move under them.
        /// Empty means "measure the rows given", which is what a caller with
        /// the whole table in hand can leave it as.
        weights: Vec<u16>,
    },
    /// A grid of large tappable tiles, the launcher's primary surface.
    TileGrid {
        id: NodeId,
        tiles: Vec<Tile>,
        shape: TileShape,
    },
    /// A passive indication of position in a paginated surface.
    PageRail {
        id: NodeId,
        page: u16,
        of: u16,
    },
    /// The tap-first question primitive.
    ///
    /// Typing on a device with no keyboard and a refresh measured in tens of
    /// milliseconds is markedly worse than tapping, so the shape of the node
    /// pushes authors toward offering answers rather than demanding prose.
    /// One quantity, its two directions, and where it stands in its range.
    ///
    /// The answer to a setting with an order to it: a type size, a brightness,
    /// a volume. A [`Node::Choice`] can express the same thing by naming every
    /// step, and for three steps that was defensible; past that it becomes a
    /// stack of full-width boxes taller than the page it is covering, and it
    /// still cannot say that "Large" is one notch above "Standard".
    ///
    /// The two controls carry pictures rather than words on purpose. Minus and
    /// plus are read the same in every language and at a glance in the dark,
    /// which is when a reader reaches for the light. What the value *is* stays
    /// written out beside them, because a stepper with no reading is the one
    /// thing worse than a list: it says which way to go and never says where
    /// you are.
    Stepper {
        id: NodeId,
        /// What is being adjusted, and its present value, already written for
        /// reading: "Type size 110%".
        label: String,
        /// The two ends, and whether either has anywhere left to go. A
        /// control at the end of its range is drawn muted rather than removed,
        /// so the row keeps its shape and the other end does not slide under
        /// the finger already reaching for it.
        less: BarAction,
        more: BarAction,
        less_state: ControlState,
        more_state: ControlState,
        /// How far along the range the value sits, drawn as a filled track
        /// under the row. `None` for a quantity with no meaningful extent.
        fill: Option<u8>,
    },
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
        /// The one thing to do about it: "Check again", "Retry", "Sign in
        /// again". Set on a line of its own under the words, against the
        /// trailing edge, so the words keep the banner's whole measure.
        action: Option<BarAction>,
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
        /// Whether to draw a rule around it.
        ///
        /// An illustration is a thing set into the page and wants an edge, so
        /// that a pale sky is not mistaken for the margin. A formula is not a
        /// picture of anything, it is a line of the text that happens to be
        /// drawn rather than written, and a box around it would be as odd as
        /// a box around a sentence.
        framed: bool,
        /// Whether the picture is the page rather than something on it.
        ///
        /// Measured against the panel instead of the text column, so it
        /// reaches the bezel on every side. The margins exist to keep prose
        /// off the edge of the glass; art that is the whole point of the
        /// screen only loses by them, and a plate with a four millimetre
        /// border reads as a photograph of a page rather than the page.
        bleed: bool,
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
    /// A setting that is either on or off, and says so with a switch.
    ///
    /// Settings used to write "On" or "Off" as a row's value, which tells the
    /// reader the state and nothing about the fact that tapping the row
    /// changes it. The whole row is the target, as a phone's settings rows
    /// are, and the tap is only reported: the application decides what on
    /// means and sends the screen back with the new state.
    ///
    /// Exactly one row high whatever it says. The label and the summary are
    /// one line each, so a long setting name is ellipsised rather than
    /// allowed to grow the row, and a list of toggles keeps one pitch.
    Toggle {
        id: NodeId,
        action: ActionId,
        label: String,
        /// What being on does, set under the label in the muted tone. Empty
        /// for a setting whose name already says it.
        summary: String,
        on: bool,
        /// A setting that cannot change right now is drawn muted and answers
        /// nothing, rather than disappearing and moving the rows under it.
        state: ControlState,
    },
    /// How many rows are ticked, and what can be done to all of them at once.
    ///
    /// Multi-select is row state plus this bar rather than a list in a
    /// select mode, so the rows stay ordinary rows and an application that
    /// only wants ticks never has to draw a bar. One bar high, with the
    /// count at the leading edge and up to [`MAX_SELECTION_ACTIONS`] verbs
    /// against the trailing one.
    SelectionBar {
        id: NodeId,
        count: u32,
        /// Entries past [`MAX_SELECTION_ACTIONS`] are dropped and reported.
        actions: Vec<BarAction>,
    },
    /// One answer from a list too long for [`Self::Choice`]: a time zone, a
    /// network, a category.
    ///
    /// Paged rather than scrolled, a page of rows at a time with a pager
    /// under them. How many rows a page holds depends only on the panel and
    /// the text size, never on the words, so an application can ask
    /// [`options_per_page`] which page its answer is on before it draws
    /// anything.
    Options {
        id: NodeId,
        /// The question, on one line above the rows.
        prompt: String,
        /// Entries past [`MAX_OPTIONS`] are dropped and reported.
        options: Vec<OptionRow>,
        /// Which option is already the answer, counted across the whole
        /// list rather than the page.
        selected: Option<u16>,
        /// Which page is showing, counting from zero. Past the last page is
        /// drawn as the last page.
        page: u16,
        /// What the pager's two chevrons send. Each is drawn only where there
        /// is a page on that side.
        previous: ActionId,
        next: ActionId,
    },
    /// Text a phone can read off the panel: an address to sign in at, a link
    /// to install from, an article to finish somewhere else.
    ///
    /// Drawn by the runtime as a QR code, so an application sends the text
    /// and never a picture of modules. Square and centred, as wide as the
    /// column up to [`CODE_SIDE_TENTH_MM`], with the caption set under it.
    Code {
        id: NodeId,
        /// At most [`MAX_CODE_BYTES`] bytes.
        payload: String,
        caption: String,
    },
    /// Where the reader is in a short sequence: "Step 2 of 4".
    ///
    /// One line. The application says which step and how many, and the
    /// runtime writes the words and draws a mark per step, so setup flows
    /// stop spelling their position out in prose of their own.
    Steps {
        id: NodeId,
        /// The step the reader is on, counting from zero.
        current: u8,
        /// How many there are, from one to [`MAX_STEPS`].
        total: u8,
        /// A name for each step, in order. Empty, or one per step.
        labels: Vec<String>,
    },
}

/// One answer in a [`Node::Options`] list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OptionRow {
    pub action: ActionId,
    pub label: String,
    /// A line under the label in the muted tone. When any option in a list
    /// has one, every row of that list leaves room for one, so the pages of
    /// a list all have the same pitch.
    pub summary: String,
}

impl OptionRow {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
            summary: summary.into(),
        }
    }
}

/// The most verbs a [`Node::SelectionBar`] draws. Three is what a bar the
/// width of a six inch panel can carry with the count beside them and every
/// label still a word rather than an ellipsis.
pub const MAX_SELECTION_ACTIONS: usize = 3;

/// The most answers one [`Node::Options`] list may carry. Enough for every
/// time zone a reader could be in, which is the longest list it was made for.
pub const MAX_OPTIONS: usize = 1024;

/// The most rows one page of a [`Node::Options`] list shows, however much room
/// the panel has. Past eight a page is a column to search rather than a
/// handful to choose from.
pub const MAX_OPTIONS_PER_PAGE: usize = 8;

/// The most steps a [`Node::Steps`] indicator counts. A sequence longer than
/// this is a document, not a setup flow.
pub const MAX_STEPS: usize = 8;

/// The most text a [`Node::Code`] may carry, in bytes.
///
/// The capacity of a version 10 QR code at the medium error correction level
/// in byte mode. Version 10 is 57 modules a side, 65 with the quiet zone, so
/// in the fifty millimetre square the runtime draws each module is nine
/// pixels on a 300 ppi panel (0.76 mm) and six on a 212 ppi one (0.72 mm).
/// Modules that size read at arm's length off a reflective panel whose
/// black is a dark grey and whose white is a light one, under a lamp, with a
/// little ghosting left from the last page. A longer payload needs a denser
/// version with modules nearer half a millimetre, which is where phone
/// cameras start to need a steady hand and good light. A sign-in address or
/// an install link fits with room to spare; a link carrying tracking
/// parameters should lose them first.
pub const MAX_CODE_BYTES: usize = 213;

/// The largest side a [`Node::Code`] is drawn at, in tenths of a millimetre.
///
/// Fifty millimetres, which is narrower than a portrait column on every
/// panel. It is the widest square that still fits a landscape screen with a
/// top bar, a bottom bar and a two line caption at the largest text size,
/// and a code is no easier to scan for being larger once its modules are
/// this size.
pub const CODE_SIDE_TENTH_MM: i32 = 500;

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

/// An outline font the runtime is holding on an application's behalf.
///
/// Handles are application-local, exactly like [`PictureHandle`]s.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FontHandle(pub u32);

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
    /// A compact landscape featured tile for a focused dashboard band.
    Card,
}

impl TileShape {
    /// The body's height as a fraction of its width, in eighths.
    #[must_use]
    pub const fn eighths(self) -> i32 {
        match self {
            Self::Square => 8,
            Self::Portrait => 12,
            Self::Card => 6,
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
            Self::Card => 280,
        }
    }
}

/// One tile in a [`Node::TileGrid`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tile {
    pub action: ActionId,
    /// Optional secondary action reached by holding the tile. Applications
    /// must expose the same action through a visible control as well.
    pub menu: Option<ActionId>,
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
    /// A short live reading at the tile's trailing edge. Empty means absent.
    pub value: String,
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
    /// Something is happening to it right now. An ink dot in the trailing
    /// corner. Still tappable, because a tap during a download is how a reader
    /// asks what the download is doing.
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
            menu: None,
            label: label.into(),
            glyph,
            picture: None,
            state: TileState::Normal,
            badge: String::new(),
            subtitle: String::new(),
            value: String::new(),
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

    /// Alias for [`Self::with_subtitle`] for caption-style tile copy.
    #[must_use]
    pub fn with_caption(self, caption: impl Into<String>) -> Self {
        self.with_subtitle(caption)
    }

    #[must_use]
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    #[must_use]
    pub const fn with_menu(mut self, action: ActionId) -> Self {
        self.menu = Some(action);
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
    /// Drawn inverted while this cell is the current board selection.
    pub selected: bool,
    /// Small crossword clue number in the upper-left corner (1–99).
    pub corner: Option<u8>,
}

impl Cell {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>) -> Self {
        Self {
            action,
            label: label.into(),
            glyph: None,
            selected: false,
            corner: None,
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

    #[must_use]
    pub const fn with_selected(mut self, selected: bool) -> Self {
        self.selected = selected;
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

/// The most options a segmented control shows as one row. Past five a row of
/// equal segments is too narrow to read on the six inch panels, and the
/// choice belongs in a list with a tick.
pub const MAX_SEGMENTS: usize = 5;

/// The most tabs one strip will lay out.
///
/// Four, because they share the panel's width equally and a fifth on the Clara
/// leaves each one narrower than the two-word label most of them carry.
pub const MAX_TABS: usize = 4;

/// The most cells one grid will lay out.
///
/// Eighty-one is a complete Sudoku board. The protocol and layout node budgets
/// can carry the full board without clipping.
pub const MAX_CELLS: usize = 81;
/// The most columns a grid may ask for.
pub const MAX_COLUMNS: u8 = 12;

/// One row of a [`Node::Table`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableRow {
    /// Whether this row names the columns rather than filling them. Drawn in
    /// the body face rather than a bolder one, and underlined by a rule,
    /// because on sixteen greys a rule separates more clearly than weight.
    pub header: bool,
    pub cells: Vec<String>,
}

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
    /// A verb the row offers beside opening it: Install, Open, Update.
    ///
    /// Drawn as a small outlined button against the trailing edge and hit
    /// tested ahead of the row, like the overflow mark. The label is one line
    /// and never wider than a third of the row, so a long verb is ellipsised
    /// rather than squeezing the title to nothing.
    pub button: Option<BarAction>,
    /// A count against the trailing edge: updates waiting, messages unread.
    ///
    /// The badge takes the width of "999+" whatever the count, so the title
    /// beside it is wrapped the same way when the count goes from nine to
    /// ten, and a list does not reflow because one more message arrived.
    pub badge: Option<u32>,
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
            button: None,
            badge: None,
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

    /// The same row, ticked or not, with a box in its lead column.
    #[must_use]
    pub fn checked(mut self, checked: bool) -> Self {
        self.state = if checked {
            RowState::Checked
        } else {
            RowState::Unchecked
        };
        self
    }

    /// The same, with a small button against the trailing edge.
    #[must_use]
    pub fn with_button(mut self, action: ActionId, label: impl Into<String>) -> Self {
        self.button = Some(BarAction::new(action, label));
        self
    }

    /// The same, with a count against the trailing edge.
    #[must_use]
    pub const fn with_badge(mut self, count: u32) -> Self {
        self.badge = Some(count);
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
    /// Ticked. A box drawn in the lead column takes the lead's place, so a
    /// row that can be ticked is exactly as tall and as wide as one that
    /// cannot, and turning select mode on moves nothing.
    Checked,
    /// Offered for ticking and not ticked: an empty box.
    Unchecked,
    /// A group some of whose members are ticked: a box with a bar across it.
    Mixed,
}

impl RowState {
    /// Whether this state draws a box rather than the row's lead.
    #[must_use]
    pub const fn is_check(self) -> bool {
        matches!(self, Self::Checked | Self::Unchecked | Self::Mixed)
    }
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

/// What a [`Node::Field`] holds, and so which keys the runtime offers for it.
///
/// Carried on the field rather than chosen by the keyboard, because the field
/// is where the application knows what it is asking for. A secret is drawn as
/// dots so a token is never left on the panel for the next person to read, and
/// the runtime's text entry uses the same value to open on digits or on the
/// characters an address needs rather than on letters.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FieldKind {
    #[default]
    Text,
    /// A password or token. Drawn masked and never echoed back in the clear.
    Secret,
    Number,
    /// A web or mail address, where a space is never wanted and `/`, `.` and
    /// `@` are.
    Address,
}

/// How loudly a [`Node::Banner`] speaks.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BannerLevel {
    #[default]
    Info,
    /// Drawn inverted. On a panel with two usable tones, inversion is the
    /// loudest signal available, and it costs one small refresh.
    Attention,
    /// Something finished as the reader hoped: saved, signed in, sent.
    Success,
    /// Something failed and the reader has to act: a token refused, a
    /// download that stopped.
    Error,
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
    /// The universal favorite mark.
    Heart,
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
    /// Bluetooth audio, drawn as headphones.
    ///
    /// Settings drew Bluetooth with the gear before this existed, which made
    /// the one row about headphones look like a link back to the screen it was
    /// already on. It was then the Bluetooth rune, which is the Bluetooth
    /// SIG's registered figure mark and reserved for qualified member
    /// products, so it is drawn as headphones instead. The variant keeps its
    /// name and wire number, so an application that asks for it is unchanged
    /// and simply draws headphones, which say what the radio is for on this
    /// reader anyway.
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
    /// Take away, and the other half of every stepper on the device.
    ///
    /// A single stroke carries no language at all, which is the whole reason
    /// to draw it: "Dimmer" and "Smaller" are words somebody has to read, and
    /// a reader adjusting the light in the dark is not reading anything.
    Minus,
    ChessWhiteKing,
    ChessWhiteQueen,
    ChessWhiteRook,
    ChessWhiteBishop,
    ChessWhiteKnight,
    ChessWhitePawn,
    ChessBlackKing,
    ChessBlackQueen,
    ChessBlackRook,
    ChessBlackBishop,
    ChessBlackKnight,
    ChessBlackPawn,
    /// A conventional solid playing disc for Reversi, draughts, and Morris.
    BlackDisc,
    /// A conventional outlined playing disc for Reversi, draughts, and Morris.
    WhiteDisc,
    /// A stacked solid draughts piece, immediately recognizable as a king.
    BlackDraughtsKing,
    /// A stacked outlined draughts piece, immediately recognizable as a king.
    WhiteDraughtsKing,
    /// A solid man on a conventional draughts board.
    BlackDraughtsMan,
    /// An outlined man on a conventional draughts board.
    WhiteDraughtsMan,
    /// An unoccupied point on a board: a small solid dot.
    ///
    /// Named for what it is rather than for the first game that wanted it.
    /// An intersection on a Nine Men's Morris board and an empty square on
    /// any other board are the same mark, and a glyph called
    /// `MorrisPoint` is one that nobody else reaches for.
    BoardPoint,
    /// A legal destination: a thin ring, large enough to read at arm's length.
    ///
    /// Every board game on this device has to say where a piece may go, and
    /// they were saying it with whatever dotted circle their typeface
    /// happened to carry. This is the mark the renderer draws.
    LegalPoint,
    /// Remove the character before the typing position.
    Backspace,
    /// Capitalize the next typed letter.
    Shift,
    /// Three points in a line joined by a rule: a mill.
    ///
    /// The one shape Nine Men's Morris is played for, and the only mark that
    /// says which game a shelf card opens. The card wore [`Self::BoardPoint`]
    /// before this existed, and a single dot the width of a full stop is not
    /// an emblem anyone reads at arm's length.
    Mill,
}

impl Glyph {
    /// Every glyph in the set, in wire order.
    ///
    /// Exhaustive by construction and asserted to stay that way, because the
    /// hand-written list this replaces had drifted to nineteen entries while
    /// the set was twenty-one: `Light` and `Close` were authored, shipped, and
    /// covered by none of the tests that walk every glyph. A glyph nobody
    /// rasterises in a test is a blank space beside a label on the panel.
    pub const ALL: [Self; 69] = [
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
        Self::Minus,
        Self::ChessWhiteKing,
        Self::ChessWhiteQueen,
        Self::ChessWhiteRook,
        Self::ChessWhiteBishop,
        Self::ChessWhiteKnight,
        Self::ChessWhitePawn,
        Self::ChessBlackKing,
        Self::ChessBlackQueen,
        Self::ChessBlackRook,
        Self::ChessBlackBishop,
        Self::ChessBlackKnight,
        Self::ChessBlackPawn,
        Self::Heart,
        Self::BlackDisc,
        Self::WhiteDisc,
        Self::BlackDraughtsKing,
        Self::WhiteDraughtsKing,
        Self::BlackDraughtsMan,
        Self::WhiteDraughtsMan,
        Self::BoardPoint,
        Self::LegalPoint,
        Self::Backspace,
        Self::Shift,
        Self::Mill,
    ];
}

impl Node {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        match self {
            Self::Heading { id, .. }
            | Self::Text { id, .. }
            | Self::RichText { id, .. }
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
            | Self::Board { id, .. }
            | Self::PencilBoard { id, .. }
            | Self::Rows { id, .. }
            | Self::Table { id, .. }
            | Self::TileGrid { id, .. }
            | Self::PageRail { id, .. }
            | Self::Choice { id, .. }
            | Self::Stepper { id, .. }
            | Self::Banner { id, .. }
            | Self::Skeleton { id, .. }
            | Self::Picture { id, .. }
            | Self::Activity { id, .. }
            | Self::Terminal { id, .. }
            | Self::Toggle { id, .. }
            | Self::SelectionBar { id, .. }
            | Self::Options { id, .. }
            | Self::Code { id, .. }
            | Self::Steps { id, .. } => *id,
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
    /// One mark at the trailing edge of the band.
    StatusMark(StatusMark),
    /// A heading, carrying the level it sits at so it is drawn at the size it
    /// was measured at.
    Heading(u8),
    Text,
    RichText(TextPresentation),
    /// A run inside a paragraph that goes somewhere.
    ///
    /// Drawn as an underline under words the paragraph node has already set,
    /// and hit-tested ahead of the page turn beneath it because it is pushed
    /// after the paragraph. One of these per line the run covers.
    InlineLink(ActionId),
    /// A line of metadata: smaller than the body, and in the muted tone.
    Secondary,
    /// The name of a group, with a hairline to the right margin and an
    /// optional count against it. The value is carried in `text_lines` as the
    /// title followed by the count, so the drawing pass needs no tree.
    Section,
    /// A section's trailing link. It owns its caption-height tap target.
    SectionLink(ActionId),
    /// An indented paragraph. The values are the clamped depth and what the
    /// paragraph is for, so the renderer can draw the gutter rules and pick a
    /// size without consulting the tree.
    Quote(u8, QuoteRole),
    Button(ActionId, ControlState, Emphasis),
    /// A checker stack on a backgammon point. `from_top` pins its base to the
    /// matching board edge so a five-checker point reads as one familiar pile.
    BackgammonStack(Glyph, u8, bool),
    /// The enclosing field and central bar of a backgammon board.
    BackgammonBoard,
    Card,
    /// The extent of a horizontal group. Draws nothing itself: it exists so a
    /// repaint can dirty the whole group rather than each column.
    Band,
    /// A row's right-aligned value. Muted, and never part of the title.
    RowTrailing,
    /// A row's overflow mark: a finger-wide target against the right edge,
    /// hit-tested ahead of the row it sits in because it is pushed after it.
    RowMenu(ActionId),
    /// A text field's border and its tap target, and whether its value was
    /// refused, which a colour panel shows as a tinted field under the note
    /// that says why.
    Field(ActionId, bool),
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
    /// The rail's track and ink segment. It is deliberately not a control.
    PageRail {
        page: u16,
        of: u16,
    },
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
    Cell(ActionId, CellStyle, bool),
    /// A grid cell's label. Carries whether the cell is a board mark (an X,
    /// an O, a Sudoku digit) rather than a keyboard key: only a board mark
    /// grows to `FontSize::Heading` when it is one or two characters. A
    /// keyboard key is also short and square-ish once a row has enough
    /// columns for its cells to turn taller than they are wide, so the size
    /// this label draws at cannot be read off its own rectangle.
    CellLabel(bool),
    /// Explicit board ink; selection is an outline independent of the mark.
    BoardMark(BoardMark, bool),
    BoardClue,
    /// The chip marking the selected square's row and column clues. Drawn
    /// behind the clue's numbers and sized to them, never to the gutter: the
    /// clue's tap target stays the full strip, but the highlight itself hugs
    /// the text it points at.
    BoardClueChip,
    PencilMark(PencilMarkKind, bool, bool, u8),
    PencilEdge(u8, bool),
    PencilNumber(bool),
    /// The three nested squares and four connectors behind a Morris board.
    MorrisBoard,
    /// The rule around a crossword grid. Its squares draw the lines they share
    /// with the neighbours above and to the left of them; this closes the two
    /// sides nothing else has drawn.
    CrosswordBoard,
    /// One cell of a table, drawn in the body face.
    TableCell,
    /// One cell of a table's heading row, drawn muted so the rule under it
    /// reads as the edge of the heading rather than an arbitrary line.
    TableHeaderCell,
    /// The rule under a table's heading row.
    TableRule,
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
    /// The visible boundary of a Folio card tile. Separate from the tap target
    /// so hit testing continues to use the complete card rectangle.
    TileOutline(ControlState),
    TileLabel,
    /// A disabled card tile's title.
    TileLabelMuted,
    /// A tile's second line. Muted, one line, and only emitted when at least
    /// one tile in the grid asked for one.
    TileSubtitle,
    /// A right-aligned, tabular live reading in a tile.
    TileValue,
    /// The mark for a tile's state, set in a chip in the trailing corner. The
    /// chip is filled with paper first, because the corner it sits in may be a
    /// cover, and a tick drawn straight onto a dark cover is not there.
    TileState(TileState),
    /// A count or a word in the tile's leading corner, in the same chip.
    TileBadge,
    TileGlyph(Glyph),
    /// A disabled card tile's icon.
    TileGlyphMuted(Glyph),
    /// A picture drawn inside another control's rect, carrying no action of
    /// its own: the mark above a grid cell's label, or the one beside a bottom
    /// action's word. Deliberately not a control, so hit testing and press
    /// inversion both belong to the thing underneath it. A glyph that was its
    /// own target would invert a square in the middle of a button.
    InlineGlyph(Glyph, bool),
    /// A picture, already placed. `rect` is where it goes; the renderer scales
    /// it to fit only if the application handed over something larger.
    Picture(PictureHandle),
    /// A picture with a rule drawn around it, which is what an illustration
    /// set into a page wants and what a formula does not.
    FramedPicture(PictureHandle),
    ChoicePrompt,
    /// A stepper's reading: what is being adjusted and where it stands. Set in
    /// the middle of the row, between the two controls, and not itself a
    /// target -- a stepper has two answers and neither of them is "the label".
    StepperValue,
    /// One end of a stepper. The glyph says which way it goes and the
    /// [`ControlState`] says whether there is anywhere left to go: a control at
    /// the end of its range is drawn muted and answers nothing, rather than
    /// disappearing and moving the other one under the reader's finger.
    StepperControl(ActionId, ControlState, Glyph),
    /// The track under a stepper, filled as far as the value has gone.
    StepperTrack(u8),
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
    /// A toggle's tap target: the whole row. Draws nothing itself.
    Toggle(ActionId, ControlState),
    /// A toggle's name, muted when the setting cannot change.
    ToggleLabel(ControlState),
    /// The switch at a toggle's trailing edge: whether it is on, and whether
    /// it can change.
    ToggleSwitch(bool, ControlState),
    /// The box a row in a check state draws in its lead column, in place of
    /// the lead.
    RowCheck(RowState),
    /// A row's own button, hit-tested ahead of the row it sits in.
    RowButton(ActionId),
    /// A row's count, in a chip of fixed width.
    RowBadge,
    /// The name of a field, on the line above it.
    FieldLabel,
    /// The line under a field: the hint, or the error in its place. The bool
    /// says which, because an error is drawn in ink and a hint is not.
    FieldNote(bool),
    /// What is in a secret field, drawn as one dot per character rather than
    /// as the characters. Carries how many dots fit.
    FieldSecret(u16),
    /// A banner's one action, on its own line under the words.
    BannerAction(ActionId),
    /// The selection bar's band and its count, which `text_lines` carries.
    SelectionBar,
    /// One verb on the selection bar.
    SelectionAction(ActionId),
    /// One answer in an option list: the whole row, and the mark that says
    /// whether it is the answer already given.
    OptionRow(ActionId, bool),
    /// An option's name.
    OptionLabel,
    /// A code's square. `text_lines` holds the payload, which is encoded
    /// when it is drawn rather than when it is laid out, so a layout pass
    /// never pays for it.
    Code,
    /// The words under a code.
    CodeCaption,
    /// A step indicator: which step and how many, with the words in
    /// `text_lines`.
    Steps(u8, u8),
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
            Self::Cell(_, CellStyle::CrosswordBlock, _) => None,
            Self::Button(action, ControlState::Enabled, _)
            | Self::BarAction(action)
            | Self::BarGlyph(action, _)
            | Self::NavDestination(action, ..)
            | Self::NavDestinationSelected(action, ..)
            | Self::Tile(action, ControlState::Enabled)
            | Self::Field(action, _)
            | Self::FieldClear(action)
            | Self::Chip(action, _)
            | Self::Tab(action, _)
            | Self::Row(action)
            | Self::RowMenu(action)
            | Self::Cell(action, ..)
            | Self::ChoiceOption(action, _)
            | Self::StepperControl(action, ControlState::Enabled, _)
            | Self::ChoiceFreeform(action)
            | Self::QuoteFold(action, _)
            | Self::PagePrevious(action)
            | Self::InlineLink(action)
            | Self::SectionLink(action)
            | Self::PageNext(action)
            | Self::Toggle(action, ControlState::Enabled)
            | Self::RowButton(action)
            | Self::BannerAction(action)
            | Self::SelectionAction(action)
            | Self::OptionRow(action, _) => Some(action),
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
    /// Height consumed by flow nodes, including their internal spacing but
    /// excluding the final gap before the next node.
    pub flow_height: i32,
    /// What paging means here, including when the answer is "nothing, for
    /// now": see [`PagingState`].
    pub page_turns: PagingState,
    /// Set when the screen asked to hear about a held finger.
    pub hold: Option<ActionId>,
    /// Secondary actions reached by holding their owning row or tile.
    pub context_actions: Vec<(Rect, ActionId)>,
    /// Word rectangles derived during layout. These are kept outside `nodes`
    /// so selectable prose does not spend the bounded semantic-node budget on
    /// every word in a novel.
    pub text_hits: Vec<(Rect, TextHit)>,
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
    InteractiveOffscreen,
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
    InvalidBoard,
    /// A code whose text is empty or longer than [`MAX_CODE_BYTES`], so the
    /// runtime would draw an empty square where the reader was told to point
    /// a phone.
    CodeUnreadable,
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
            LayoutIssueKind::InteractiveOffscreen => {
                write!(
                    formatter,
                    "{node}: interactive control is outside the visible panel"
                )
            }
            LayoutIssueKind::TouchTargetTooSmall { minimum } => write!(
                formatter,
                "{node}: touch target is smaller than the {minimum}px minimum"
            ),
            LayoutIssueKind::TextOverflow => {
                write!(formatter, "{node}: text does not fit its rectangle")
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
            LayoutIssueKind::InvalidBoard => {
                formatter.write_str("Board viewport has invalid dimensions, marks or clues.")
            }
            LayoutIssueKind::CodeUnreadable => write!(
                formatter,
                "{node}: code text is empty or longer than {MAX_CODE_BYTES} bytes"
            ),
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
        let visible_start = self
            .nodes
            .iter()
            .rposition(|node| matches!(node.kind, LayoutKind::Scrim { .. }))
            .map_or(0, |index| index + 1);
        self.nodes[visible_start..]
            .iter()
            .filter(|node| node.rect.contains(x, y))
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::Button(_, ControlState::Enabled, _)
                        | LayoutKind::Back
                        | LayoutKind::OverlayClose
                        | LayoutKind::BarAction(_)
                        | LayoutKind::BarGlyph(..)
                        | LayoutKind::NavDestination(..)
                        | LayoutKind::NavDestinationSelected(..)
                        | LayoutKind::Row(_)
                        | LayoutKind::RowMenu(_)
                        | LayoutKind::Cell(..)
                        | LayoutKind::Tile(_, ControlState::Enabled)
                        | LayoutKind::Field(..)
                        | LayoutKind::FieldClear(_)
                        | LayoutKind::Chip(_, _)
                        | LayoutKind::Tab(_, _)
                        | LayoutKind::ChoiceOption(_, _)
                        | LayoutKind::StepperControl(_, ControlState::Enabled, _)
                        | LayoutKind::ChoiceFreeform(_)
                        | LayoutKind::Toggle(_, ControlState::Enabled)
                        | LayoutKind::RowButton(_)
                        | LayoutKind::BannerAction(_)
                        | LayoutKind::SelectionAction(_)
                        | LayoutKind::OptionRow(..)
                )
            })
            .min_by_key(|node| {
                i64::from(node.rect.width.max(0)) * i64::from(node.rect.height.max(0))
            })
            .map(|node| node.rect)
    }

    /// How much of the band between the bars is already spoken for.
    ///
    /// Uses the layout cursor rather than the bounds of painted rectangles:
    /// sections and other components also consume internal spacing after their
    /// last visible mark. Omitting that spacing gives a following list more
    /// room than it will actually receive.
    #[must_use]
    pub const fn content_used(&self) -> i32 {
        self.flow_height
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
        if let Some(action) = self
            .context_actions
            .iter()
            .rev()
            .find_map(|(rect, action)| rect.contains(x, y).then_some(*action))
        {
            return Some(action);
        }
        let hold = self.hold?;
        if !self.content.contains(x, y) || self.hit_control(x, y).is_some() {
            return None;
        }
        Some(hold)
    }

    /// The logical word under a held finger. Controls always win, matching
    /// ordinary tap hit testing and preventing a held link or toolbar button
    /// from unexpectedly selecting book text beneath it.
    #[must_use]
    pub fn hit_text(&self, x: i32, y: i32) -> Option<TextHit> {
        if !self.content.contains(x, y)
            || self.hit_control(x, y).is_some()
            || self.hit_inert_control(x, y)
        {
            return None;
        }
        self.text_hits
            .iter()
            .rev()
            .find_map(|(rect, hit)| rect.contains(x, y).then_some(*hit))
    }

    /// The page turn a tap on empty content means, if any.
    #[must_use]
    pub fn hit_page_turn(&self, x: i32, y: i32) -> Option<ActionId> {
        let turns = self.page_turns.declared()?;
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
                LayoutKind::Button(_, ControlState::Disabled, _)
                    | LayoutKind::Tile(_, ControlState::Disabled)
                    | LayoutKind::Toggle(_, ControlState::Disabled)
                    | LayoutKind::Scrim { .. }
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
            .find(|node| node.kind.acts_on() == Some(action))
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
        Node::Heading { text, level, .. } => {
            measure_text(text, FontSize::for_heading_level(*level)).0
        }
        Node::Text { text, .. } => measure_text_in(text, FontSize::Body, prose).0,
        Node::Secondary { text, .. } => measure_text(text, FontSize::Caption).0,
        // The same padding the button lays itself out with, not a smaller one
        // that looks similar. A menu measured with two millimetres a side and
        // drawn with four is two words wide and four millimetres short, which
        // at the largest reader text settings is enough to break "Rename"
        // across two lines in the middle of the word.
        Node::Button { label, .. } => measure_text(label, FontSize::Body)
            .0
            .saturating_add(2 * metrics.tenth_mm(BUTTON_HORIZONTAL_PADDING_TENTH_MM)),
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
                if let Some(button) = &row.button {
                    text = text
                        .saturating_add(measure_text(&button.label, FontSize::Body).0)
                        .saturating_add(3 * metrics.space(Space::Small));
                }
                if row.badge.is_some() {
                    text = text
                        .saturating_add(row_badge_width(metrics))
                        .saturating_add(metrics.space(Space::Small));
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
        // A box needs a mark's column even in a ranked list, whose digits
        // would otherwise leave it too narrow to hold one.
        if row.state.is_check() {
            column = max(column, row_mark_column(metrics));
        }
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
///
/// Measured on the fixed digit advance the ranks are drawn on rather than on
/// what the face would set them at, because a column measured for a
/// proportional eleven is nearly twenty pixels too narrow for a tabular one
/// and the rank spills into the title.
#[must_use]
pub fn row_rank_column(metrics: &DisplayMetrics, highest: u16) -> i32 {
    min(
        metrics.touch_target_default(),
        figures_width(&highest.to_string(), FontSize::Caption, Face::Text),
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
    let top = match lead {
        // A cover is the row's content rather than a label on it, so it sits
        // against the whole row the way a picture beside a paragraph does.
        RowLead::Picture(..) => y.saturating_add((height - side) / 2),
        // A mark labels the row, and what it labels is the title. Centred on
        // the row instead it sinks the moment a summary wraps to a second
        // line, until it sits against the summary and reads as a mark on
        // that. A rank has always been set against the title for the same
        // reason; this is the rest of the marks agreeing with it.
        RowLead::Icon(_) | RowLead::Number(_) => {
            text_y.saturating_add((FontSize::Body.line_height() - side) / 2)
        }
    };
    Rect {
        x: x.saturating_add((column - side) / 2),
        y: top,
        width: side,
        height: side,
    }
}

fn rich_run_at(start: usize, line_end: usize, spans: &[RichTextSpan]) -> (usize, TextPresentation) {
    for span in spans.iter().take(MAX_RICH_TEXT_SPANS) {
        if span.start <= start && start < span.end {
            return (span.end.min(line_end).max(start + 1), span.presentation);
        }
        if span.start > start {
            return (
                span.start.min(line_end).max(start + 1),
                TextPresentation::default(),
            );
        }
    }
    (line_end.max(start + 1), TextPresentation::default())
}

fn rich_text_spacing(presentation: ParagraphPresentation, prose: Face) -> (i32, i32, i32, i32) {
    let natural = FontSize::Body.line_height_in(prose).max(1);
    let line_height =
        natural.saturating_mul(i32::from(presentation.line_height_percent.clamp(80, 250))) / 100;
    let before = natural.saturating_mul(i32::from(presentation.margin_before_em)) / 100;
    let after = natural.saturating_mul(i32::from(presentation.margin_after_em)) / 100;
    let indent = measure_text_in("M", FontSize::Body, prose)
        .0
        .saturating_mul(i32::from(presentation.first_line_indent_em))
        / 100;
    (line_height, before, after, indent)
}

#[allow(clippy::too_many_arguments)]
fn layout_flow_node(
    node: &Node,
    x: i32,
    y: i32,
    width: i32,
    bottom: i32,
    depth: usize,
    metrics: &DisplayMetrics,
    prose: Face,
    layout: &mut Layout,
) -> i32 {
    let maximum = if legacy_typography() {
        width
    } else {
        match node {
            Node::Heading { .. }
            | Node::Text { .. }
            | Node::RichText { .. }
            | Node::Secondary { .. }
            | Node::Quote { .. }
            | Node::Facts { .. }
            | Node::PagedList { .. }
            | Node::Banner { .. }
            | Node::Splash { .. }
            | Node::Skeleton { .. }
            | Node::Activity { .. } => width.min(metrics.readable_width()),
            Node::Section { .. }
            | Node::Button { .. }
            | Node::Field { .. }
            | Node::Chips { .. }
            | Node::Tabs { .. }
            | Node::Band { .. }
            | Node::Rows { .. }
            | Node::Progress { .. }
            | Node::Stepper { .. }
            | Node::Toggle { .. }
            | Node::SelectionBar { .. }
            | Node::Options { .. } => width.min(metrics.control_width()),
            Node::Steps { .. } => width.min(metrics.readable_width()),
            Node::Code { .. } => width,
            Node::Card { .. }
            | Node::Divider { .. }
            | Node::Spacer { .. }
            | Node::Flex { .. }
            | Node::Grid { .. }
            | Node::Board { .. }
            | Node::PencilBoard { .. }
            | Node::Table { .. }
            | Node::Picture { .. }
            | Node::TileGrid { .. }
            | Node::PageRail { .. }
            | Node::Choice { .. }
            | Node::Terminal { .. } => width,
        }
    }
    .max(0);
    let x = x.saturating_add(width.saturating_sub(maximum) / 2);
    layout_node(node, x, y, maximum, bottom, depth, metrics, prose, layout)
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
        Node::Heading { id, text, level } => {
            let size = FontSize::for_heading_level(*level);
            let lines = wrap_text(text, width, size);
            let height = max(36, lines.len() as i32 * size.line_height());
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Heading(*level),
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Text { id, text, links } => {
            let line_height = FontSize::Body.line_height_in(prose).max(1);
            let visible_lines = if legacy_typography() && !flow_height_bound() {
                usize::MAX
            } else {
                usize::try_from(max(0, bottom.saturating_sub(y)) / line_height).unwrap_or(0)
            };
            let ranges = wrap_ranges(text, width, FontSize::Body, prose)
                .into_iter()
                .take(visible_lines)
                .collect::<Vec<_>>();
            let lines: Vec<String> = if ranges.is_empty() {
                if text.is_empty() && visible_lines > 0 {
                    vec![String::new()]
                } else {
                    Vec::new()
                }
            } else {
                ranges
                    .iter()
                    .map(|line| text[line.0..line.1].to_owned())
                    .collect()
            };
            let natural_height = max(MIN_TEXT_HEIGHT, lines.len() as i32 * line_height);
            let height = if legacy_typography() && !flow_height_bound() {
                natural_height
            } else {
                min(natural_height, max(0, bottom.saturating_sub(y)))
            };
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
            // After the paragraph, so the hit test -- which reads the nodes
            // backwards, on the principle that what is drawn last is what the
            // finger touched -- finds a link before the page turn underneath
            // it. A run split across a line break becomes two rectangles, both
            // naming the same action, which is what a link that wraps should
            // feel like.
            for link in links.iter().take(MAX_TEXT_LINKS) {
                for (index, &(from, to)) in ranges.iter().enumerate() {
                    let start = max(from, link.start);
                    let end = min(to, link.end);
                    if start >= end || !text.is_char_boundary(start) || !text.is_char_boundary(end)
                    {
                        continue;
                    }
                    let before = measure_text_in(&text[from..start], FontSize::Body, prose).0;
                    let through = measure_text_in(&text[from..end], FontSize::Body, prose).0;
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(before),
                            y: y.saturating_add(index as i32 * line_height),
                            width: through.saturating_sub(before),
                            height: line_height,
                        },
                        kind: LayoutKind::InlineLink(link.action),
                        text_lines: Vec::new(),
                    });
                }
            }
            y.saturating_add(height)
        }
        Node::RichText {
            id,
            text,
            spans,
            links,
            presentation,
            selection,
            formulae,
        } => {
            // Rich text is emitted as one node per styled run, so keep one
            // empty run carrying the paragraph's actual layout bounds. It
            // draws nothing, but lets diagnostics distinguish several runs
            // on one line from wrapped lines discarded at the height bound.
            let rich_text_limit = MAX_LAYOUT_NODES.saturating_sub(1);
            let (line_height, before, after, indent) = rich_text_spacing(*presentation, prose);
            let measure = width.saturating_sub(indent.max(0)).max(1);
            let ranges =
                wrap_ranges_with(text, measure, FontSize::Body, prose, formulae, line_height);
            let visible_lines = if legacy_typography() && !flow_height_bound() {
                usize::MAX
            } else {
                usize::try_from(
                    max(
                        0,
                        bottom
                            .saturating_sub(y)
                            .saturating_sub(before)
                            .saturating_sub(after),
                    ) / line_height.max(1),
                )
                .unwrap_or(0)
            };
            let mut line_y = y.saturating_add(before);
            for (line_index, &(from, to)) in ranges.iter().take(visible_lines).enumerate() {
                if layout.nodes.len() >= rich_text_limit {
                    break;
                }
                let line = &text[from..to];
                let line_width =
                    measure_range_in(text, from, to, FontSize::Body, prose, formulae, line_height);
                let first_indent = if line_index == 0 { indent } else { 0 };
                let available = width.saturating_sub(first_indent.max(0));
                let aligned = match presentation.alignment {
                    ParagraphAlignment::Center => (available - line_width).max(0) / 2,
                    ParagraphAlignment::End => (available - line_width).max(0),
                    ParagraphAlignment::Start | ParagraphAlignment::Justify => 0,
                };
                let line_x = x.saturating_add(first_indent).saturating_add(aligned);
                let mut cursor = from;
                let mut run_x = line_x;
                while cursor < to {
                    // Every slice below is by byte, and a paper is full of
                    // characters that take more than one of them. An offset
                    // that landed inside a "\u{3c0}" would take the whole
                    // application down with it, which on a reader means the
                    // panel going back to the stock software mid-sentence, so
                    // a stray offset walks forward to the next character
                    // instead of being trusted.
                    if !text.is_char_boundary(cursor) {
                        let Some(next) = (cursor..to).find(|at| text.is_char_boundary(*at)) else {
                            break;
                        };
                        cursor = next;
                        continue;
                    }
                    // A formula is drawn, not written, so it leaves the run
                    // machinery entirely: it takes its own width on the line
                    // and the words standing in for it are skipped over.
                    // Only while there is a node left to draw it with. Out of
                    // nodes, the formula falls through to the run machinery
                    // below and is set as the words it was written as, which
                    // is how it reads without a picture anywhere else. Skipping
                    // it here instead would take the mathematics off the page
                    // and leave the sentence with a hole in it.
                    if let Some(formula) = formula_at(cursor, formulae)
                        .filter(|_| layout.nodes.len().saturating_add(2) < MAX_LAYOUT_NODES)
                    {
                        let end = formula.end.min(to).max(cursor + 1);
                        if formula.start == cursor {
                            let (drawn_width, drawn_height) =
                                inline_formula_size(formula, line_height);
                            // Centred on the line rather than sat on its top,
                            // because a formula is read against the middle of
                            // the letters beside it and a tall one has to
                            // stand out of the line evenly at both ends.
                            layout.nodes.push(LayoutNode {
                                id: *id,
                                rect: Rect {
                                    x: run_x,
                                    y: line_y + (line_height - drawn_height) / 2,
                                    width: drawn_width,
                                    height: drawn_height,
                                },
                                kind: LayoutKind::Picture(formula.handle),
                                text_lines: Vec::new(),
                            });
                            run_x = run_x.saturating_add(drawn_width);
                        }
                        cursor = end;
                        continue;
                    }
                    let (mut end, mut styled) = rich_run_at(cursor, to, spans);
                    // Never run a styled run across the start of a formula:
                    // the formula has to be reached at its own offset for the
                    // branch above to see it.
                    if let Some(next) = formulae
                        .iter()
                        .take(MAX_INLINE_FORMULAE)
                        .map(|formula| formula.start)
                        .filter(|start| *start > cursor && *start < end)
                        .min()
                    {
                        end = next;
                    }
                    if end <= cursor || end > to || !text.is_char_boundary(end) {
                        end = text[cursor..to]
                            .char_indices()
                            .nth(1)
                            .map_or(to, |(offset, _)| cursor + offset);
                    }
                    // One node per styled run per line. A block carrying the
                    // permitted number of runs would otherwise spend the whole
                    // page's node budget inside this one paragraph, and every
                    // block after it would be dropped without a word. The rest
                    // of the line goes out as a single plain run instead, which
                    // loses emphasis rather than losing the book.
                    if layout.nodes.len().saturating_add(2) >= MAX_LAYOUT_NODES {
                        end = to;
                        styled = TextPresentation::default();
                    }
                    let run = &text[cursor..end];
                    let run_width = measure_text_in(run, FontSize::Body, prose).0;
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: run_x,
                            y: line_y,
                            width: run_width,
                            height: line_height,
                        },
                        kind: LayoutKind::RichText(styled),
                        text_lines: vec![run.to_owned()],
                    });
                    run_x = run_x.saturating_add(run_width);
                    cursor = end;
                }
                for link in links.iter().take(MAX_TEXT_LINKS) {
                    if layout.nodes.len() >= rich_text_limit {
                        break;
                    }
                    let start = max(from, link.start);
                    let end = min(to, link.end);
                    if start < end && text.is_char_boundary(start) && text.is_char_boundary(end) {
                        // Measured the way the line is set, not the way it was
                        // written: a formula earlier on the line takes the width
                        // of its picture, and measuring the words it stands for
                        // instead puts every target after it in the wrong place.
                        let before = measure_range_in(
                            text,
                            from,
                            start,
                            FontSize::Body,
                            prose,
                            formulae,
                            line_height,
                        );
                        let through = measure_range_in(
                            text,
                            from,
                            end,
                            FontSize::Body,
                            prose,
                            formulae,
                            line_height,
                        );
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect: Rect {
                                x: line_x.saturating_add(before),
                                y: line_y,
                                width: through.saturating_sub(before),
                                height: line_height,
                            },
                            kind: LayoutKind::InlineLink(link.action),
                            text_lines: Vec::new(),
                        });
                    }
                }
                if let Some(selection) = selection {
                    for (relative, word) in line.unicode_word_indices() {
                        if layout.text_hits.len() >= MAX_TEXT_HITS {
                            break;
                        }
                        let start = from.saturating_add(relative);
                        let end = start.saturating_add(word.len());
                        let (Ok(start_offset), Ok(end_offset)) =
                            (u32::try_from(start), u32::try_from(end))
                        else {
                            continue;
                        };
                        let before = measure_range_in(
                            text,
                            from,
                            start,
                            FontSize::Body,
                            prose,
                            formulae,
                            line_height,
                        );
                        let through = measure_range_in(
                            text,
                            from,
                            end,
                            FontSize::Body,
                            prose,
                            formulae,
                            line_height,
                        );
                        layout.text_hits.push((
                            Rect {
                                x: line_x.saturating_add(before),
                                y: line_y,
                                width: through.saturating_sub(before).max(1),
                                height: line_height,
                            },
                            TextHit {
                                context: selection.context,
                                start: selection.offset.saturating_add(start_offset),
                                end: selection.offset.saturating_add(end_offset),
                            },
                        ));
                    }
                }
                line_y = line_y.saturating_add(line_height);
            }
            let end = if legacy_typography() && !flow_height_bound() {
                line_y.saturating_add(after)
            } else {
                min(line_y.saturating_add(after), bottom)
            };
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: end.saturating_sub(y),
                },
                kind: LayoutKind::RichText(TextPresentation::default()),
                text_lines: Vec::new(),
            });
            end
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
        Node::Section {
            id,
            title,
            value,
            link,
        } => {
            let lead = metrics.space(Space::Small);
            let trail = metrics.space(Space::Tight);
            let line = FontSize::Caption.line_height();
            let gap = metrics.space(Space::Tight);
            // The value is measured first and the title clamped against what
            // is left, so a long name gives up its own hairline rather than
            // pushing a total off the right margin.
            let trailing = link
                .as_ref()
                .map(|link| one_line(&link.label, width / 2, FontSize::Caption))
                .or_else(|| {
                    value.as_ref().map(|value| {
                        one_line(
                            value,
                            if legacy_typography() {
                                width
                            } else {
                                width / 2
                            },
                            FontSize::Caption,
                        )
                    })
                });
            let trailing_width = trailing
                .as_ref()
                .map_or(0, |value| measure_text(value, FontSize::Caption).0);
            let reserved = trailing_width
                .saturating_add(if trailing_width > 0 { gap } else { 0 })
                .saturating_add(gap)
                .saturating_add(metrics.tenth_mm(MIN_SECTION_RULE_TENTH_MM));
            let title = one_line(
                title,
                max(0, width.saturating_sub(reserved)),
                FontSize::Caption,
            );
            let mut text_lines = vec![title];
            if link.is_none() {
                if let Some(value) = &trailing {
                    text_lines.push(value.clone());
                }
            }
            let section_width = if link.is_some() {
                width.saturating_sub(trailing_width).saturating_sub(gap)
            } else {
                width
            };
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y: y.saturating_add(lead),
                    width: section_width,
                    height: line,
                },
                kind: LayoutKind::Section,
                text_lines,
            });
            if let Some(link) = link {
                let target_width = trailing_width.max(metrics.touch_target_minimum());
                let target_height = line.max(metrics.touch_target_minimum());
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: x.saturating_add(width).saturating_sub(target_width),
                        y: y.saturating_add(lead),
                        width: target_width,
                        height: target_height,
                    },
                    kind: LayoutKind::SectionLink(link.action),
                    text_lines: vec![trailing.unwrap_or_default()],
                });
            }
            y.saturating_add(lead)
                .saturating_add(if link.is_some() {
                    line.max(metrics.touch_target_minimum())
                } else {
                    line
                })
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
            let finger = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            let legacy = legacy_typography();
            let padding = if legacy {
                16
            } else {
                metrics.tenth_mm(BUTTON_HORIZONTAL_PADDING_TENTH_MM)
            };
            let desired = measure_text(label, FontSize::Body)
                .0
                .saturating_add(padding.saturating_mul(2))
                .max(metrics.touch_target_minimum());
            // A primary control is deliberately dominant. Ordinary controls
            // stay content-width unless their label would leave too little
            // surrounding paper to read as a deliberate target.
            let button_width = if legacy
                || *emphasis == Emphasis::Primary
                || in_band_slot()
                || desired >= width * 4 / 5
            {
                width
            } else {
                desired.min(width)
            };
            let button_x = if button_width == width {
                x
            } else {
                x.saturating_add((width - button_width) / 2)
            };
            let lines = wrap_text(
                label,
                if legacy {
                    button_width - 32
                } else {
                    button_width.saturating_sub(padding.saturating_mul(2))
                },
                FontSize::Body,
            );
            // Tall enough for the words on it. A finger is the floor, not the
            // ceiling: at the larger reader text settings a two word label
            // beside another control wraps, and a button drawn one line tall
            // around two lines of text is a screen the renderer refuses
            // outright, so the whole panel went blank rather than one label
            // being a little taller than its neighbour.
            // Exactly what the words need, and never less than a finger. No
            // padding on top of that: a button that grew by a few pixels on
            // every screen would take the room something else had already
            // been measured into, which on a board game is the board.
            let wrapped =
                i32::try_from(lines.len()).unwrap_or(1).max(1) * FontSize::Body.line_height();
            let height = max(finger, wrapped);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: button_x,
                    y,
                    width: button_width,
                    height,
                },
                kind: LayoutKind::Button(*action, *state, *emphasis),
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Field {
            id,
            action,
            value,
            placeholder,
            clear,
            label,
            hint,
            error,
            kind,
        } => {
            let height = max(
                metrics.touch_target_minimum(),
                metrics.touch_target_default(),
            );
            let inset = metrics.space(Space::Small);
            let caption = FontSize::Caption.line_height();
            let tight = metrics.space(Space::Tight);
            let mut y = y;
            if let Some(label) = label.as_deref().filter(|label| !label.is_empty()) {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y,
                        width,
                        height: caption,
                    },
                    kind: LayoutKind::FieldLabel,
                    text_lines: vec![one_line(label, width, FontSize::Caption)],
                });
                y = y.saturating_add(caption).saturating_add(tight);
            }
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height,
                },
                kind: LayoutKind::Field(*action, error.is_some()),
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
            let value_rect = Rect {
                x: x + inset,
                y: y + (height - line) / 2,
                width: text_width,
                height: line,
            };
            if *kind == FieldKind::Secret && !empty {
                // Dots rather than the characters, and only as many as fit,
                // so the length of a token shows without the token. Drawn as
                // marks rather than set as a bullet character, so the mask
                // does not depend on what the installed face happens to hold.
                let fits = text_width / secret_dot_pitch(metrics).max(1);
                let dots = value
                    .chars()
                    .count()
                    .min(usize::try_from(fits).unwrap_or(0));
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: value_rect,
                    kind: LayoutKind::FieldSecret(u16::try_from(dots).unwrap_or(u16::MAX)),
                    text_lines: Vec::new(),
                });
            } else {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: value_rect,
                    kind: LayoutKind::FieldValue(empty),
                    text_lines: vec![one_line(shown, text_width, FontSize::Body)],
                });
            }
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
            let mut y = y.saturating_add(height);
            let shown_note = error
                .as_deref()
                .filter(|error| !error.is_empty())
                .map(|error| (error, true))
                .or_else(|| {
                    hint.as_deref()
                        .filter(|hint| !hint.is_empty())
                        .map(|hint| (hint, false))
                });
            let lines = field_note_lines(hint.as_deref(), error.as_deref(), width);
            if lines > 0 {
                let (text, is_error) = shown_note.unwrap_or_default();
                y = y.saturating_add(tight);
                let height = i32::try_from(lines).unwrap_or(1) * caption;
                let wrapped = wrap_text(text, width, FontSize::Caption);
                let text_lines = if wrapped.len() <= lines {
                    wrapped
                } else {
                    wrap_text(
                        &clamp_lines(text, width, FontSize::Caption, lines),
                        width,
                        FontSize::Caption,
                    )
                    .into_iter()
                    .take(lines)
                    .collect()
                };
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y,
                        width,
                        height,
                    },
                    kind: LayoutKind::FieldNote(is_error),
                    text_lines,
                });
                y = y.saturating_add(height);
            }
            y
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
            // Inside a band slot a run of chips is one choice among a few, the
            // segmented control of both phone platforms: one row, every
            // segment the same width, the chosen one inverted. Equal widths
            // are what make it read as a single control with a current value
            // rather than as tags that happen to sit together. Only when every
            // label fits its share; otherwise the run wraps as it always has,
            // because a truncated option is not one anybody can choose.
            let shown = chips.len().min(MAX_CHIPS);
            if in_band_slot() && (2..=MAX_SEGMENTS).contains(&shown) {
                let gap = metrics.space(Space::Small);
                let count = shown as i32;
                let each = width.saturating_sub(gap * (count - 1)) / count;
                let fits = each >= metrics.touch_target_minimum()
                    && chips.iter().take(shown).all(|chip| {
                        measure_text(&chip.label, FontSize::Caption).0 + pad * 2 <= each
                    });
                if fits {
                    for (position, chip) in chips.iter().take(shown).enumerate() {
                        if layout.nodes.len() >= MAX_LAYOUT_NODES {
                            break;
                        }
                        let segment_x = x + (each + gap) * position as i32;
                        // The last segment takes the rounding, so the row
                        // always ends exactly at the slot's edge.
                        let segment_width = if position + 1 == shown {
                            x + width - segment_x
                        } else {
                            each
                        };
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect: Rect {
                                x: segment_x,
                                y,
                                width: segment_width,
                                height,
                            },
                            kind: LayoutKind::Chip(chip.action, chip.selected),
                            text_lines: vec![chip.label.clone()],
                        });
                    }
                    layout.nodes[index].rect.height = height;
                    return y.saturating_add(height);
                }
            }
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
            let child_width = width.saturating_sub(2 * padding);
            for (position, child) in children.iter().enumerate() {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                let child_bottom = flow_node_bottom(
                    child,
                    &children[position + 1..],
                    x.saturating_add(padding),
                    child_width,
                    bottom,
                    metrics,
                    prose,
                    inner_gap,
                );
                cursor = with_flow_height_bound(child_bottom < bottom, || {
                    layout_flow_node(
                        child,
                        x.saturating_add(padding),
                        cursor,
                        child_width,
                        child_bottom,
                        depth + 1,
                        metrics,
                        prose,
                        layout,
                    )
                })
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
                    for (position, node) in slot.nodes.iter().enumerate() {
                        if layout.nodes.len() >= MAX_LAYOUT_NODES {
                            break;
                        }
                        let node_bottom = flow_node_bottom(
                            node,
                            &slot.nodes[position + 1..],
                            x,
                            width,
                            bottom,
                            metrics,
                            prose,
                            gap,
                        );
                        cursor = with_flow_height_bound(node_bottom < bottom, || {
                            with_band_slot(|| {
                                layout_flow_node(
                                    node,
                                    x,
                                    cursor,
                                    width,
                                    node_bottom,
                                    depth + 1,
                                    metrics,
                                    prose,
                                    layout,
                                )
                            })
                        });
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
                    for (position, node) in slot.nodes.iter().enumerate() {
                        if layout.nodes.len() >= MAX_LAYOUT_NODES {
                            break;
                        }
                        let node_bottom = flow_node_bottom(
                            node,
                            &slot.nodes[position + 1..],
                            slot_x,
                            widths[slot_index],
                            bottom,
                            metrics,
                            prose,
                            gap,
                        );
                        end = with_flow_height_bound(node_bottom < bottom, || {
                            with_band_slot(|| {
                                layout_flow_node(
                                    node,
                                    slot_x,
                                    end,
                                    widths[slot_index],
                                    node_bottom,
                                    depth + 1,
                                    metrics,
                                    prose,
                                    layout,
                                )
                            })
                        });
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
        Node::Table { id, rows, weights } => {
            let rows = &rows[..min(rows.len(), MAX_TABLE_ROWS)];
            let columns = min(
                rows.iter().map(|row| row.cells.len()).max().unwrap_or(0),
                MAX_TABLE_COLUMNS,
            );
            if columns == 0 {
                return y;
            }
            // A column is only told apart from the next one by the space
            // between them, so the gap has to be wider than a word space or
            // two columns of prose read as one ragged paragraph.
            let gap = metrics.space(Space::Small);
            let size = FontSize::Body;
            let line = size.line_height_in(prose);
            let between = gap.saturating_mul(i32::try_from(columns).unwrap_or(1) - 1);
            let usable = max(0, width.saturating_sub(between));
            let minimum = metrics.tenth_mm(MIN_TABLE_COLUMN_TENTH_MM);

            // One measurement for the whole table. A column is as wide as its
            // widest cell wants to be, and the columns are then squeezed in
            // proportion to those wants until they fit -- so a table of one
            // long sentence and two numbers gives the sentence the room and
            // does not give a two character column a third of the panel.
            let mut wants = vec![0_i32; columns];
            for row in rows {
                for (column, cell) in row.cells.iter().take(columns).enumerate() {
                    wants[column] = max(wants[column], measure_text_in(cell, size, prose).0);
                }
            }
            for (column, want) in wants.iter_mut().enumerate() {
                if let Some(weight) = weights.get(column) {
                    *want = i32::from(*weight);
                }
            }
            let (widths, stacked) = table_column_widths(&wants, usable, minimum);

            // Squeezing has a floor, and past it a table is not a table any
            // more. Rather than draw columns four characters wide, each row
            // is stacked as its own lines, which is always readable.
            if stacked {
                // The first row of a table on this screen is the row that
                // names its columns -- the reader repeats it at the top of
                // every page a long table runs onto -- so each value below it
                // can be written with the heading it sat under.
                let labels = rows
                    .first()
                    .filter(|first| first.header)
                    .map_or(&[][..], |first| first.cells.as_slice());
                let mut cursor = y;
                for (index, row) in rows.iter().enumerate() {
                    // The heading row is not drawn on its own: every value
                    // below carries its heading beside it, so setting the
                    // headings again above them is eight lines saying what
                    // the next eighty already say.
                    if index == 0 && !labels.is_empty() {
                        continue;
                    }
                    for (column, cell) in row.cells.iter().take(columns).enumerate() {
                        if cell.trim().is_empty() || layout.nodes.len() >= MAX_LAYOUT_NODES {
                            continue;
                        }
                        let labelled = stacked_cell(labels, index, column, cell);
                        let cell = labelled.as_deref().unwrap_or(cell);
                        let lines = wrap_text_in(cell, width, size, prose);
                        let height = max(line, lines.len() as i32 * line);
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect: Rect {
                                x,
                                y: cursor,
                                width,
                                height,
                            },
                            kind: if row.header {
                                LayoutKind::TableHeaderCell
                            } else {
                                LayoutKind::TableCell
                            },
                            text_lines: lines,
                        });
                        cursor = cursor.saturating_add(height);
                    }
                    cursor = cursor.saturating_add(gap);
                }
                return cursor;
            }

            let mut cursor = y;
            for row in rows {
                let mut tallest = line;
                let mut column_x = x;
                for (column, width) in widths.iter().enumerate() {
                    let cell = row.cells.get(column).map_or("", String::as_str);
                    if !cell.is_empty() && layout.nodes.len() < MAX_LAYOUT_NODES {
                        let lines = wrap_text_in(cell, *width, size, prose);
                        let height = max(line, lines.len() as i32 * line);
                        tallest = max(tallest, height);
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect: Rect {
                                x: column_x,
                                y: cursor,
                                width: *width,
                                height,
                            },
                            kind: if row.header {
                                LayoutKind::TableHeaderCell
                            } else {
                                LayoutKind::TableCell
                            },
                            text_lines: lines,
                        });
                    }
                    column_x = column_x.saturating_add(*width).saturating_add(gap);
                }
                cursor = cursor.saturating_add(tallest);
                // The rule belongs under the headings, where it says the
                // table has started rather than that a section has ended.
                if row.header && layout.nodes.len() < MAX_LAYOUT_NODES {
                    let thickness = metrics.rule_thickness();
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x,
                            y: cursor.saturating_add(gap / 2),
                            width,
                            height: thickness,
                        },
                        kind: LayoutKind::TableRule,
                        text_lines: Vec::new(),
                    });
                    cursor = cursor.saturating_add(gap).saturating_add(thickness);
                }
                cursor = cursor.saturating_add(gap / 2);
            }
            cursor
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
                // What every node does when it runs out of panel: the facts
                // that cannot fit whole are dropped, never drawn half-cut.
                if cursor.saturating_add(height) > bottom {
                    break;
                }
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
            let line_height = FontSize::Body.line_height().max(1);
            let visible_lines = if legacy_typography() && !flow_height_bound() {
                usize::MAX
            } else {
                usize::try_from(max(0, bottom.saturating_sub(y)) / line_height).unwrap_or(0)
            };
            let lines = items
                .iter()
                .skip(start)
                .take(per_page)
                .flat_map(|item| wrap_text(item, width, FontSize::Body))
                .take(visible_lines)
                .collect::<Vec<_>>();
            let natural_height = max(MIN_TEXT_HEIGHT, lines.len() as i32 * line_height);
            let height = if legacy_typography() && !flow_height_bound() {
                natural_height
            } else {
                min(natural_height, max(0, bottom.saturating_sub(y)))
            };
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
        Node::PencilBoard { id, board } => pencil::layout(
            *id,
            board,
            Rect {
                x,
                y,
                width,
                height: bottom.saturating_sub(y),
            },
            metrics,
            layout,
        ),
        Node::Board { id, surface } => board::layout(
            *id,
            surface,
            Rect {
                x,
                y,
                width,
                height: bottom.saturating_sub(y),
            },
            metrics,
            layout,
        ),
        Node::Grid {
            id,
            columns,
            square,
            cells,
        } => {
            let tight = metrics.space(Space::Tight);
            let requested = i32::from((*columns).clamp(1, MAX_COLUMNS));
            let backgammon_board = *square
                && requested == 12
                && cells.len() == 24
                && cells.iter().all(|cell| cell.label.starts_with("Point "));
            let pad_deck = *square && requested == 5 && cells.len() <= 15;
            let chess_board = *square
                && requested == 8
                && cells.len() == 64
                && cells.iter().any(|cell| {
                    matches!(
                        cell.glyph,
                        Some(
                            Glyph::ChessWhiteKing
                                | Glyph::ChessWhiteQueen
                                | Glyph::ChessWhiteRook
                                | Glyph::ChessWhiteBishop
                                | Glyph::ChessWhiteKnight
                                | Glyph::ChessWhitePawn
                                | Glyph::ChessBlackKing
                                | Glyph::ChessBlackQueen
                                | Glyph::ChessBlackRook
                                | Glyph::ChessBlackBishop
                                | Glyph::ChessBlackKnight
                                | Glyph::ChessBlackPawn
                        )
                    )
                });
            // A board's column count is the board, so narrowing it to the touch
            // target would deal a different game. Only free-form grids shrink.
            let columns = if legacy_typography() || *square {
                requested
            } else {
                let fits = width
                    .saturating_add(tight)
                    .checked_div(metrics.touch_target_minimum().saturating_add(tight).max(1))
                    .unwrap_or(1)
                    .max(1);
                requested.min(fits)
            };
            let (mut x, mut width, gutter) = if backgammon_board {
                (0, metrics.width, 0)
            } else if chess_board || (*square && cells.iter().any(|cell| cell.corner.is_some())) {
                (x, width, 0)
            } else if pad_deck {
                (x, width, metrics.space(Space::Small))
            } else {
                (x, width, tight)
            };
            let crossword_squares = *square && cells.iter().any(|cell| cell.corner.is_some());
            let block_extra =
                if *square && columns == 9 && cells.len() >= 27 && cells.len() % 27 == 0 {
                    gutter
                } else {
                    0
                };
            let mut cell_width = (width - gutter * (columns - 1) - block_extra * 2) / columns;
            if *square && !legacy_typography() && !backgammon_board && !cells.is_empty() {
                let rows = i32::try_from(
                    cells
                        .len()
                        .min(MAX_CELLS)
                        .div_ceil(usize::try_from(columns).unwrap_or(1)),
                )
                .unwrap_or(i32::MAX);
                let vertical_gaps = gutter * (rows - 1) + ((rows - 1) / 3) * block_extra;
                let fits_height = bottom.saturating_sub(y).saturating_sub(vertical_gaps) / rows;
                cell_width = cell_width.min(fits_height.max(metrics.touch_target_minimum()));
                // A printed crossword square is about a centimetre across
                // whether the puzzle is a five square mini or a fifteen square
                // daily. Given the whole panel a small grid stretched every
                // square to two centimetres and read as a toy; the cap is what
                // makes a mini look like a mini. A large grid is still bound by
                // the panel, so nothing here shrinks below what fits.
                if crossword_squares {
                    cell_width =
                        cell_width.min(metrics.tenth_mm(110).max(metrics.touch_target_minimum()));
                }
                let board_width = cell_width * columns + gutter * (columns - 1) + block_extra * 2;
                x = x.saturating_add((width - board_width).max(0) / 2);
                width = board_width;
            }
            // A square cell is what makes a board read as a board. A grid that
            // is not square is a keyboard, and there one row of touch target
            // is exactly right and anything taller wastes the panel.
            // A row whose every cell carries a picture is a row of actions,
            // not a keyboard, and it is drawn as the pictures alone.
            let key_height =
                if !legacy_typography() && metrics.width > metrics.height && columns >= 7 {
                    metrics.touch_target_minimum()
                } else {
                    metrics.touch_target_default()
                };
            let (cell_height, style) = if backgammon_board {
                (cell_width.saturating_mul(3), CellStyle::BackgammonTop)
            } else if pad_deck {
                (cell_width, CellStyle::Pad)
            } else if *square {
                (cell_width, CellStyle::Board)
            } else if cells.iter().all(|cell| cell.glyph.is_some()) {
                (metrics.touch_target_default(), CellStyle::Plain)
            } else {
                (key_height, CellStyle::Key)
            };
            let morris_board = *square
                && columns == 7
                && cells.len() == 49
                && cells
                    .iter()
                    .any(|cell| matches!(cell.glyph, Some(Glyph::BoardPoint | Glyph::LegalPoint)));
            let crossword_board = crossword_squares;
            let index = layout.nodes.len();
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: 0,
                },
                kind: if backgammon_board {
                    LayoutKind::BackgammonBoard
                } else if morris_board {
                    LayoutKind::MorrisBoard
                } else if crossword_board {
                    LayoutKind::CrosswordBoard
                } else {
                    LayoutKind::Spacer
                },
                text_lines: Vec::new(),
            });
            let mut rows = 0;
            let draughts_board = *square
                && matches!(columns, 8 | 10)
                && cells.iter().any(|cell| {
                    matches!(
                        cell.glyph,
                        Some(
                            Glyph::BlackDraughtsMan
                                | Glyph::WhiteDraughtsMan
                                | Glyph::BlackDraughtsKing
                                | Glyph::WhiteDraughtsKing
                        )
                    )
                });
            for (position, cell) in cells.iter().take(MAX_CELLS).enumerate() {
                if layout.nodes.len() + 3 > MAX_LAYOUT_NODES {
                    break;
                }
                let position = i32::try_from(position).unwrap_or(0);
                let column = position % columns;
                let row = position / columns;
                let style = if backgammon_board {
                    if row == 0 {
                        CellStyle::BackgammonTop
                    } else {
                        CellStyle::BackgammonBottom
                    }
                } else if crossword_board && cell.label == "#" {
                    CellStyle::CrosswordBlock
                } else if crossword_board {
                    CellStyle::Crossword
                } else if morris_board {
                    CellStyle::Plain
                } else if (draughts_board || chess_board) && (row + column) % 2 == 1 {
                    CellStyle::BoardDark
                } else if draughts_board || chess_board {
                    CellStyle::BoardLight
                } else {
                    style
                };
                rows = row + 1;
                let rect = Rect {
                    x: x.saturating_add(column * (cell_width + gutter) + column / 3 * block_extra),
                    y: y.saturating_add(row * (cell_height + gutter) + row / 3 * block_extra),
                    width: cell_width,
                    height: cell_height,
                };
                // A pad with neither a word nor a picture on it is a place
                // rather than a key, and is drawn as one.
                let style = if style == CellStyle::Pad
                    && cell.glyph.is_none()
                    && cell.label.trim().is_empty()
                {
                    CellStyle::EmptyPad
                } else {
                    style
                };
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect,
                    kind: LayoutKind::Cell(cell.action, style, cell.selected),
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
                if backgammon_board {
                    if let Some(glyph) = cell.glyph {
                        let count = cell
                            .label
                            .split_whitespace()
                            .nth(3)
                            .and_then(|count| count.parse::<u8>().ok())
                            .unwrap_or(1)
                            .clamp(1, 5);
                        layout.nodes.push(LayoutNode {
                            id: *id,
                            rect,
                            kind: LayoutKind::BackgammonStack(
                                glyph,
                                count,
                                matches!(style, CellStyle::BackgammonTop),
                            ),
                            text_lines: vec![cell.label.clone()],
                        });
                    }
                } else {
                    match cell.glyph {
                        Some(glyph) if style == CellStyle::Pad => {
                            let inset = metrics.tenth_mm(PAD_BORDER_TENTH_MM);
                            let mark = min(cell_height, cell_width) * 2 / 5;
                            layout.nodes.push(LayoutNode {
                                id: *id,
                                rect: Rect {
                                    x: rect.x.saturating_add((cell_width - mark).max(0) / 2),
                                    y: rect.y.saturating_add(inset + cell_height / 8),
                                    width: mark,
                                    height: mark,
                                },
                                // Inverted only where the cell is actually
                                // drawn on ink. Every other selected cell is
                                // filled with paper or the surface tone, and a
                                // paper glyph on either of those is a mark
                                // nobody can see: a won line of noughts came
                                // out as three empty squares.
                                kind: LayoutKind::InlineGlyph(
                                    glyph,
                                    cell.selected && style == CellStyle::CrosswordBlock,
                                ),
                                text_lines: vec![cell.label.clone()],
                            });
                            if !cell.label.is_empty() {
                                let label_top = rect.y + cell_height * 3 / 5;
                                layout.nodes.push(LayoutNode {
                                    id: *id,
                                    rect: Rect {
                                        x: rect.x + inset,
                                        y: label_top,
                                        width: (cell_width - inset * 2).max(1),
                                        height: (rect.y + cell_height - inset - label_top).max(1),
                                    },
                                    kind: LayoutKind::CellLabel(false),
                                    text_lines: vec![cell.label.clone()],
                                });
                            }
                        }
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
                                // Inverted only where the cell is actually
                                // drawn on ink. Every other selected cell is
                                // filled with paper or the surface tone, and a
                                // paper glyph on either of those is a mark
                                // nobody can see: a won line of noughts came
                                // out as three empty squares.
                                kind: LayoutKind::InlineGlyph(
                                    glyph,
                                    cell.selected && style == CellStyle::CrosswordBlock,
                                ),
                                text_lines: vec![cell.label.clone()],
                            });
                        }
                        None if style == CellStyle::CrosswordBlock => (),
                        None => {
                            // A letter in a crossword square is set the size a
                            // newspaper sets it: a little over half the square,
                            // leaving the corner number its own air. Given the
                            // whole square it took the largest size that fit,
                            // which is a headline in a box and left the grid
                            // looking like a toy rather than a puzzle.
                            let letter = if style == CellStyle::Crossword {
                                let inset = rect.height / 5;
                                Rect {
                                    x: rect.x,
                                    y: rect.y.saturating_add(inset),
                                    width: rect.width,
                                    height: rect.height.saturating_sub(inset * 2).max(1),
                                }
                            } else {
                                rect
                            };
                            layout.nodes.push(LayoutNode {
                                id: *id,
                                rect: letter,
                                kind: LayoutKind::CellLabel(matches!(
                                    style,
                                    CellStyle::Board | CellStyle::BoardDark | CellStyle::Crossword
                                )),
                                text_lines: vec![cell.label.clone()],
                            });
                            if let Some(number) = cell.corner.filter(|n| (1..=99).contains(n)) {
                                let inset = metrics.rule_thickness() * 2;
                                layout.nodes.push(LayoutNode {
                                    id: *id,
                                    rect: Rect {
                                        x: rect.x + inset,
                                        y: rect.y + inset,
                                        width: rect.width / 4,
                                        height: (rect.height / 6)
                                            .max(with_text_scale(TextScale::Smallest, || {
                                                FontSize::Caption.line_height()
                                            })),
                                    },
                                    kind: LayoutKind::CellLabel(true),
                                    text_lines: vec![number.to_string()],
                                });
                            }
                        }
                    }
                }
            }
            let height = if rows == 0 {
                0
            } else {
                let block_gaps = if block_extra == 0 {
                    0
                } else {
                    (rows - 1) / 3 * block_extra
                };
                rows * cell_height + (rows - 1) * gutter + block_gaps
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
                if layout.nodes.len() + 9 > MAX_LAYOUT_NODES {
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
                // The button and the badge are measured next, for the same
                // reason: each has a width the title must not run under.
                let button = row.button.as_ref().map(|button| {
                    let label = one_line(&button.label, max(1, width / 3), FontSize::Body);
                    let wide = measure_text(&label, FontSize::Body)
                        .0
                        .saturating_add(2 * padding)
                        .max(metrics.touch_target_minimum());
                    (button.action, label, wide)
                });
                let button_column = button
                    .as_ref()
                    .map_or(0, |(_, _, wide)| wide.saturating_add(padding));
                let badge_column = if row.badge.is_some() {
                    row_badge_width(metrics).saturating_add(padding)
                } else {
                    0
                };
                let text_width = max(1, text_width - button_column - badge_column);
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
                let title_lines =
                    wrap_text_in(&row.title, text_width, FontSize::Body, layout.prose_face);
                let summary_lines = if row.summary.is_empty() {
                    Vec::new()
                } else {
                    wrap_text(&row.summary, text_width, FontSize::Caption)
                };
                let title_height =
                    title_lines.len() as i32 * FontSize::Body.line_height_in(layout.prose_face);
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
                if let Some(menu) = row.menu {
                    layout.context_actions.push((
                        Rect {
                            x,
                            y: cursor,
                            width,
                            height,
                        },
                        menu,
                    ));
                }
                let text_y = cursor.saturating_add((height - content) / 2);
                if row.state.is_check() {
                    // The box stands where an icon would, and at an icon's
                    // size, so it marks the title rather than the row.
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: lead_rect(
                            metrics,
                            RowLead::Icon(Glyph::Check),
                            x,
                            cursor,
                            icon,
                            height,
                            text_y,
                        ),
                        kind: LayoutKind::RowCheck(row.state),
                        text_lines: Vec::new(),
                    });
                } else {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: lead_rect(metrics, row.lead, x, cursor, icon, height, text_y),
                        kind: LayoutKind::RowLead(row.lead),
                        text_lines: Vec::new(),
                    });
                }
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: text_x,
                        y: text_y,
                        width: text_width,
                        height: title_height,
                    },
                    kind: match row.state {
                        RowState::Done => LayoutKind::RowTitleDone,
                        RowState::Open
                        | RowState::Checked
                        | RowState::Unchecked
                        | RowState::Mixed => LayoutKind::RowTitle,
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
                                .saturating_sub(button_column)
                                .saturating_sub(badge_column)
                                .saturating_sub(measured),
                            y: text_y,
                            width: measured,
                            height: FontSize::Caption.line_height(),
                        },
                        kind: LayoutKind::RowTrailing,
                        text_lines: vec![value],
                    });
                }
                if let Some(count) = row.badge {
                    let badge = row_badge_width(metrics);
                    let line = FontSize::Caption.line_height();
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(width)
                                .saturating_sub(padding)
                                .saturating_sub(menu_column)
                                .saturating_sub(button_column)
                                .saturating_sub(badge),
                            y: cursor.saturating_add((height - line) / 2),
                            width: badge,
                            height: line,
                        },
                        kind: LayoutKind::RowBadge,
                        text_lines: vec![badge_text(count)],
                    });
                }
                if let Some((action, label, wide)) = button {
                    // The whole height of the row is the target, as the
                    // overflow mark's is; the outline drawn inside it is only
                    // as tall as a line of type and its air.
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(width)
                                .saturating_sub(padding)
                                .saturating_sub(menu_column)
                                .saturating_sub(wide),
                            y: cursor,
                            width: wide,
                            height,
                        },
                        kind: LayoutKind::RowButton(action),
                        text_lines: vec![label],
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
            framed,
            bleed,
        } => {
            // Against the panel, not the column. Pulled up to the top edge
            // only when nothing has been placed above it: a picture that
            // follows a heading bleeds sideways but must not climb over what
            // it follows.
            let (x, width, y, bottom) = if *bleed {
                let top = if y <= metrics.screen_margin() { 0 } else { y };
                (0, metrics.width, top, metrics.height)
            } else {
                (x, width, y, bottom)
            };
            let ceiling = metrics
                .tenth_mm(i32::from(*max_height_tenths_mm))
                .min(bottom.saturating_sub(y).max(0));
            let (drawn_width, drawn_height) = fit_within(*source, width, ceiling);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: x + (width - drawn_width) / 2,
                    y,
                    width: drawn_width,
                    height: drawn_height,
                },
                kind: if *framed {
                    LayoutKind::FramedPicture(*handle)
                } else {
                    LayoutKind::Picture(*handle)
                },
                text_lines: Vec::new(),
            });
            y.saturating_add(drawn_height)
        }
        Node::TileGrid { id, tiles, shape } => {
            let columns = if legacy_typography() {
                metrics.grid_columns(*shape)
            } else {
                metrics.grid_columns_for_width(*shape, width)
            } as i32;
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
            let valued = tiles.iter().any(|tile| !tile.value.is_empty());
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
            let title_size = if *shape == TileShape::Card {
                FontSize::Body
            } else {
                FontSize::Caption
            };
            let title_lines = tiles
                .iter()
                .map(|tile| wrap_text(&tile.label, title_width, title_size).len())
                .max()
                .unwrap_or(1)
                .clamp(1, 2) as i32;
            let label_band = title_size.line_height() * title_lines
                + caption * (i32::from(subtitled) + i32::from(valued))
                + metrics.space(Space::Tight);
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
                // Tile, outline, mark, label, subtitle, value, state, badge:
                // eight is what one tile can put down now that every shape but
                // the legacy one is outlined.
                if layout.nodes.len() + 8 > MAX_LAYOUT_NODES {
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
                let state = if tile.state.is_tappable() {
                    ControlState::Enabled
                } else {
                    ControlState::Disabled
                };
                // Only the card carries an outline. A tile outline is drawn in
                // ink at the button weight, not in grey at the rule weight, so
                // on a grid it is a box around every cell rather than the
                // hairline it reads as in isolation. The launcher is a grid of
                // square tiles, and the box says nothing the position of the
                // tile does not already say.
                let outlined = *shape == TileShape::Card;
                let muted = outlined && state == ControlState::Disabled;
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x,
                        y: cell_y,
                        width: cell,
                        height: cell_height,
                    },
                    kind: LayoutKind::Tile(tile.action, state),
                    text_lines: Vec::new(),
                });
                if outlined {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: cell_x,
                            y: cell_y,
                            width: cell,
                            height: cell_height,
                        },
                        kind: LayoutKind::TileOutline(state),
                        text_lines: Vec::new(),
                    });
                }
                if tile.state.is_tappable() {
                    if let Some(menu) = tile.menu {
                        layout.context_actions.push((
                            Rect {
                                x: cell_x,
                                y: cell_y,
                                width: cell,
                                height: cell_height,
                            },
                            menu,
                        ));
                    }
                }
                // Fitted inside the body and centred, so a cover that is not
                // exactly the tile's proportion is letterboxed rather than
                // stretched. A stretched face is worse than a smaller one.
                let (mark, mark_width, mark_height) = if let Some(picture) = tile.picture {
                    let (width, height) = fit_within(picture.source, cell, body);
                    (LayoutKind::FramedPicture(picture.handle), width, height)
                } else {
                    let size = if legacy_typography() {
                        metrics.tenth_mm(110)
                    } else if *shape == TileShape::Card || *shape == TileShape::Square {
                        metrics.tenth_mm(55)
                    } else {
                        metrics.tenth_mm(70)
                    };
                    (
                        if muted {
                            LayoutKind::TileGlyphMuted(tile.glyph)
                        } else {
                            LayoutKind::TileGlyph(tile.glyph)
                        },
                        size,
                        size,
                    )
                };
                let inset = metrics.space(Space::Tight);
                // Mark and name are one object, centred together, rather than
                // a mark centred in the body with the name pinned to the cell's
                // bottom edge. Those are the same thing only when the mark
                // fills the body: a glyph is barely a third of it, so the name
                // ended up stranded a finger's width below its own icon and
                // hard against the tile's rule. Every phone home screen sets an
                // icon and its label as a pair for the same reason.
                let names = title_size.line_height() * title_lines
                    + caption * (i32::from(subtitled) + i32::from(valued));
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
                    &clamp_lines(&tile.label, label_width, title_size, title_lines as usize),
                    label_width,
                    title_size,
                );
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x + inset,
                        y: group_y.saturating_add(mark_height).saturating_add(inset),
                        width: label_width,
                        // Keep the reserved band above, but describe the
                        // actual text here so painting uses its measured size.
                        height: title_size.line_height()
                            * max_i32(1, i32::try_from(label.len()).unwrap_or(1)),
                    },
                    kind: if muted {
                        LayoutKind::TileLabelMuted
                    } else {
                        LayoutKind::TileLabel
                    },
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
                                .saturating_add(title_size.line_height() * title_lines),
                            width: label_width,
                            height: caption,
                        },
                        kind: LayoutKind::TileSubtitle,
                        text_lines: vec![one_line(&tile.subtitle, label_width, FontSize::Caption)],
                    });
                }
                if valued {
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: cell_x + inset,
                            y: group_y
                                .saturating_add(mark_height)
                                .saturating_add(inset)
                                .saturating_add(title_size.line_height() * title_lines)
                                .saturating_add(caption * i32::from(subtitled)),
                            width: label_width,
                            height: caption,
                        },
                        kind: LayoutKind::TileValue,
                        text_lines: vec![one_line(&tile.value, label_width, FontSize::Caption)],
                    });
                }
                // Both corner chips are square and the same size, so a tile
                // that carries a badge and a state reads as one pair rather
                // than two unrelated stickers. They are placed against the
                // body's corners, not the cell's, because the label band below
                // is the one part of the tile that is certainly text.
                let chip = caption.saturating_add(inset);
                let chip_inset = metrics.rule_thickness().saturating_mul(2);
                // A muted card drops its chip: a grid of cards that cannot be
                // used yet is explained once by the screen rather than by a
                // cross on every one of them. A square tile stands among
                // working tiles, so it keeps the mark that says why it refuses
                // a tap; without it, it is just a tile that appears broken.
                let chip_muted = muted && *shape == TileShape::Card;
                if tile.state.glyph().is_some() && !chip_muted {
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
        Node::PageRail { id, page, of } => {
            if *of > 1 {
                let rail_width = metrics.tenth_mm(4).max(1);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: x.saturating_add(width)
                            .saturating_add(metrics.space(Space::Tight)),
                        y,
                        width: rail_width,
                        height: bottom.saturating_sub(y),
                    },
                    kind: LayoutKind::PageRail {
                        page: (*page).min(of.saturating_sub(1)),
                        of: *of,
                    },
                    text_lines: Vec::new(),
                });
            }
            y
        }
        Node::Stepper {
            id,
            label,
            less,
            more,
            less_state,
            more_state,
            fill,
        } => {
            // Square controls at both ends, the reading between them. Square
            // because a stepper is two targets side by side and a finger is
            // round: making them the height of the row and no wider is what
            // keeps the gap between minus and plus wide enough that neither is
            // hit by accident.
            let row = metrics.touch_target_default();
            let side = row.min(width / 3).max(1);
            let middle = width.saturating_sub(side.saturating_mul(2)).max(1);
            let control =
                |action: &BarAction, state: ControlState, fallback: Glyph, at: i32| LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: at,
                        y,
                        width: side,
                        height: row,
                    },
                    kind: LayoutKind::StepperControl(
                        action.action,
                        state,
                        action.glyph.unwrap_or(fallback),
                    ),
                    text_lines: Vec::new(),
                };
            layout
                .nodes
                .push(control(less, *less_state, Glyph::Minus, x));
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: x.saturating_add(side),
                    y,
                    width: middle,
                    height: row,
                },
                kind: LayoutKind::StepperValue,
                text_lines: vec![clamp_lines(label, middle, FontSize::Body, 1)],
            });
            layout.nodes.push(control(
                more,
                *more_state,
                Glyph::Plus,
                x.saturating_add(side).saturating_add(middle),
            ));
            let mut cursor = y.saturating_add(row);
            if let Some(fill) = fill {
                let track = metrics.tenth_mm(8);
                cursor = cursor.saturating_add(metrics.space(Space::Tight));
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height: track,
                    },
                    kind: LayoutKind::StepperTrack(*fill),
                    text_lines: Vec::new(),
                });
                cursor = cursor.saturating_add(track);
            }
            cursor
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
        Node::Banner {
            id,
            level,
            text,
            action,
        } => {
            let padding = metrics.space(Space::Small);
            let mark = banner_mark_column(*level, metrics);
            let lines = wrap_text(text, width - 2 * padding - mark, FontSize::Body);
            let words = banner_height_for_lines(lines.len(), metrics);
            // The action is a line of its own rather than a button beside the
            // words, so the words keep the banner's whole measure. Beside
            // them, "Sign in again" at the largest text size left the reason
            // three words a line.
            let button = action.as_ref().map(|action| {
                let finger = banner_action_height(metrics);
                let inner = max(1, width - 2 * padding);
                let label = one_line(&action.label, max(1, inner - 2 * padding), FontSize::Body);
                let wide = measure_text(&label, FontSize::Body)
                    .0
                    .saturating_add(2 * padding)
                    .max(metrics.touch_target_minimum())
                    .min(inner);
                (action.action, label, wide, finger)
            });
            let height = button.as_ref().map_or(words, |(.., finger)| {
                words
                    .saturating_add(*finger)
                    .saturating_add(metrics.space(Space::Tight))
            });
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
            if let Some((action, label, wide, finger)) = button {
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: x.saturating_add(width).saturating_sub(padding + wide),
                        y: y.saturating_add(height)
                            .saturating_sub(padding)
                            .saturating_sub(finger),
                        width: wide,
                        height: finger,
                    },
                    kind: LayoutKind::BannerAction(action),
                    text_lines: vec![label],
                });
            }
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
                metrics.tenth_mm(if legacy_typography() { 140 } else { 80 })
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
            let visible_rows = if legacy_typography() && !flow_height_bound() {
                usize::MAX
            } else {
                usize::try_from(max(0, bottom.saturating_sub(y)) / max(1, cell_height)).unwrap_or(0)
            };
            let lines: Vec<String> = rows
                .iter()
                .take(MAX_TERMINAL_ROWS)
                .take(visible_rows)
                // Clipped, never wrapped. A row that overflowed onto the next
                // one would shift every row below it and the grid would stop
                // being a grid.
                .map(|row| terminal_row_prefix(row, columns as usize))
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
                    let (under, under_width) = lines
                        .get(row as usize)
                        .map_or((' ', 1), |line| terminal_cell_at(line, column as usize));
                    layout.nodes.push(LayoutNode {
                        id: *id,
                        rect: Rect {
                            x: x.saturating_add(column * cell_width),
                            y: y.saturating_add(row * cell_height),
                            width: cell_width.saturating_mul(under_width as i32),
                            height: cell_height,
                        },
                        kind: LayoutKind::TerminalCursor,
                        text_lines: vec![under.to_string()],
                    });
                }
            }
            y.saturating_add(height)
        }
        Node::Toggle {
            id,
            action,
            label,
            summary,
            on,
            state,
        } => layout_toggle(
            *id,
            *action,
            (label, summary),
            *on,
            *state,
            Rect {
                x,
                y,
                width,
                height: 0,
            },
            metrics,
            layout,
        ),
        Node::SelectionBar { id, count, actions } => {
            layout_selection_bar(*id, *count, actions, x, y, width, metrics, layout)
        }
        Node::Options {
            id,
            prompt,
            options,
            selected,
            page,
            previous,
            next,
        } => layout_options(
            *id,
            prompt,
            options,
            (*selected, *page),
            (*previous, *next),
            Rect {
                x,
                y,
                width,
                height: 0,
            },
            metrics,
            layout,
        ),
        Node::Code {
            id,
            payload,
            caption,
        } => {
            let side = code_side(width, metrics);
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x: x.saturating_add((width - side) / 2),
                    y,
                    width: side,
                    height: side,
                },
                kind: LayoutKind::Code,
                text_lines: vec![payload.clone()],
            });
            let mut cursor = y.saturating_add(side);
            let lines = code_caption_lines(caption, width);
            if !lines.is_empty() {
                cursor = cursor.saturating_add(metrics.space(Space::Tight));
                let height =
                    i32::try_from(lines.len()).unwrap_or(1) * FontSize::Caption.line_height();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height,
                    },
                    kind: LayoutKind::CodeCaption,
                    text_lines: lines,
                });
                cursor = cursor.saturating_add(height);
            }
            cursor
        }
        Node::Steps {
            id,
            current,
            total,
            labels,
        } => {
            let total = (*total).clamp(1, MAX_STEPS as u8);
            let current = (*current).min(total - 1);
            let line = FontSize::Caption.line_height();
            let marks = steps_marks_width(total, metrics);
            let text = one_line(
                &steps_text(current, total, labels),
                max(1, width - marks - metrics.space(Space::Small)),
                FontSize::Caption,
            );
            layout.nodes.push(LayoutNode {
                id: *id,
                rect: Rect {
                    x,
                    y,
                    width,
                    height: line,
                },
                kind: LayoutKind::Steps(current, total),
                text_lines: vec![text],
            });
            y.saturating_add(line)
        }
    }
}

/// How tall a row of one line of label and, when there is one, one line of
/// summary is: the pitch a toggle and an option keep whatever their words.
///
/// The `Node::Rows` arithmetic for a row whose title and summary each fit on a
/// line, so a toggle set between two rows is the height of either of them.
#[must_use]
pub fn one_line_row_height(metrics: &DisplayMetrics, summary: bool) -> i32 {
    let content = FontSize::Body.line_height().saturating_add(if summary {
        FontSize::Caption.line_height()
    } else {
        0
    });
    max(
        metrics.touch_target_default(),
        content.saturating_add(metrics.space(Space::Small) * 2),
    )
}

/// The column the switch takes at a toggle's trailing edge: a finger wide,
/// whatever the text size, because the switch is a control and not type.
fn toggle_switch_column(metrics: &DisplayMetrics) -> i32 {
    metrics.touch_target_default()
}

/// The track a toggle's switch is drawn in, inside its column: nine
/// millimetres by five, set against the trailing edge and centred on the row.
fn toggle_track(column: Rect, metrics: &DisplayMetrics) -> Rect {
    let width = metrics.tenth_mm(90).min(column.width);
    let height = metrics.tenth_mm(50).min(column.height);
    Rect {
        x: column.x + column.width - width,
        y: column.y + (column.height - height) / 2,
        width,
        height,
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_toggle(
    id: NodeId,
    action: ActionId,
    (label, summary): (&str, &str),
    on: bool,
    state: ControlState,
    at: Rect,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    let Rect { x, y, width, .. } = at;
    let has_summary = !summary.is_empty();
    let height = one_line_row_height(metrics, has_summary);
    let switch = toggle_switch_column(metrics);
    let text_width = max(1, width - switch - metrics.space(Space::Small));
    let title = FontSize::Body.line_height();
    let under = if has_summary {
        FontSize::Caption.line_height()
    } else {
        0
    };
    let text_y = y + (height - title - under) / 2;
    layout.nodes.push(LayoutNode {
        id,
        rect: Rect {
            x,
            y,
            width,
            height,
        },
        kind: LayoutKind::Toggle(action, state),
        text_lines: Vec::new(),
    });
    layout.nodes.push(LayoutNode {
        id,
        rect: Rect {
            x,
            y: text_y,
            width: text_width,
            height: title,
        },
        kind: LayoutKind::ToggleLabel(state),
        text_lines: vec![one_line(label, text_width, FontSize::Body)],
    });
    if has_summary {
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x,
                y: text_y + title,
                width: text_width,
                height: under,
            },
            kind: LayoutKind::RowSummary,
            text_lines: vec![one_line(summary, text_width, FontSize::Caption)],
        });
    }
    layout.nodes.push(LayoutNode {
        id,
        rect: Rect {
            x: x + width - switch,
            y,
            width: switch,
            height,
        },
        kind: LayoutKind::ToggleSwitch(on, state),
        text_lines: Vec::new(),
    });
    y.saturating_add(height)
}

/// How tall the selection bar is: a finger, or a line of type and its air
/// when that is more.
#[must_use]
pub fn selection_bar_height(metrics: &DisplayMetrics) -> i32 {
    max(
        metrics.touch_target_default(),
        FontSize::Body
            .line_height()
            .saturating_add(metrics.space(Space::Small) * 2),
    )
}

/// What the selection bar says about its count.
fn selection_count_text(count: u32) -> String {
    if count == 0 {
        "None selected".to_owned()
    } else {
        format!("{count} selected")
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_selection_bar(
    id: NodeId,
    count: u32,
    actions: &[BarAction],
    x: i32,
    y: i32,
    width: i32,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    let padding = metrics.space(Space::Small);
    let height = selection_bar_height(metrics);
    let actions = &actions[..actions.len().min(MAX_SELECTION_ACTIONS)];
    // The count is measured first and given at most half the bar, and the
    // verbs share what is left equally, so a long verb is ellipsised in its
    // own share rather than taking a neighbour's.
    let count_text = one_line(
        &selection_count_text(count),
        max(1, width / 2 - padding * 2),
        FontSize::Body,
    );
    let count_width = measure_text(&count_text, FontSize::Body)
        .0
        .saturating_add(padding * 2);
    layout.nodes.push(LayoutNode {
        id,
        rect: Rect {
            x,
            y,
            width,
            height,
        },
        kind: LayoutKind::SelectionBar,
        text_lines: vec![count_text],
    });
    let shown = i32::try_from(actions.len()).unwrap_or(1).max(1);
    let share = max(1, (width - count_width) / shown);
    let mut right = x.saturating_add(width);
    for action in actions.iter().rev() {
        let label = one_line(&action.label, max(1, share - padding * 2), FontSize::Body);
        let wide = measure_text(&label, FontSize::Body)
            .0
            .saturating_add(padding * 2)
            .max(metrics.touch_target_minimum())
            .min(share);
        right = right.saturating_sub(wide);
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x: right,
                y,
                width: wide,
                height,
            },
            kind: LayoutKind::SelectionAction(action.action),
            text_lines: vec![label],
        });
    }
    y.saturating_add(height)
}

/// How many rows one page of an option list holds on this panel at this text
/// size.
///
/// A function of the panel and the text size alone, so an application can
/// work out which page holds its answer, and which page is the last, before
/// it builds a screen. Counted for the worst screen an option list is put on:
/// the status band, a top bar and its rule, a one line prompt, the pager and a
/// bottom bar or bottom action, so a list that fits there fits everywhere, and
/// a screen without a bottom bar has the bar's height left over for a line of
/// its own.
#[must_use]
pub fn options_per_page(metrics: &DisplayMetrics, summaries: bool) -> usize {
    with_text_scale(metrics.text_scale, || {
        options_per_page_at_current_scale(metrics, summaries)
    })
}

/// The same count, at whatever text size is in force. Layout calls this
/// rather than [`options_per_page`], because a screen may set a text size of
/// its own in place of the reader's, and the rows have to be counted at the
/// size they are drawn at.
fn options_per_page_at_current_scale(metrics: &DisplayMetrics, summaries: bool) -> usize {
    {
        let gap = metrics.space(Space::Tight);
        // The top bar is drawn with a rule under it, and content starts below
        // the rule. Leaving the rule out counted a few pixels the panel does
        // not have, which on the odd panel and size was the difference
        // between the pager and the bottom action sharing a line.
        let chrome = metrics
            .status_band_height()
            .saturating_add(metrics.top_bar_height())
            .saturating_add(metrics.rule_thickness())
            .saturating_add(gap)
            .saturating_add(metrics.nav_bar_height());
        let prompt = FontSize::Body
            .line_height()
            .saturating_add(metrics.space(Space::Small));
        let pager = options_pager_height(metrics).saturating_add(gap);
        let room = metrics
            .height
            .saturating_sub(chrome)
            .saturating_sub(prompt)
            .saturating_sub(pager);
        let pitch = one_line_row_height(metrics, summaries).saturating_add(gap * 2);
        let rows = room.saturating_add(gap * 2) / pitch.max(1);
        usize::try_from(rows)
            .unwrap_or(1)
            .clamp(1, MAX_OPTIONS_PER_PAGE)
    }
}

/// How many pages an option list of `count` answers takes on this panel.
#[must_use]
pub fn options_pages(count: usize, metrics: &DisplayMetrics, summaries: bool) -> usize {
    count
        .min(MAX_OPTIONS)
        .div_ceil(options_per_page(metrics, summaries))
        .max(1)
}

/// The band under an option list's rows that turns its pages.
fn options_pager_height(metrics: &DisplayMetrics) -> i32 {
    max(
        metrics.touch_target_default(),
        FontSize::Caption.line_height(),
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_options(
    id: NodeId,
    prompt: &str,
    options: &[OptionRow],
    (selected, page): (Option<u16>, u16),
    (previous, next): (ActionId, ActionId),
    at: Rect,
    metrics: &DisplayMetrics,
    layout: &mut Layout,
) -> i32 {
    let Rect { x, y, width, .. } = at;
    let gap = metrics.space(Space::Tight);
    let padding = metrics.space(Space::Small);
    let options = &options[..options.len().min(MAX_OPTIONS)];
    let summaries = options.iter().any(|option| !option.summary.is_empty());
    let per_page = options_per_page_at_current_scale(metrics, summaries);
    let pages = options.len().div_ceil(per_page).max(1);
    let page = usize::from(page).min(pages - 1);
    let mut cursor = y;
    if !prompt.is_empty() {
        let line = FontSize::Body.line_height();
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x,
                y: cursor,
                width,
                height: line,
            },
            kind: LayoutKind::ChoicePrompt,
            text_lines: vec![one_line(prompt, width, FontSize::Body)],
        });
        cursor = cursor.saturating_add(line).saturating_add(padding);
    }
    let height = one_line_row_height(metrics, summaries);
    let mark = row_mark_column(metrics);
    let text_x = x.saturating_add(mark).saturating_add(padding);
    let text_width = max(1, width - mark - padding * 2);
    let title = FontSize::Body.line_height();
    let under = if summaries {
        FontSize::Caption.line_height()
    } else {
        0
    };
    let first = page * per_page;
    let top = cursor;
    for (index, option) in options.iter().enumerate().skip(first).take(per_page) {
        if layout.nodes.len() + 8 > MAX_LAYOUT_NODES {
            break;
        }
        if index > first {
            layout.nodes.push(LayoutNode {
                id,
                rect: Rect {
                    x: text_x,
                    y: cursor,
                    width: max(1, width - mark - padding),
                    height: metrics.rule_thickness(),
                },
                kind: LayoutKind::RowRule,
                text_lines: Vec::new(),
            });
            cursor = cursor.saturating_add(gap);
        }
        let chosen = selected.is_some_and(|selected| usize::from(selected) == index);
        let text_y = cursor + (height - title - under) / 2;
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x,
                y: cursor,
                width,
                height,
            },
            kind: LayoutKind::OptionRow(option.action, chosen),
            text_lines: Vec::new(),
        });
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x: text_x,
                y: text_y,
                width: text_width,
                height: title,
            },
            kind: LayoutKind::OptionLabel,
            text_lines: vec![one_line(&option.label, text_width, FontSize::Body)],
        });
        if !option.summary.is_empty() {
            layout.nodes.push(LayoutNode {
                id,
                rect: Rect {
                    x: text_x,
                    y: text_y + title,
                    width: text_width,
                    height: under,
                },
                kind: LayoutKind::RowSummary,
                text_lines: vec![one_line(&option.summary, text_width, FontSize::Caption)],
            });
        }
        cursor = cursor.saturating_add(height).saturating_add(gap);
    }
    if pages == 1 {
        return cursor.saturating_sub(gap).max(top);
    }
    // A short last page keeps the room of a full one, so the pager stays
    // under the finger that turned to it instead of rising to meet the rows.
    let pitch = height.saturating_add(gap * 2);
    let rows = i32::try_from(per_page).unwrap_or(1);
    let band_y = top
        .saturating_add(pitch.saturating_mul(rows))
        .saturating_sub(gap);
    let band = Rect {
        x,
        y: band_y,
        width,
        height: options_pager_height(metrics),
    };
    layout.nodes.push(LayoutNode {
        id,
        rect: band,
        kind: LayoutKind::PagePosition,
        text_lines: vec![format!("{} of {pages}", page + 1)],
    });
    let side = min(metrics.touch_target_default(), width / 3);
    if page > 0 {
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                width: side,
                ..band
            },
            kind: LayoutKind::PagePrevious(previous),
            text_lines: Vec::new(),
        });
    }
    if page + 1 < pages {
        layout.nodes.push(LayoutNode {
            id,
            rect: Rect {
                x: band.x + band.width - side,
                width: side,
                ..band
            },
            kind: LayoutKind::PageNext(next),
            text_lines: Vec::new(),
        });
    }
    band_y.saturating_add(band.height)
}

/// The side of the square a code is drawn in: the column, up to
/// [`CODE_SIDE_TENTH_MM`].
#[must_use]
pub fn code_side(width: i32, metrics: &DisplayMetrics) -> i32 {
    max(0, width.min(metrics.tenth_mm(CODE_SIDE_TENTH_MM)))
}

/// A code's caption, wrapped to the column and cut at two lines.
fn code_caption_lines(caption: &str, width: i32) -> Vec<String> {
    if caption.is_empty() {
        return Vec::new();
    }
    let lines = wrap_text(caption, width, FontSize::Caption);
    if lines.len() <= 2 {
        return lines;
    }
    wrap_text(
        &clamp_lines(caption, width, FontSize::Caption, 2),
        width,
        FontSize::Caption,
    )
    .into_iter()
    .take(2)
    .collect()
}

/// The words a step indicator sets: "Step 2 of 4", and the step's name after
/// a colon when it has one.
fn steps_text(current: u8, total: u8, labels: &[String]) -> String {
    let position = format!("Step {} of {total}", u16::from(current) + 1);
    match labels
        .get(usize::from(current))
        .filter(|label| !label.is_empty())
    {
        Some(label) if labels.len() == usize::from(total) => format!("{position}: {label}"),
        _ => position,
    }
}

/// One step's mark, and the gap after it, in pixels. Physical rather than
/// scaled with the type, like every other mark that is not a letter.
fn steps_mark(metrics: &DisplayMetrics) -> (i32, i32) {
    (metrics.tenth_mm(16).max(2), metrics.tenth_mm(12).max(1))
}

/// The run of marks at a step indicator's trailing edge.
fn steps_marks_width(total: u8, metrics: &DisplayMetrics) -> i32 {
    let (mark, gap) = steps_mark(metrics);
    let total = i32::from(total);
    mark.saturating_mul(total)
        .saturating_add(gap.saturating_mul(total.saturating_sub(1)))
}

/// The width a row's badge takes whatever it says: the width of "999+" in
/// the caption figures, with air either side.
#[must_use]
pub fn row_badge_width(metrics: &DisplayMetrics) -> i32 {
    figures_width("999+", FontSize::Caption, Face::Text)
        .saturating_add(metrics.space(Space::Tight) * 2)
}

/// What a badge says. Past 999 the number stops being read and starts being
/// counted, so it stops there.
fn badge_text(count: u32) -> String {
    if count > 999 {
        "999+".to_owned()
    } else {
        count.to_string()
    }
}

/// The distance from one secret dot to the next.
fn secret_dot_pitch(metrics: &DisplayMetrics) -> i32 {
    let (dot, gap) = secret_dot(metrics);
    dot.saturating_add(gap)
}

/// A secret dot's diameter and the gap after it: a third of the body em, and
/// a little less again between them.
fn secret_dot(metrics: &DisplayMetrics) -> (i32, i32) {
    let em = FontSize::Body.em_in(Face::Text);
    let _ = metrics;
    (max(2, em / 3), max(1, em / 4))
}

/// How many caption lines the note under a field takes: the longer of the
/// hint and the error, and never more than two.
///
/// Measured across both rather than across whichever is showing, so an error
/// arriving in place of a hint of the same length moves nothing on the panel.
fn field_note_lines(hint: Option<&str>, error: Option<&str>, width: i32) -> usize {
    [hint, error]
        .into_iter()
        .flatten()
        .filter(|text| !text.is_empty())
        .map(|text| wrap_text(text, width, FontSize::Caption).len().max(1))
        .max()
        .unwrap_or(0)
        .min(2)
}

/// The height a banner's action takes: a finger, or a line of type when that
/// is taller.
fn banner_action_height(metrics: &DisplayMetrics) -> i32 {
    max(metrics.touch_target_default(), FontSize::Body.line_height())
}

/// The column a success or an error banner gives its mark ahead of the words.
/// Info and attention give none, so their banners are laid out exactly as
/// they always were.
fn banner_mark_column(level: BannerLevel, metrics: &DisplayMetrics) -> i32 {
    match level {
        BannerLevel::Info | BannerLevel::Attention => 0,
        BannerLevel::Success | BannerLevel::Error => FontSize::Body
            .line_height()
            .saturating_add(metrics.space(Space::Tight)),
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
    // The flow gap between the terminal and that next node is not a terminal
    // row. Including it can negotiate one row more than layout can draw.
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
        .min();
    let available = floor.map_or_else(
        || bottom.saturating_sub(terminal.rect.y),
        |floor| {
            min(floor, bottom)
                .saturating_sub(terminal.rect.y)
                .saturating_sub(metrics.space(Space::Tight))
        },
    );
    terminal_grid(terminal.rect.width, available)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FontSize {
    /// Dense monospace terminal text; interface labels retain their usual sizes.
    Terminal,
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
    /// were, because the top of a scale is where shouting starts.
    ///
    /// Title and Heading are now set in a bold cut, so weight rather than size
    /// says which of two lines is the heading. That is what lets these two
    /// numbers stay where they are instead of climbing again: a heavier
    /// heading at 5.4 mm separates from body far more clearly than a regular
    /// one did at 6.8 mm, and takes a third less of the panel doing it. The
    /// weight is chosen inside the typesetter from the size, so nothing here
    /// and nothing in an application asks for it.
    #[must_use]
    pub const fn tenth_mm(self) -> i32 {
        match self {
            Self::Terminal => 18,
            Self::Caption => 24,
            Self::Body => 31,
            Self::Title => 42,
            Self::Heading => 54,
        }
    }

    /// The size a heading at `level` is set at, counting levels from one.
    ///
    /// The one place this is decided. Pagination measures a heading and the
    /// renderer draws it, and when those two disagreed by a single step the
    /// line a heading was measured as two and drawn as three pushed the last
    /// line of the page over the page number.
    #[must_use]
    pub const fn for_heading_level(level: u8) -> Self {
        if level <= 1 {
            Self::Heading
        } else {
            // Level three is already small enough that going smaller again
            // would barely separate it from the text under it.
            Self::Title
        }
    }

    /// The legacy bitmap scale factor, used only by the built-in fallback.
    #[must_use]
    pub const fn scale(self) -> i32 {
        match self {
            Self::Terminal | Self::Caption => 2,
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
        with_typesetter(face, |typesetter| typesetter.line_height(self, face))
            .unwrap_or_else(|| self.fallback_line_height_in(face))
    }

    /// The em in pixels: the size the type is actually set at.
    ///
    /// What anything set beside the text rather than in it is scaled against.
    #[must_use]
    pub fn em_in(self, face: Face) -> i32 {
        with_typesetter(face, |typesetter| typesetter.em(self, face))
            .unwrap_or_else(|| self.fallback_line_height_in(face))
            .max(1)
    }

    /// The built-in bitmap's line height, at the current type size.
    ///
    /// Scaled like the real one. Without this a host test could not tell two
    /// type sizes apart, and pagination is exactly the thing that has to be
    /// tested without hardware.
    #[must_use]
    pub fn fallback_line_height(self) -> i32 {
        self.fallback_line_height_in(Face::Text)
    }

    /// The same, for a named face, so prose follows the reading size and the
    /// interface around it does not.
    #[must_use]
    pub fn fallback_line_height_in(self, face: Face) -> i32 {
        (self.unscaled_fallback_line_height() * scale_percent(face) + 50) / 100
    }

    const fn unscaled_fallback_line_height(self) -> i32 {
        match self {
            Self::Terminal | Self::Caption => 18,
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
    /// The em in pixels for a size: the size the type is actually set at.
    ///
    /// Distinct from the line height, which is the em plus whatever the face
    /// and the reader want between lines. Anything set alongside the text
    /// rather than in it -- a typeset formula, most of all -- has to be scaled
    /// against this or it comes out a fifth too large.
    ///
    /// The default is the line height, which is wrong but is the right shape:
    /// a typesetter that cannot say has to say something.
    fn em(&self, size: FontSize, face: Face) -> i32 {
        self.line_height(size, face)
    }
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

/// Publisher faces uploaded by the active application.
///
/// Parsing remains outside this crate; it receives the same bounded
/// [`Typesetter`] interface as the system face. The map is intentionally
/// process-local and handles are namespaced by the runtime's application
/// session, so clearing it when an app leaves releases every glyph cache.
static BOOK_TYPESETTERS: OnceLock<Mutex<BTreeMap<FontHandle, Box<dyn Typesetter>>>> =
    OnceLock::new();

thread_local! {
    static READING_FONT: std::cell::Cell<Option<FontHandle>> = const { std::cell::Cell::new(None) };
    static LEGACY_TYPOGRAPHY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FLOW_HEIGHT_BOUND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static IN_BAND_SLOT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lays out one band slot with its buttons filling the slot.
///
/// A row of buttons is a set, and a set reads as one when its members are the
/// same size. Left at their own widths, "Solo round" and "How to play" each sat
/// in the middle of half a panel with different paper either side, so two rows
/// of two made a ragged pyramid rather than a grid, and a stacked band became
/// a column of boxes of four different widths.
fn with_band_slot<T>(body: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            IN_BAND_SLOT.with(|slot| slot.set(self.0));
        }
    }
    let previous = IN_BAND_SLOT.with(|slot| slot.replace(true));
    let _restore = Restore(previous);
    body()
}

fn in_band_slot() -> bool {
    IN_BAND_SLOT.with(std::cell::Cell::get)
}

fn with_flow_height_bound<T>(bounded: bool, body: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            FLOW_HEIGHT_BOUND.with(|slot| slot.set(self.0));
        }
    }
    let previous = FLOW_HEIGHT_BOUND.with(|slot| {
        let previous = slot.get();
        slot.set(bounded || previous);
        previous
    });
    let _restore = Restore(previous);
    body()
}

fn flow_height_bound() -> bool {
    FLOW_HEIGHT_BOUND.with(std::cell::Cell::get)
}

/// Runs layout or painting with pre-Folio interface typography selected.
///
/// Only the protocol decoder uses this for version-11 screens. Keeping the
/// choice scoped to one frame prevents an old app's local text measurements
/// from drifting when a 0.3.5 runtime also renders Folio screens.
pub fn with_legacy_typography<T>(legacy: bool, body: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            LEGACY_TYPOGRAPHY.with(|slot| slot.set(self.0));
        }
    }
    let previous = LEGACY_TYPOGRAPHY.with(|slot| {
        let previous = slot.get();
        slot.set(legacy);
        previous
    });
    let _restore = Restore(previous);
    body()
}

/// Whether the currently laid-out frame must use protocol-11 type metrics.
#[must_use]
pub fn legacy_typography() -> bool {
    LEGACY_TYPOGRAPHY.with(std::cell::Cell::get)
}

/// Installs or replaces one bounded publisher face.
pub fn put_book_typesetter(handle: FontHandle, typesetter: Box<dyn Typesetter>) {
    if let Ok(mut fonts) = BOOK_TYPESETTERS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
    {
        fonts.insert(handle, typesetter);
    }
}

/// Releases a publisher face and its raster cache.
pub fn drop_book_typesetter(handle: FontHandle) {
    if let Some(fonts) = BOOK_TYPESETTERS.get() {
        if let Ok(mut fonts) = fonts.lock() {
            fonts.remove(&handle);
        }
    }
}

/// Runs layout or painting with one publisher face selected for reading prose.
pub fn with_reading_font<T>(font: Option<FontHandle>, body: impl FnOnce() -> T) -> T {
    struct Restore(Option<FontHandle>);
    impl Drop for Restore {
        fn drop(&mut self) {
            READING_FONT.with(|slot| slot.set(self.0));
        }
    }
    let previous = READING_FONT.with(|slot| {
        let previous = slot.get();
        slot.set(font);
        previous
    });
    let _restore = Restore(previous);
    body()
}

fn with_typesetter<T>(face: Face, body: impl FnOnce(&dyn Typesetter) -> T) -> Option<T> {
    if face == Face::Reading {
        if let Some(handle) = READING_FONT.with(std::cell::Cell::get) {
            if let Some(fonts) = BOOK_TYPESETTERS.get() {
                if let Ok(fonts) = fonts.lock() {
                    if let Some(typesetter) = fonts.get(&handle) {
                        return Some(body(typesetter.as_ref()));
                    }
                }
            }
        }
    }
    TYPESETTER.get().map(|typesetter| body(typesetter.as_ref()))
}

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
    static READING_SCALE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
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

/// Sets the size of book prose, and of nothing else.
///
/// Separate from [`set_text_scale`] because the two answer different people.
/// The text scale is an accessibility preference: somebody has said how large
/// they need an interface to be, and a title bar, a page number and a button
/// are all interface. The reading scale is a reader saying how large they want
/// *this book*, which is a decision about the page and not about the device.
///
/// They were one value until a reader on a Clara found that making a novel
/// larger also grew the book's name in the bar above it, took the height out
/// of the page to do it, and left less room for the larger type than there had
/// been for the smaller.
pub fn set_reading_scale(scale: TextScale) {
    READING_SCALE.with(|slot| slot.set(scale.wire_value()));
}

/// The size book prose is currently being set at.
#[must_use]
pub fn reading_scale() -> TextScale {
    TextScale::from_wire(READING_SCALE.with(std::cell::Cell::get)).unwrap_or_default()
}

/// The percentage in force for one face.
///
/// The single place that decides which of the two ambient sizes a face obeys,
/// so a new caller cannot get the answer half right.
#[must_use]
pub fn scale_percent(face: Face) -> i32 {
    match face {
        Face::Reading => reading_scale().percent(),
        Face::Text | Face::Mono => text_scale().percent(),
    }
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

/// Runs `body` with book prose at `scale`, putting it back afterwards.
///
/// What an application calls to measure a book at a size it is not showing
/// yet, which is every repagination after the reader touches the stepper.
pub fn with_reading_scale<T>(scale: TextScale, body: impl FnOnce() -> T) -> T {
    struct Restore(TextScale);
    impl Drop for Restore {
        fn drop(&mut self) {
            set_reading_scale(self.0);
        }
    }
    let _restore = Restore(reading_scale());
    set_reading_scale(scale);
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
    if let Some(measured) = with_typesetter(face, |typesetter| typesetter.measure(text, size, face))
    {
        return measured;
    }
    let scale = size.scale();
    let glyphs = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
    let percent = scale_percent(face);
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
    with_typesetter(face, |typesetter| {
        text.chars()
            .find(|character| !character.is_whitespace() && !typesetter.has_glyph(*character, face))
    })?
}

/// The size of one monospace cell: what a character grid is built from.
///
/// Returns width and height together because a caller that needs one always
/// needs the other, and taking them from a single call means a grid can never
/// be sized from two different answers.
#[must_use]
pub fn mono_cell(size: FontSize) -> (i32, i32) {
    TYPESETTER.get().map_or_else(
        || (6 * size.scale(), size.fallback_line_height_in(Face::Mono)),
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
            width: if legacy_typography() {
                self.content_width()
            } else {
                self.readable_width()
            },
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
    paginate_tagged_below(paragraphs, metrics, area, 0)
}

/// The same, for a screen that draws something of its own above the first page
/// and nothing above the rest.
///
/// `taken` is how much of the first page that something occupies. A detail
/// screen is usually this shape: a block of facts, a picture or a byline at
/// the head of a long piece of prose, and only at the head of it. Measuring
/// the whole document against the shortened page wastes a line on every page
/// after the first; measuring it against the full page draws the last
/// paragraph of the first page through whatever is below it.
///
/// Applications reached for the alternative before this existed, which was to
/// reserve paragraphs of roughly the right height and swap them for the real
/// block while drawing. It is close, and close is a paragraph over the edge:
/// a block of four facts is a dozen pixels taller than four paragraphs of the
/// same words, which is an eighth of a line, which is one line too many at the
/// foot of the page.
#[must_use]
pub fn paginate_tagged_below(
    paragraphs: &[(u32, u8, QuoteRole, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
    taken: i32,
) -> Vec<Vec<(u32, u8, QuoteRole, String)>> {
    let mut pages: Vec<Page> = Vec::new();
    let mut page: Page = Vec::new();
    // Counted as though the block were the first thing on the page, so the
    // gap between it and the first paragraph is the gap between any two.
    let mut used = taken.max(0);
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
            // A gap above the first paragraph as well, when something the
            // paginator did not place is already standing on the page.
            let spacing = if page.is_empty() && used == 0 {
                0
            } else {
                area.gap
            };
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
/// Text width for rows whose leading column reserves room for cover artwork.
#[must_use]
pub fn cover_row_text_width(metrics: &DisplayMetrics, area: ProseArea) -> i32 {
    row_text_width_beside(metrics, area, metrics.touch_target_default())
}

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

/// A boxed page of a document's first lines, with the format named inside
/// the frame at the trailing foot.
///
/// This is the cover a shelf draws when the document has no artwork: the
/// words themselves, not a generic glyph. A pale empty rectangle on this
/// panel reads as a decoding fault; a framed first page reads as a book
/// whose jacket was never printed.
#[must_use]
pub fn document_preview(text: &str, badge: &str, width: u32, height: u32) -> Vec<u8> {
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
    surface.clear(tone::PAPER);
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
    let inset = max(rule * 3, bounds.width / 12);
    let measure = bounds.width - inset * 2;
    if measure <= 0 {
        stamp_format_badge_on(&mut surface, badge);
        return surface.pixels;
    }
    let chip = format_badge_size(badge);
    let text_bottom = (bounds.height - inset - chip.1 - inset / 2).max(inset);
    let line = FontSize::Caption.line_height_in(Face::Reading);
    let room = ((text_bottom - inset) / line).max(1);
    let room = usize::try_from(room).unwrap_or(1);
    let mut lines = wrap_text_in(text, measure, FontSize::Caption, Face::Reading);
    if lines.len() > room {
        lines.truncate(room);
        if let Some(last) = lines.last_mut() {
            *last = one_line(&format!("{last}…"), measure, FontSize::Caption);
        }
    }
    draw_lines_in(
        &mut surface,
        &lines,
        inset,
        inset,
        FontSize::Caption,
        Face::Reading,
        tone::INK,
        bounds,
    );
    stamp_format_badge_on(&mut surface, badge);
    surface.pixels
}

/// Paints a format chip in the trailing foot of an existing cover.
///
/// The chip lives inside the picture so a shelf does not grow a second
/// chrome for "this is a PDF". Empty or oversized badges are ignored.
pub fn stamp_format_badge(pixels: &mut [u8], width: u32, height: u32, badge: &str) {
    let Ok(pixel_width) = usize::try_from(width) else {
        return;
    };
    let Ok(pixel_height) = usize::try_from(height) else {
        return;
    };
    if pixels.len() != pixel_width.saturating_mul(pixel_height) {
        return;
    }
    let mut surface = Surface {
        width: pixel_width,
        height: pixel_height,
        pixels: pixels.to_vec(),
        chroma: None,
    };
    stamp_format_badge_on(&mut surface, badge);
    pixels.copy_from_slice(&surface.pixels);
}

fn format_badge_size(badge: &str) -> (i32, i32) {
    let label: String = badge.chars().take(TILE_BADGE_LIMIT).collect();
    let text = measure_text_in(&label, FontSize::Caption, Face::Text);
    let pad = FontSize::Caption.line_height() / 4;
    (
        (text.0 + pad * 2).max(FontSize::Caption.line_height()),
        text.1 + pad,
    )
}

fn stamp_format_badge_on(surface: &mut Surface, badge: &str) {
    let label: String = badge
        .chars()
        .take(TILE_BADGE_LIMIT)
        .collect::<String>()
        .to_ascii_lowercase();
    if label.is_empty() {
        return;
    }
    let bounds = Rect {
        x: 0,
        y: 0,
        width: i32::try_from(surface.width).unwrap_or(i32::MAX),
        height: i32::try_from(surface.height).unwrap_or(i32::MAX),
    };
    let (chip_width, chip_height) = format_badge_size(&label);
    let inset = max(2, bounds.width / 24);
    let rect = Rect {
        x: bounds.width - inset - chip_width,
        y: bounds.height - inset - chip_height,
        width: chip_width,
        height: chip_height,
    };
    surface.fill_rect(rect, tone::INK);
    let pad = FontSize::Caption.line_height() / 4;
    draw_text_in(
        surface,
        &label,
        rect.x + pad,
        rect.y + pad / 2,
        FontSize::Caption,
        Face::Text,
        tone::PAPER,
        bounds,
    );
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

/// The banner's actual measured height, for paginators reserving an inline
/// result before their rows. `width` is the content width, including padding.
#[must_use]
pub fn banner_height(text: &str, width: i32, metrics: &DisplayMetrics) -> i32 {
    banner_height_with(text, BannerLevel::Info, false, width, metrics)
}

/// The same, for a banner of any level and with or without its action.
///
/// A success or an error banner gives a column to its mark, so its words wrap
/// narrower than an info banner's, and an action takes a line of its own
/// under the words. A paginator that measured only the words reserved a line
/// too little for each, and the last row of the page went under the bar,
/// which is why retries were kept off the banners they answer.
#[must_use]
pub fn banner_height_with(
    text: &str,
    level: BannerLevel,
    action: bool,
    width: i32,
    metrics: &DisplayMetrics,
) -> i32 {
    with_text_scale(metrics.text_scale, || {
        let width = if legacy_typography() {
            width
        } else {
            width.min(metrics.readable_width())
        };
        let padding = metrics.space(Space::Small);
        let lines = wrap_text(
            text,
            width - 2 * padding - banner_mark_column(level, metrics),
            FontSize::Body,
        );
        let words = banner_height_for_lines(lines.len(), metrics);
        if action {
            words
                .saturating_add(banner_action_height(metrics))
                .saturating_add(metrics.space(Space::Tight))
        } else {
            words
        }
    })
}

fn banner_height_for_lines(lines: usize, metrics: &DisplayMetrics) -> i32 {
    i32::try_from(lines)
        .unwrap_or(i32::MAX)
        .saturating_mul(FontSize::Body.line_height())
        .saturating_add(2 * metrics.space(Space::Small))
        .max(metrics.touch_target_minimum())
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
    let edge = RowEdge {
        trailing,
        menu,
        button: None,
        badge: false,
    };
    row_title_width_at_edge_beside(metrics, area, edge, lead)
}

/// Everything that shares a row's right edge with its words.
///
/// The layout engine measures each of these before the title and gives the
/// title what is left: the overflow mark, then a row button, then a badge,
/// then a trailing value. A paginator or a clamp that leaves one of them out
/// measures a width the row will never have, so a title it counted as one
/// line is drawn on two and the last row of the page falls under the bar.
/// Describe each row the way it is built.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RowEdge<'a> {
    /// A short value against the edge, as `rows_with_trailing` sets it. Empty
    /// for none.
    pub trailing: &'a str,
    /// Whether the row carries an overflow mark.
    pub menu: bool,
    /// The label of the row's button, if it has one.
    pub button: Option<&'a str>,
    /// Whether the row carries a count. A badge keeps the same width whatever
    /// it says, so the count itself is not needed to measure it.
    pub badge: bool,
}

impl<'a> RowEdge<'a> {
    /// Nothing at the edge: the row's words have its whole width.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            trailing: "",
            menu: false,
            button: None,
            badge: false,
        }
    }

    /// The same edge, with an overflow mark.
    #[must_use]
    pub const fn with_menu(mut self) -> Self {
        self.menu = true;
        self
    }

    /// The same edge, with a button labelled `label`.
    #[must_use]
    pub const fn with_button(mut self, label: &'a str) -> Self {
        self.button = Some(label);
        self
    }

    /// The same edge, with a count badge.
    #[must_use]
    pub const fn with_badge(mut self) -> Self {
        self.badge = true;
        self
    }

    /// The same edge, with a trailing value.
    #[must_use]
    pub const fn with_trailing(mut self, value: &'a str) -> Self {
        self.trailing = value;
        self
    }
}

/// How wide a row's title really is beside everything at its right edge.
///
/// The width [`row_title_width`] gives, once a row button and a badge are
/// paid for as well, so a title clamped with it is clamped at the width it
/// is drawn at.
#[must_use]
pub fn row_title_width_at_edge(metrics: &DisplayMetrics, area: ProseArea, edge: RowEdge) -> i32 {
    row_title_width_at_edge_beside(metrics, area, edge, row_mark_column(metrics))
}

/// The same, for a list whose lead column is not a mark's. The arithmetic is
/// the `Node::Rows` arm's, in its order, and if that changes this must.
fn row_title_width_at_edge_beside(
    metrics: &DisplayMetrics,
    area: ProseArea,
    edge: RowEdge,
    lead: i32,
) -> i32 {
    let padding = metrics.space(Space::Small);
    let mut width = row_text_width_beside(metrics, area, lead);
    if edge.menu {
        width = max(1, width - metrics.touch_target_default());
    }
    if let Some(label) = edge.button {
        // The engine lets a button have a third of the row, and a list's row
        // spans the content width, which on a wide panel is more than the
        // readable measure a paginator is handed. Capped at a third of the
        // measure instead, a label that fits the row's third but not the
        // measure's was counted cut short and drawn whole, and the title
        // beside it took a line more than it was clamped to. The wider cap
        // only ever measures the title narrower than it is drawn.
        let row = max(area.width, metrics.content_width());
        let label = one_line(label, max(1, row / 3), FontSize::Body);
        let wide = measure_text(&label, FontSize::Body)
            .0
            .saturating_add(2 * padding)
            .max(metrics.touch_target_minimum());
        width = max(1, width - wide.saturating_add(padding));
    }
    if edge.badge {
        width = max(1, width - row_badge_width(metrics).saturating_add(padding));
    }
    if edge.trailing.is_empty() {
        return width;
    }
    let value = one_line(edge.trailing, width, FontSize::Caption);
    max(
        1,
        width - measure_text(&value, FontSize::Caption).0 - padding,
    )
}

/// Breaks a list of rows into pages, measuring each row with what stands at
/// its right edge.
///
/// For a list where some rows carry a button or a count and others do not,
/// or where a row carries an overflow mark as well: each row is measured at
/// the width its own title will have. Returns row indices per page, as
/// [`paginate_rows_with_trailing`] does, with one empty page for no rows.
#[must_use]
pub fn paginate_rows_with_edges(
    rows: &[(&str, &str, RowEdge)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    let lead = row_mark_column(metrics);
    let heights = rows.iter().map(|(title, summary, edge)| {
        measured_row_height_at_edge(metrics, area, title, summary, *edge, lead)
    });
    paginate_row_heights(heights, area)
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
/// How much room trailing nodes need, so flexible content can leave it.
///
/// Measured by laying them out into a layout that is thrown away, which is the
/// only measurement that cannot drift from the one that gets drawn.
#[allow(clippy::too_many_arguments)]
fn flow_node_bottom(
    node: &Node,
    following: &[Node],
    margin: i32,
    width: i32,
    bottom: i32,
    metrics: &DisplayMetrics,
    prose: Face,
    gap: i32,
) -> i32 {
    let flexible = matches!(
        node,
        Node::Text { .. }
            | Node::RichText { .. }
            | Node::PagedList { .. }
            | Node::Terminal { .. }
            | Node::Grid { square: true, .. }
            | Node::Picture { .. }
    );
    let protects_interaction = following.iter().any(node_has_enabled_interaction);
    if matches!(node, Node::Splash { .. }) || (flexible && protects_interaction) {
        bottom.saturating_sub(trailing_height(
            following, margin, width, bottom, metrics, prose, gap,
        ))
    } else {
        bottom
    }
}

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
        cursor = layout_flow_node(
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
    let edge = RowEdge {
        trailing,
        menu,
        button: None,
        badge: false,
    };
    measured_row_height_at_edge(metrics, area, title, summary, edge, lead)
}

/// The same, with a row button and a badge paid for as well.
fn measured_row_height_at_edge(
    metrics: &DisplayMetrics,
    area: ProseArea,
    title: &str,
    summary: &str,
    edge: RowEdge,
    lead: i32,
) -> i32 {
    let padding = metrics.space(Space::Small);
    let text_width = row_title_width_at_edge_beside(metrics, area, edge, lead);
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

/// Paginate rows with a cover-sized leading column, including glyph fallbacks.
#[must_use]
pub fn paginate_cover_rows(
    rows: &[(&str, &str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    paginate_rows_measured(rows, metrics, area, false, metrics.touch_target_default())
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
    let heights = rows.iter().map(|(title, summary, trailing)| {
        measured_row_height(metrics, area, title, summary, trailing, menu, lead)
    });
    paginate_row_heights(heights, area)
}

/// Packs rows of known heights into pages of `area`, returning the indices on
/// each, and one empty page for no rows.
fn paginate_row_heights(heights: impl Iterator<Item = i32>, area: ProseArea) -> Vec<Vec<usize>> {
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
    for (index, height) in heights.enumerate() {
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
    let columns = max(
        1,
        if legacy_typography() {
            metrics.grid_columns(shape)
        } else {
            metrics.grid_columns_for_width(shape, area.width)
        },
    );
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
    let lines = wrap_ranges(text, max_width, size, face);
    if lines.is_empty() {
        return vec![String::new()];
    }
    lines
        .into_iter()
        .map(|line| text[line.0..line.1].to_owned())
        .collect()
}

/// The same, for a paragraph with formulas set into it.
///
/// A page is counted before it is drawn, and until now it was counted from
/// the words a formula was written as rather than the picture drawn over
/// them. The two are different widths, so a paragraph wrapped one way and
/// drawn the other came out a line longer than the page had room for, and
/// that line was drawn over the page number at the foot of it.
#[must_use]
pub fn wrap_text_with_formulae(
    text: &str,
    max_width: i32,
    size: FontSize,
    face: Face,
    formulae: &[InlineFormula],
    line_height: i32,
) -> Vec<String> {
    let lines = wrap_ranges_with(text, max_width, size, face, formulae, line_height);
    if lines.is_empty() {
        return vec![String::new()];
    }
    lines
        .into_iter()
        .map(|line| text[line.0..line.1].to_owned())
        .collect()
}

/// The same wrapping, as byte offsets into `text` rather than copies of it.
///
/// What lets a run inside a paragraph be found on the page: a link is a range
/// of the paragraph's own bytes, and turning it into a rectangle means knowing
/// which line it landed on and how much of that line comes before it. Answering
/// from the strings alone is not possible -- wrapping drops the whitespace at
/// each break, so the lines put back together are not the paragraph.
///
/// Empty for text that wraps to nothing, which [`wrap_text_in`] turns back into
/// the single empty line every caller downstream expects.
fn wrap_ranges(text: &str, max_width: i32, size: FontSize, face: Face) -> Vec<(usize, usize)> {
    wrap_ranges_with(text, max_width, size, face, &[], 0)
}

/// The same, for a paragraph with formulas set into it.
///
/// A formula is drawn at a width of its own that has nothing to do with the
/// width of the words standing in for it, so a line that would fit as a
/// string may not fit as a line, and the other way about. Breaking on the
/// drawn width is the only way the two agree.
fn wrap_ranges_with(
    text: &str,
    max_width: i32,
    size: FontSize,
    face: Face,
    formulae: &[InlineFormula],
    line_height: i32,
) -> Vec<(usize, usize)> {
    if text.is_empty() || max_width <= 0 {
        return Vec::new();
    }
    let opportunities = with_typesetter(face, |typesetter| typesetter.line_breaks(text))
        .unwrap_or_else(|| fallback_line_breaks(text));
    let graphemes = with_typesetter(face, |typesetter| typesetter.grapheme_boundaries(text))
        .unwrap_or_else(|| {
            text.char_indices()
                .map(|(offset, character)| offset + character.len_utf8())
                .collect()
        });
    let mut lines: Vec<(usize, usize)> = Vec::new();
    let mut start = 0;
    while start < text.len() {
        start = skip_soft_whitespace(text, start);
        if start == text.len() {
            break;
        }

        let mut best: Option<(usize, usize)> = None;
        let mut emitted = false;
        for &(end, opportunity) in opportunities.iter().filter(|entry| entry.0 > start) {
            let visible_end = if opportunity == BreakOpportunity::Mandatory {
                trim_line_separator(text, start, end)
            } else {
                end
            };
            let candidate = text[start..visible_end].trim_end_matches(char::is_whitespace);
            let candidate_end = start + candidate.len();
            if measure_range_in(
                text,
                start,
                candidate_end,
                size,
                face,
                formulae,
                line_height,
            ) <= max_width
            {
                best = Some((end, candidate_end));
                if opportunity == BreakOpportunity::Mandatory {
                    lines.push((start, candidate_end));
                    start = end;
                    emitted = true;
                    break;
                }
                continue;
            }

            if let Some((best_end, line_end)) = best.take() {
                lines.push((start, line_end));
                start = best_end;
            } else {
                let forced_end =
                    force_grapheme_break(text, start, visible_end, max_width, size, &graphemes);
                lines.push((start, start + text[start..forced_end].trim_end().len()));
                start = forced_end;
            }
            emitted = true;
            break;
        }

        if !emitted {
            let forced_end =
                force_grapheme_break(text, start, text.len(), max_width, size, &graphemes);
            lines.push((start, start + text[start..forced_end].trim_end().len()));
            start = forced_end;
        }
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
/// Corner radius of a command-deck pad. Larger than a keyboard key so a
/// fifteen-key grid reads as recessed hardware rather than a ruled board.
pub const PAD_RADIUS_TENTH_MM: i32 = 28;
/// Bezel of a command-deck pad, in tenths of a millimetre. Heavier than a
/// rule so the key sits in a dark frame the way a Stream Deck key does.
pub const PAD_BORDER_TENTH_MM: i32 = 12;

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

/// The luminance of one colour, by the weights broadcast television settled
/// on for the same purpose: turning a colour picture into the grey one a
/// monochrome set would show.
///
/// Used wherever a colour pixel needs the single grey value the rest of the
/// pipeline reasons about: the planner's grey test, the greyscale panels, the
/// simulator's monochrome preview. Integer throughout; the weights sum to one
/// thousand so a grey colour comes back as exactly itself.
#[must_use]
pub const fn luma([red, green, blue]: [u8; 3]) -> u8 {
    let weighted = 299 * red as u32 + 587 * green as u32 + 114 * blue as u32;
    // Rounded, not truncated, so that (v, v, v) gives v for every v.
    ((weighted + 500) / 1000) as u8
}

/// A rendered frame: one grey byte per pixel, and, once anything has been
/// drawn in colour, three colour bytes per pixel beside it.
///
/// `pixels` is the frame as every greyscale panel and every existing caller
/// sees it, and it is always complete: a colour draw writes the luminance
/// here and the colour into `chroma`. `chroma` is absent until the first
/// colour draw and dropped again by [`Surface::clear`], so a frame with no
/// colour in it costs exactly what it did before colour existed, and a
/// runtime on a greyscale panel can ignore the plane altogether.
///
/// When `chroma` is present every grey write also lands in it as three
/// equal bytes, so the two planes describe one picture. Code that writes
/// `pixels` directly, as tests do, keeps that promise only while `chroma` is
/// `None`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Surface {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub chroma: Option<Vec<u8>>,
}

impl Surface {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![tone::PAPER; width.saturating_mul(height)],
            chroma: None,
        }
    }

    pub fn clear(&mut self, value: u8) {
        self.pixels.fill(value);
        self.chroma = None;
    }

    /// The colour of one pixel, whether or not the frame holds any colour.
    #[must_use]
    pub fn rgb_at(&self, index: usize) -> Option<[u8; 3]> {
        match &self.chroma {
            Some(chroma) => chroma
                .get(index * 3..index * 3 + 3)
                .map(|c| [c[0], c[1], c[2]]),
            None => self.pixels.get(index).map(|grey| [*grey; 3]),
        }
    }

    /// Whether any pixel of the frame is a colour rather than a grey.
    ///
    /// Cheap when no colour has been drawn; a scan of the colour plane when
    /// one exists, because a plane that was needed for one picture may since
    /// have been painted over in grey.
    #[must_use]
    pub fn has_colour(&self) -> bool {
        self.chroma
            .as_ref()
            .is_some_and(|chroma| chroma.chunks_exact(3).any(|c| c[0] != c[1] || c[1] != c[2]))
    }

    /// Whether any pixel inside `region` is a colour rather than a grey.
    #[must_use]
    pub fn region_has_colour(&self, region: Rect) -> bool {
        let Some(chroma) = &self.chroma else {
            return false;
        };
        let Some((left, top, width, height)) = region_extent(region) else {
            return false;
        };
        (top..top.saturating_add(height)).any(|y| {
            let start = y.saturating_mul(self.width).saturating_add(left) * 3;
            let end = start.saturating_add(width * 3);
            chroma
                .get(start..end)
                .unwrap_or(&[])
                .chunks_exact(3)
                .any(|c| c[0] != c[1] || c[1] != c[2])
        })
    }

    /// The colour bytes of `region`, one row at a time with three bytes per
    /// pixel, when the frame holds colour. The rows are borrowed from the
    /// frame rather than copied, so a caller writing them elsewhere pays for
    /// one buffer, not two. `None` for a frame that has no colour, or a
    /// region that is not inside it: the caller then has exactly the grey
    /// path it had.
    #[must_use]
    pub fn colour_rows(&self, region: Rect) -> Option<impl Iterator<Item = &[u8]> + '_> {
        let chroma = self.chroma.as_ref()?;
        let (left, top, width, height) = region_extent(region)?;
        if left.checked_add(width)? > self.width || top.checked_add(height)? > self.height {
            return None;
        }
        let stride = self.width * 3;
        Some((top..top + height).map(move |y| {
            let start = y * stride + left * 3;
            &chroma[start..start + width * 3]
        }))
    }

    /// The colour plane, brought into being from the grey one on first use.
    fn chroma_mut(&mut self) -> &mut Vec<u8> {
        if self.chroma.is_none() {
            let mut plane = Vec::with_capacity(self.pixels.len() * 3);
            for grey in &self.pixels {
                plane.extend_from_slice(&[*grey; 3]);
            }
            self.chroma = Some(plane);
        }
        self.chroma.as_mut().expect("just created")
    }

    /// Writes one grey value to a pixel of both planes.
    fn set_grey(&mut self, index: usize, value: u8) {
        if let Some(pixel) = self.pixels.get_mut(index) {
            *pixel = value;
        }
        if let Some(chroma) = &mut self.chroma {
            if let Some(colour) = chroma.get_mut(index * 3..index * 3 + 3) {
                colour.fill(value);
            }
        }
    }

    /// Turns one pixel of both planes to its opposite.
    fn invert_at(&mut self, index: usize) {
        if let Some(pixel) = self.pixels.get_mut(index) {
            *pixel = u8::MAX - *pixel;
        }
        if let Some(chroma) = &mut self.chroma {
            if let Some(colour) = chroma.get_mut(index * 3..index * 3 + 3) {
                for channel in colour {
                    *channel = u8::MAX - *channel;
                }
            }
        }
    }

    /// [`Self::fill_rect`] in a [`Paint`]: a grey lands in both planes as it
    /// always has, and a colour brings the colour plane into being and keeps
    /// the grey plane at the colour's luminance, as [`Self::blend_colour`]
    /// does for one pixel.
    pub fn fill_rect_paint(&mut self, rect: Rect, paint: Paint) {
        match paint {
            Paint::Grey(value) => self.fill_rect(rect, value),
            Paint::Colour(colour) => {
                let bounds = Rect {
                    x: 0,
                    y: 0,
                    width: i32::try_from(self.width).unwrap_or(i32::MAX),
                    height: i32::try_from(self.height).unwrap_or(i32::MAX),
                };
                let Some(clipped) = rect.intersection(bounds) else {
                    return;
                };
                let grey = luma(colour);
                let width = self.width;
                let chroma = self.chroma_mut();
                for y in clipped.y..clipped.y + clipped.height {
                    let row = usize::try_from(y).unwrap_or(0).saturating_mul(width);
                    for x in clipped.x..clipped.x + clipped.width {
                        let index = row.saturating_add(usize::try_from(x).unwrap_or(0));
                        if let Some(target) = chroma.get_mut(index * 3..index * 3 + 3) {
                            target.copy_from_slice(&colour);
                        }
                    }
                }
                for y in clipped.y..clipped.y + clipped.height {
                    let row = usize::try_from(y).unwrap_or(0).saturating_mul(width);
                    for x in clipped.x..clipped.x + clipped.width {
                        let index = row.saturating_add(usize::try_from(x).unwrap_or(0));
                        if let Some(pixel) = self.pixels.get_mut(index) {
                            *pixel = grey;
                        }
                    }
                }
            }
        }
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
                    self.set_grey(index, value);
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
                    self.invert_at(index);
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
                self.invert_at(index);
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
        let Some(index) = self.index_of(x, y) else {
            return;
        };
        if let Some(pixel) = self.pixels.get_mut(index) {
            *pixel = mix(*pixel, value, coverage);
        }
        if let Some(chroma) = &mut self.chroma {
            if let Some(colour) = chroma.get_mut(index * 3..index * 3 + 3) {
                for channel in colour {
                    *channel = mix(*channel, value, coverage);
                }
            }
        }
    }

    /// [`Self::blend`] for a colour: mixes each channel by `coverage` and
    /// keeps the grey plane at the result's luminance.
    ///
    /// This is the only way colour gets into a frame, which is what lets the
    /// grey plane stay authoritative for everything that is not a picture.
    /// The colour plane comes into being on the first call.
    pub fn blend_colour(&mut self, x: i32, y: i32, colour: [u8; 3], coverage: u8) {
        let Some(index) = self.index_of(x, y) else {
            return;
        };
        if index >= self.pixels.len() {
            return;
        }
        let chroma = self.chroma_mut();
        let Some(target) = chroma.get_mut(index * 3..index * 3 + 3) else {
            return;
        };
        for (channel, ink) in target.iter_mut().zip(colour) {
            *channel = mix(*channel, ink, coverage);
        }
        let mixed = [target[0], target[1], target[2]];
        if let Some(pixel) = self.pixels.get_mut(index) {
            *pixel = luma(mixed);
        }
    }

    fn index_of(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }
        let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
            return None;
        };
        if x >= self.width {
            return None;
        }
        y.checked_mul(self.width).and_then(|row| row.checked_add(x))
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

/// Mixes `ink` into `destination` by `coverage`, where 0 leaves it untouched
/// and 255 replaces it.
fn mix(destination: u8, ink: u8, coverage: u8) -> u8 {
    let destination = i32::from(destination);
    let mixed = destination + (i32::from(ink) - destination) * i32::from(coverage) / 255;
    u8::try_from(mixed.clamp(0, 255)).unwrap_or(u8::MAX)
}

/// A rectangle as unsigned left, top, width and height, or `None` when any
/// side is negative and so cannot index a frame.
fn region_extent(region: Rect) -> Option<(usize, usize, usize, usize)> {
    Some((
        usize::try_from(region.x).ok()?,
        usize::try_from(region.y).ok()?,
        usize::try_from(region.width).ok()?,
        usize::try_from(region.height).ok()?,
    ))
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
    /// Sixteen-level refresh of a region holding colour, written in colour.
    ///
    /// Chosen whenever the changed region has a pixel whose channels differ.
    /// The runtime writes that region's colour plane to the framebuffer and
    /// asks the controller to drive the colour filter; on a panel without one
    /// the runtime writes grey and this is a quality update. Partial unless
    /// it is also the cleaning refresh, when it covers the whole panel.
    Colour,
}

impl PanelWaveform {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Du => "DU",
            Self::Gl16 => "GL16",
            Self::Gc16 => "GC16",
            Self::Colour => "COLOUR",
        }
    }

    /// Whether the runtime should write the region's colour plane rather
    /// than its grey one.
    #[must_use]
    pub const fn writes_colour(self) -> bool {
        matches!(self, Self::Colour)
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
    /// The colour plane of the last committed frame, kept only while a frame
    /// with colour has been shown; `None` means every previous pixel was
    /// the grey in `previous`.
    previous_chroma: Option<Vec<u8>>,
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
            previous_chroma: None,
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
        // A cleaning refresh repaints the whole panel, and a whole panel with
        // a colour picture on it has to be repainted in colour or the picture
        // comes back grey.
        let clean = || {
            if surface.has_colour() {
                (whole, PanelWaveform::Colour, 0, true)
            } else {
                (whole, PanelWaveform::Gc16, 0, true)
            }
        };
        let (region, waveform, dirty, full) = if self.started {
            let (changed, flipped) = self.changed(surface)?;
            // The budget is checked before this update is added to it, so that
            // a full panel's worth of repainting still buys exactly
            // PANEL_CLEAN_INTERVAL updates before anything flashes, as it did
            // when updates rather than pixels were being counted.
            if self.dirty >= self.clean_after() {
                clean()
            } else if surface.region_has_colour(changed) {
                (
                    changed,
                    PanelWaveform::Colour,
                    self.dirty.saturating_add(flipped),
                    false,
                )
            } else if Self::has_grey(surface, changed) {
                (
                    changed,
                    PanelWaveform::Gl16,
                    self.dirty.saturating_add(flipped),
                    false,
                )
            } else {
                (
                    changed,
                    PanelWaveform::Du,
                    self.dirty.saturating_add(flipped),
                    false,
                )
            }
        } else {
            clean()
        };
        Some(FrameTransition {
            region,
            waveform,
            full,
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
        // Kept only while there is colour to remember: a frame that has gone
        // back to grey is fully described by `previous`, and holding a plane
        // for it would make every later comparison three times the work.
        self.previous_chroma = surface
            .has_colour()
            .then(|| surface.chroma.clone())
            .flatten();
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
        // A pixel has changed when its grey has, or when either frame holds
        // colour for it and the colour has. Two greys that match cannot hide
        // a colour change when neither side has a plane, which is the common
        // case and stays the single comparison it always was.
        let colour_differs = |index: usize| -> bool {
            if surface.chroma.is_none() && self.previous_chroma.is_none() {
                return false;
            }
            let current = surface.rgb_at(index);
            let previous = match &self.previous_chroma {
                Some(chroma) => chroma
                    .get(index * 3..index * 3 + 3)
                    .map(|c| [c[0], c[1], c[2]]),
                None => self.previous.get(index).map(|grey| [*grey; 3]),
            };
            current != previous
        };
        for (index, _) in surface
            .pixels
            .iter()
            .zip(self.previous.iter())
            .enumerate()
            .filter(|(index, (current, previous))| current != previous || colour_differs(*index))
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

/// How the bytes of a picture are laid out.
///
/// Grey is what every application has always sent and what every panel can
/// show. Colour is three bytes per pixel, red then green then blue, and is
/// only worth sending when the runtime has said the panel can show it; on any
/// other panel it is drawn as its luminance, which costs three times the
/// transfer for the same picture.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PictureFormat {
    /// One byte per pixel.
    Grey,
    /// Three bytes per pixel, red, green, blue.
    Rgb,
}

impl PictureFormat {
    #[must_use]
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Grey => 1,
            Self::Rgb => 3,
        }
    }

    /// The byte count of a `width` by `height` picture in this format, or
    /// `None` when it does not fit in memory arithmetic.
    #[must_use]
    pub fn byte_len(self, width: u32, height: u32) -> Option<usize> {
        usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(self.bytes_per_pixel())
    }
}

/// Eight-bit grey pixels, row major, `width * height` of them, and the same
/// pixels in colour when the picture arrived that way.
///
/// `grey` is always present so that nothing drawing a picture has to know
/// about colour; `colour` holds three bytes per pixel in the same order and is
/// what a colour panel draws instead.
#[derive(Clone, Copy, Debug)]
pub struct PicturePixels<'a> {
    pub width: u32,
    pub height: u32,
    pub grey: &'a [u8],
    pub colour: Option<&'a [u8]>,
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
    let _environment = environment::TextEnvironment::enter(screen, metrics);
    with_reading_font(screen.reading_font, || {
        diagnose_screen_with_selected_font(screen, metrics, chrome, pictures)
    })
}

fn diagnose_screen_with_selected_font(
    screen: &Screen,
    metrics: &DisplayMetrics,
    chrome: &Chrome,
    pictures: Option<&dyn Pictures>,
) -> LayoutDiagnostics {
    let layout = screen.layout_with_selected_font(metrics, chrome);
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
        validate_node(node, metrics, layout.prose_face, pictures, &mut issues);
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
            | LayoutKind::TableHeaderCell
            | LayoutKind::FactLabel
            | LayoutKind::PagePosition
            | LayoutKind::ActivityBytes
            | LayoutKind::ActivityFailure
            | LayoutKind::FieldNote(false) => muted = true,
            LayoutKind::Card
            | LayoutKind::Tile(..)
            | LayoutKind::Overlay
            | LayoutKind::SelectionBar => surface = true,
            LayoutKind::Toggle(..) => {}
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
    prose_face: Face,
    pictures: Option<&dyn Pictures>,
    issues: &mut Vec<LayoutIssue>,
) {
    let id = node.id();
    match node {
        Node::Text { text, .. } | Node::RichText { text, .. } | Node::Quote { text, .. } => {
            check_text_coverage(id, text, prose_face, issues);
        }
        Node::Heading { text, .. } | Node::Secondary { text, .. } => {
            check_text_coverage(id, text, Face::Text, issues);
        }
        Node::Banner { text, action, .. } => {
            check_text_coverage(id, text, Face::Text, issues);
            if let Some(action) = action {
                check_text_coverage(id, &action.label, Face::Text, issues);
            }
        }
        Node::Section {
            title, value, link, ..
        } => {
            check_text_coverage(id, title, Face::Text, issues);
            if let Some(value) = value {
                check_text_coverage(id, value, Face::Text, issues);
            }
            if let Some(link) = link {
                check_text_coverage(id, &link.label, Face::Text, issues);
            }
        }
        Node::Button { label, .. } => check_text_coverage(id, label, Face::Text, issues),
        Node::Field {
            value,
            placeholder,
            label,
            hint,
            error,
            kind,
            ..
        } => {
            // A secret is drawn as dots, so what it holds never has to be
            // drawable, and is never named in a report either.
            if *kind != FieldKind::Secret {
                check_text_coverage(id, value, Face::Text, issues);
            }
            check_text_coverage(id, placeholder, Face::Text, issues);
            for text in [label, hint, error].into_iter().flatten() {
                check_text_coverage(id, text, Face::Text, issues);
            }
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
        Node::PageRail { .. } => {}
        Node::PagedList { items, .. } => {
            for item in items {
                check_text_coverage(id, item, Face::Text, issues);
            }
        }
        Node::PencilBoard { board, .. } => {
            if !board.is_valid() {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::InvalidBoard,
                    rect: None,
                });
            }
        }
        Node::Board { surface, .. } => {
            if !surface.is_valid() {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::InvalidBoard,
                    rect: None,
                });
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
        Node::Table { rows, .. } => {
            if rows.len() > MAX_TABLE_ROWS {
                issues.push(limit_issue(id, "table rows", rows.len(), MAX_TABLE_ROWS));
            }
            for row in rows.iter().take(MAX_TABLE_ROWS) {
                if row.cells.len() > MAX_TABLE_COLUMNS {
                    issues.push(limit_issue(
                        id,
                        "table columns",
                        row.cells.len(),
                        MAX_TABLE_COLUMNS,
                    ));
                }
                for cell in row.cells.iter().take(MAX_TABLE_COLUMNS) {
                    check_text_coverage(id, cell, Face::Text, issues);
                }
            }
        }
        Node::Rows { rows, .. } => {
            if rows.len() > MAX_ROWS {
                issues.push(limit_issue(id, "rows", rows.len(), MAX_ROWS));
            }
            for row in rows {
                check_text_coverage(id, &row.title, prose_face, issues);
                check_text_coverage(id, &row.summary, Face::Text, issues);
                if let Some(button) = &row.button {
                    check_text_coverage(id, &button.label, Face::Text, issues);
                }
            }
        }
        Node::Toggle { label, summary, .. } => {
            check_text_coverage(id, label, Face::Text, issues);
            check_text_coverage(id, summary, Face::Text, issues);
        }
        Node::SelectionBar { actions, .. } => {
            if actions.len() > MAX_SELECTION_ACTIONS {
                issues.push(limit_issue(
                    id,
                    "selection actions",
                    actions.len(),
                    MAX_SELECTION_ACTIONS,
                ));
            }
            for action in actions.iter().take(MAX_SELECTION_ACTIONS) {
                check_text_coverage(id, &action.label, Face::Text, issues);
            }
        }
        Node::Options {
            prompt, options, ..
        } => {
            check_text_coverage(id, prompt, Face::Text, issues);
            if options.is_empty() {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::EmptyChoice,
                    rect: None,
                });
            }
            if options.len() > MAX_OPTIONS {
                issues.push(limit_issue(id, "options", options.len(), MAX_OPTIONS));
            }
            for option in options.iter().take(MAX_OPTIONS) {
                check_text_coverage(id, &option.label, Face::Text, issues);
                check_text_coverage(id, &option.summary, Face::Text, issues);
            }
        }
        Node::Code {
            payload, caption, ..
        } => {
            check_text_coverage(id, caption, Face::Text, issues);
            // A code that cannot be drawn is an empty square where the reader
            // was told to point a phone, so it is an error, not a warning.
            // Judged by length rather than by encoding it, so an application
            // checking its own screens never links the encoder: every payload
            // within the limit fits version 10 even in byte mode.
            if payload.is_empty() || payload.len() > MAX_CODE_BYTES {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::CodeUnreadable,
                    rect: None,
                });
            }
        }
        Node::Steps { total, labels, .. } => {
            if usize::from(*total) > MAX_STEPS {
                issues.push(limit_issue(id, "steps", usize::from(*total), MAX_STEPS));
            }
            for label in labels.iter().take(MAX_STEPS) {
                check_text_coverage(id, label, Face::Text, issues);
            }
        }
        Node::TileGrid { tiles, .. } => {
            for tile in tiles {
                check_text_coverage(id, &tile.label, Face::Text, issues);
                check_text_coverage(id, &tile.subtitle, Face::Text, issues);
                check_text_coverage(id, &tile.value, Face::Text, issues);
                if let (Some(pictures), Some(picture)) = (pictures, tile.picture) {
                    check_picture(id, picture.handle, picture.source, pictures, issues);
                }
            }
        }
        Node::Stepper { label, .. } => {
            check_text_coverage(id, label, Face::Text, issues);
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
                let columns = UnicodeWidthStr::width(row.as_str());
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
    let panel = Rect {
        x: 0,
        y: 0,
        width: metrics.width,
        height: metrics.height,
    };
    for node in nodes {
        let id = node.id();
        // A picture that is the page is outside the content box on purpose,
        // so it answers to the panel instead. Off the panel is still off the
        // panel, which is the thing this check exists to catch.
        let allowed = if matches!(node, Node::Picture { bleed: true, .. }) {
            panel
        } else {
            layout.content
        };
        let laid_out = layout
            .nodes
            .iter()
            .filter(|laid_out| laid_out.id == id)
            .collect::<Vec<_>>();
        let rects = laid_out
            .iter()
            .map(|laid_out| laid_out.rect)
            .collect::<Vec<_>>();
        let offscreen_interactive = laid_out.iter().find(|laid_out| {
            is_enabled_interactive(laid_out.kind) && !rect_is_inside(laid_out.rect, layout.content)
        });
        let expected_interactions = node_enabled_interaction_count(node);
        let visible_interactions = laid_out
            .iter()
            .filter(|laid_out| is_enabled_interactive(laid_out.kind))
            .count();
        // A flex draws nothing by design: it moves the cursor and leaves. So
        // does an empty list. Neither is content that layout hid.
        let expects_rect = !matches!(node, Node::Rows { rows, .. } if rows.is_empty())
            && !matches!(node, Node::Grid { cells, .. } if cells.is_empty())
            && !matches!(node, Node::Terminal { rows, .. } if rows.is_empty())
            && !matches!(node, Node::Flex { .. });
        let completely_hidden = expects_rect
            && (rects.is_empty()
                || rects
                    .iter()
                    .all(|rect| rect.intersection(layout.content).is_none()));
        if completely_hidden {
            hidden.push(id);
            if expected_interactions > 0 {
                issues.push(LayoutIssue {
                    severity: DiagnosticSeverity::Error,
                    node: Some(id),
                    kind: LayoutIssueKind::InteractiveOffscreen,
                    rect: None,
                });
            }
        } else if rects.iter().any(|rect| !rect_is_inside(*rect, allowed)) && !clipped.contains(&id)
        {
            clipped.push(id);
            let rect = rects
                .iter()
                .copied()
                .find(|rect| !rect_is_inside(*rect, allowed));
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(id),
                kind: LayoutIssueKind::Clipped,
                rect,
            });
        }
        if !completely_hidden {
            if let Node::Text { text, .. } = node {
                if let Some(text_layout) = laid_out
                    .iter()
                    .find(|laid_out| laid_out.kind == LayoutKind::Text)
                {
                    let required = wrap_ranges(
                        text,
                        text_layout.rect.width,
                        FontSize::Body,
                        layout.prose_face,
                    )
                    .len();
                    if required > text_layout.text_lines.len() {
                        issues.push(LayoutIssue {
                            severity: DiagnosticSeverity::Error,
                            node: Some(id),
                            kind: LayoutIssueKind::TextOverflow,
                            rect: Some(text_layout.rect),
                        });
                    }
                }
            } else if let Node::RichText {
                text,
                presentation,
                formulae,
                ..
            } = node
            {
                if let Some(text_layout) = laid_out.iter().find(|laid_out| {
                    matches!(laid_out.kind, LayoutKind::RichText(_))
                        && laid_out.text_lines.is_empty()
                }) {
                    let (line_height, before, after, indent) =
                        rich_text_spacing(*presentation, layout.prose_face);
                    let measure = text_layout.rect.width.saturating_sub(indent.max(0)).max(1);
                    let required = wrap_ranges_with(
                        text,
                        measure,
                        FontSize::Body,
                        layout.prose_face,
                        formulae,
                        line_height,
                    )
                    .len();
                    let rendered = usize::try_from(
                        text_layout
                            .rect
                            .height
                            .saturating_sub(before)
                            .saturating_sub(after)
                            / line_height.max(1),
                    )
                    .unwrap_or(0);
                    if required > rendered {
                        issues.push(LayoutIssue {
                            severity: DiagnosticSeverity::Error,
                            node: Some(id),
                            kind: LayoutIssueKind::TextOverflow,
                            rect: Some(text_layout.rect),
                        });
                    }
                }
            }
        }
        if let Some(offscreen) = offscreen_interactive {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(id),
                kind: LayoutIssueKind::InteractiveOffscreen,
                rect: Some(offscreen.rect),
            });
        } else if !completely_hidden && visible_interactions < expected_interactions {
            issues.push(LayoutIssue {
                severity: DiagnosticSeverity::Error,
                node: Some(id),
                kind: LayoutIssueKind::InteractiveOffscreen,
                rect: None,
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

fn node_has_enabled_interaction(node: &Node) -> bool {
    match node {
        Node::Card { children, .. } => children.iter().any(node_has_enabled_interaction),
        Node::Band { slots, .. } => slots
            .iter()
            .flat_map(|slot| &slot.nodes)
            .any(node_has_enabled_interaction),
        _ => node_enabled_interaction_count(node) > 0,
    }
}

fn node_enabled_interaction_count(node: &Node) -> usize {
    match node {
        Node::Text { links, .. } | Node::RichText { links, .. } => links.len(),
        Node::Section { link, .. } => usize::from(link.is_some()),
        Node::Quote { fold, .. } => usize::from(fold.is_some()),
        Node::Button { state, .. } => usize::from(state.is_enabled()),
        Node::Field { clear, .. } => 1 + usize::from(clear.is_some()),
        Node::Chips { chips, .. } | Node::Tabs { tabs: chips, .. } => chips.len(),
        Node::Card { .. } | Node::Band { .. } => 0,
        Node::Grid { cells, square, .. } => {
            let crossword = *square && cells.iter().any(|cell| cell.corner.is_some());
            cells
                .iter()
                .filter(|cell| !crossword || cell.label != "#")
                .count()
        }
        Node::PencilBoard { board, .. } => board.action_count(),
        Node::Board { surface, .. } => {
            surface.cells.len() + surface.row_clues.len() + surface.column_clues.len()
        }
        Node::Rows { rows, .. } => rows
            .iter()
            .map(|row| 1 + usize::from(row.menu.is_some()) + usize::from(row.button.is_some()))
            .sum(),
        Node::Toggle { state, .. } => usize::from(state.is_enabled()),
        Node::SelectionBar { actions, .. } => actions.len().min(MAX_SELECTION_ACTIONS),
        // At least the one row the page is sure to show. How many more, and
        // whether the pager draws a chevron, depends on the panel, which this
        // count is not given.
        Node::Options { options, .. } => usize::from(!options.is_empty()),
        Node::Banner { action, .. } => usize::from(action.is_some()),
        Node::TileGrid { tiles, .. } => tiles
            .iter()
            .filter(|tile| tile.state != TileState::Unavailable)
            .count(),
        Node::Stepper {
            less_state,
            more_state,
            ..
        } => usize::from(less_state.is_enabled()) + usize::from(more_state.is_enabled()),
        Node::Choice {
            options, freeform, ..
        } => options.len() + usize::from(freeform.is_some()),
        Node::Activity { cancel, .. } => usize::from(cancel.is_some()),
        Node::Heading { .. }
        | Node::Secondary { .. }
        | Node::Facts { .. }
        | Node::Divider { .. }
        | Node::Spacer { .. }
        | Node::Flex { .. }
        | Node::Progress { .. }
        | Node::PagedList { .. }
        | Node::Table { .. }
        | Node::PageRail { .. }
        | Node::Splash { .. }
        | Node::Skeleton { .. }
        | Node::Picture { .. }
        | Node::Terminal { .. }
        | Node::Code { .. }
        | Node::Steps { .. } => 0,
    }
}

const fn is_enabled_interactive(kind: LayoutKind) -> bool {
    kind.acts_on().is_some()
        && !matches!(
            kind,
            LayoutKind::Button(_, ControlState::Disabled, _)
                | LayoutKind::Tile(_, ControlState::Disabled)
                | LayoutKind::StepperControl(_, ControlState::Disabled, _)
                | LayoutKind::Toggle(_, ControlState::Disabled)
        )
}

const fn interaction_uses_panel_bounds(kind: LayoutKind) -> bool {
    matches!(
        kind,
        LayoutKind::Back
            | LayoutKind::BarAction(_)
            | LayoutKind::BarGlyph(..)
            | LayoutKind::NavDestination(..)
            | LayoutKind::NavDestinationSelected(..)
            | LayoutKind::PagePrevious(_)
            | LayoutKind::PageNext(_)
    )
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
        let style = match node.kind {
            LayoutKind::Quote(_, role) => Some((role.size(), layout.prose_face)),
            _ => layout_text_style(node),
        };
        let Some((size, face)) = style else {
            continue;
        };
        let scale = if matches!(node.kind, LayoutKind::PencilNumber(_)) {
            pencil::label_style(node).1
        } else if matches!(node.kind, LayoutKind::CellLabel(true)) {
            board_label_style(node).1
        } else if matches!(node.kind, LayoutKind::CellLabel(false)) {
            key_label_style(node).1
        } else {
            text_scale()
        };
        let (too_wide, too_tall) = with_text_scale(scale, || {
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
            // A lone pencil number is its glyph, not its leading: the box may
            // clip a hair of line height the same way it clips a hairline.
            let leading_allowance = if rows == 1 && matches!(node.kind, LayoutKind::PencilNumber(_))
            {
                metrics.rule_thickness() * 2
            } else {
                0
            };
            let too_tall = rows.saturating_mul(size.line_height_in(face))
                > node.rect.height + leading_allowance;
            (too_wide, too_tall)
        });
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

/// Which kinds are held to the minimum a finger can reliably hit.
///
/// Controls, in other words: something a designer chose the size of, and could
/// have made larger. [`LayoutKind::InlineLink`] is deliberately absent. Its size
/// is the size of the words the author wrote, which nobody here chose and
/// nothing here can change, and reporting every footnote marker in an annotated
/// edition as a layout error would say only that books have short words in
/// them.
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
            | LayoutKind::Field(..)
            | LayoutKind::FieldClear(_)
            | LayoutKind::Chip(_, _)
            | LayoutKind::Tab(_, _)
            | LayoutKind::ChoiceOption(_, _)
            | LayoutKind::StepperControl(_, ControlState::Enabled, _)
            | LayoutKind::ChoiceFreeform(_)
            | LayoutKind::PagePrevious(_)
            | LayoutKind::PageNext(_)
            | LayoutKind::Toggle(_, ControlState::Enabled)
            | LayoutKind::RowButton(_)
            | LayoutKind::BannerAction(_)
            | LayoutKind::SelectionAction(_)
            | LayoutKind::OptionRow(..)
    )
}

// Board marks fit the square; enlarging interface text must not clip a digit.
// The renderer and diagnostics share this choice. Ordinary key labels retain
// their body size, so an undersized application control still reports a fault.
fn board_label_style(node: &LayoutNode) -> (FontSize, TextScale) {
    let scale = text_scale();
    let short = node
        .text_lines
        .first()
        .is_some_and(|text| text.chars().count() <= 2);
    let fits = |size: FontSize| {
        size.line_height() * i32::try_from(node.text_lines.len()).unwrap_or(i32::MAX)
            <= node.rect.height
            && node
                .text_lines
                .iter()
                .all(|line| measure_text(line, size).0 <= node.rect.width)
    };
    if let Some(size) = [FontSize::Heading, FontSize::Body, FontSize::Caption]
        .into_iter()
        .filter(|size| short || *size != FontSize::Heading)
        .find(|size| fits(*size))
    {
        return (size, scale);
    }
    // A digit is a board mark, not prose. At the smallest legal square the
    // largest caption can still be too tall. Short marks may step down the
    // scale to fit; long app labels remain errors instead of tiny text.
    if node
        .text_lines
        .first()
        .is_some_and(|text| text.chars().count() <= 3)
    {
        for candidate in TextScale::STEPS
            .into_iter()
            .take_while(|candidate| *candidate != scale)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if with_text_scale(candidate, || fits(FontSize::Caption)) {
                return (FontSize::Caption, candidate);
            }
        }
    }
    (FontSize::Caption, scale)
}
fn board_label_size(node: &LayoutNode) -> FontSize {
    board_label_style(node).0
}

// Keyboard keys keep physical targets. Fit their short labels within those targets,
// including the compact landscape keyboard at the largest reader text settings.
fn key_label_style(node: &LayoutNode) -> (FontSize, TextScale) {
    let current = text_scale();
    let fits = |size: FontSize| {
        let pad = 2;
        size.line_height() + pad <= node.rect.height
            && node
                .text_lines
                .iter()
                .all(|line| measure_text(line, size).0 + pad <= node.rect.width)
    };
    for size in [FontSize::Body, FontSize::Caption] {
        if fits(size) {
            return (size, current);
        }
    }
    for scale in TextScale::STEPS
        .into_iter()
        .take_while(|s| *s != current)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        if with_text_scale(scale, || fits(FontSize::Caption)) {
            return (FontSize::Caption, scale);
        }
    }
    (FontSize::Caption, current)
}

fn layout_text_style(node: &LayoutNode) -> Option<(FontSize, Face)> {
    let size = match node.kind {
        LayoutKind::Heading(level) => FontSize::for_heading_level(level),
        LayoutKind::CellLabel(true) => board_label_size(node),
        LayoutKind::PencilNumber(_) => pencil::label_style(node).0,
        LayoutKind::CellLabel(false) => key_label_style(node).0,
        LayoutKind::TopBarTitle => BAR_TITLE,
        LayoutKind::BarAction(_) => {
            if FontSize::Body.line_height() <= node.rect.height {
                FontSize::Body
            } else {
                FontSize::Caption
            }
        }
        LayoutKind::OverlayTitle => FontSize::Title,
        LayoutKind::BoardClue
        | LayoutKind::Secondary
        | LayoutKind::Section
        | LayoutKind::TableHeaderCell
        | LayoutKind::FactLabel
        | LayoutKind::RowTrailing
        | LayoutKind::RowSummary
        | LayoutKind::TileLabel
        | LayoutKind::TileLabelMuted
        | LayoutKind::TileSubtitle
        | LayoutKind::TileBadge
        | LayoutKind::Chip(_, _)
        | LayoutKind::Tab(_, _)
        | LayoutKind::PagePosition
        | LayoutKind::ActivityBytes
        | LayoutKind::ActivityFailure
        | LayoutKind::NavDestination(..)
        | LayoutKind::NavDestinationSelected(..)
        | LayoutKind::RowBadge
        | LayoutKind::FieldLabel
        | LayoutKind::FieldNote(_)
        | LayoutKind::CodeCaption
        | LayoutKind::Steps(..) => FontSize::Caption,
        LayoutKind::Text
        | LayoutKind::FieldValue(_)
        | LayoutKind::TableCell
        | LayoutKind::FactValue
        | LayoutKind::Quote(..)
        | LayoutKind::Button(..)
        | LayoutKind::PagedList
        | LayoutKind::RowTitle
        | LayoutKind::RowTitleDone
        | LayoutKind::ChoicePrompt
        | LayoutKind::ChoiceOption(_, _)
        | LayoutKind::StepperValue
        | LayoutKind::ChoiceFreeform(_)
        | LayoutKind::Banner(_)
        | LayoutKind::ActivityLabel
        | LayoutKind::ToggleLabel(_)
        | LayoutKind::RowButton(_)
        | LayoutKind::BannerAction(_)
        | LayoutKind::SelectionBar
        | LayoutKind::SelectionAction(_)
        | LayoutKind::OptionLabel => FontSize::Body,
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
    /// The luminance of every pixel, derived on arrival for a colour picture
    /// so that drawing never has to convert.
    grey: Vec<u8>,
    /// Three bytes per pixel when the picture arrived in colour.
    colour: Option<Vec<u8>>,
    used: std::cell::Cell<u64>,
}

impl HeldPicture {
    /// What the budget is charged for this picture: both planes.
    fn bytes(&self) -> usize {
        self.grey.len() + self.colour.as_ref().map_or(0, Vec::len)
    }
}

struct PendingPicture {
    handle: PictureHandle,
    width: u32,
    height: u32,
    format: PictureFormat,
    expected: usize,
    bytes: Vec<u8>,
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
        self.put_report_with(handle, width, height, PictureFormat::Grey, grey)
    }

    /// [`Self::put_report`] for a picture in any format.
    ///
    /// A colour picture is charged for both the colour bytes and the grey
    /// plane derived from them, because both are held: the grey so that a
    /// greyscale panel draws without converting, the colour so that a colour
    /// panel draws what was sent.
    pub fn put_report_with(
        &mut self,
        handle: PictureHandle,
        width: u32,
        height: u32,
        format: PictureFormat,
        bytes: Vec<u8>,
    ) -> Option<Vec<PictureHandle>> {
        let expected = format.byte_len(width, height)?;
        if expected == 0 || expected != bytes.len() {
            return None;
        }
        let (grey, colour) = match format {
            PictureFormat::Grey => (bytes, None),
            PictureFormat::Rgb => (
                bytes
                    .chunks_exact(3)
                    .map(|c| luma([c[0], c[1], c[2]]))
                    .collect(),
                Some(bytes),
            ),
        };
        let entry = HeldPicture {
            handle,
            width,
            height,
            grey,
            colour,
            used: std::cell::Cell::new(0),
        };
        let size = entry.bytes();
        if size > self.budget {
            return None;
        }
        self.remove(handle);
        let mut evicted = Vec::new();
        while self.held + size > self.budget {
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
            self.held -= self.entries[oldest].bytes();
            self.entries.remove(oldest);
        }
        self.held += size;
        self.clock.set(self.clock.get() + 1);
        entry.used.set(self.clock.get());
        self.entries.push(entry);
        Some(evicted)
    }

    /// Starts a bounded, in-order upload without replacing the live picture.
    ///
    /// Starting another upload cancels the incomplete one. The previous live
    /// value under `handle` remains drawable until [`Self::commit_upload`].
    pub fn begin_upload(&mut self, handle: PictureHandle, width: u32, height: u32) -> bool {
        self.begin_upload_with(handle, width, height, PictureFormat::Grey)
    }

    /// [`Self::begin_upload`] for a picture in any format.
    ///
    /// The budget check here is on the bytes in flight. A colour picture is
    /// charged for its grey plane as well once it lands, so an upload that
    /// starts may still be refused at commit; the sender learns that from the
    /// commit result exactly as it would for eviction.
    pub fn begin_upload_with(
        &mut self,
        handle: PictureHandle,
        width: u32,
        height: u32,
        format: PictureFormat,
    ) -> bool {
        let Some(expected) = format.byte_len(width, height) else {
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
            format,
            expected,
            bytes: Vec::with_capacity(expected),
        });
        true
    }

    /// Appends one chunk at its exact expected offset.
    pub fn upload_chunk(&mut self, handle: PictureHandle, offset: usize, bytes: &[u8]) -> bool {
        let Some(pending) = self.pending.as_mut() else {
            return false;
        };
        if pending.handle != handle
            || offset != pending.bytes.len()
            || pending.bytes.len().saturating_add(bytes.len()) > pending.expected
        {
            self.pending = None;
            return false;
        }
        pending.bytes.extend_from_slice(bytes);
        true
    }

    /// Atomically replaces the live picture after every byte has arrived.
    ///
    /// Returns evicted handles on success and `None` for an incomplete or
    /// mismatched upload.
    pub fn commit_upload(&mut self, handle: PictureHandle) -> Option<Vec<PictureHandle>> {
        let pending = self.pending.take()?;
        if pending.handle != handle || pending.bytes.len() != pending.expected {
            return None;
        }
        self.put_report_with(
            pending.handle,
            pending.width,
            pending.height,
            pending.format,
            pending.bytes,
        )
    }

    pub fn remove(&mut self, handle: PictureHandle) {
        if let Some(index) = self.entries.iter().position(|entry| entry.handle == handle) {
            self.held -= self.entries[index].bytes();
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
            colour: entry.colour.as_deref(),
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
    // Colour is drawn only when the whole plane is there; a short one is
    // treated as absent rather than read past.
    let colour = pixels
        .colour
        .filter(|colour| colour.len() >= source_width * source_height * 3);
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
            let mut total = [0u32; 3];
            let mut counted = 0u32;
            for sample_y in from_y..to_y.min(source_height) {
                let base = sample_y * source_width;
                for sample_x in from_x..to_x.min(source_width) {
                    let index = base + sample_x;
                    match colour {
                        Some(colour) => {
                            let pixel = &colour[index * 3..index * 3 + 3];
                            total[0] += u32::from(pixel[0]);
                            total[1] += u32::from(pixel[1]);
                            total[2] += u32::from(pixel[2]);
                        }
                        None => total[0] += u32::from(pixels.grey[index]),
                    }
                    counted += 1;
                }
            }
            if counted == 0 {
                continue;
            }
            let mean = |sum: u32| u8::try_from(sum / counted).unwrap_or(u8::MAX);
            if colour.is_some() {
                surface.blend_colour(x, y, [mean(total[0]), mean(total[1]), mean(total[2])], 255);
            } else {
                surface.blend(x, y, mean(total[0]), 255);
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

/// Converts a physical panel coordinate into the active logical viewport.
///
/// Landscape follows the clockwise display convention: logical top-left is
/// displayed at physical top-right. The inverse is applied before hit testing.
#[must_use]
pub const fn logical_point(
    orientation: Orientation,
    physical_width: i32,
    x: i32,
    y: i32,
) -> (i32, i32) {
    logical_point_with_turn(
        orientation,
        LandscapeTurn::Clockwise,
        physical_width,
        0,
        x,
        y,
    )
}

#[must_use]
pub const fn logical_point_with_turn(
    orientation: Orientation,
    turn: LandscapeTurn,
    physical_width: i32,
    physical_height: i32,
    x: i32,
    y: i32,
) -> (i32, i32) {
    match orientation {
        Orientation::Portrait => (x, y),
        Orientation::Landscape => match turn {
            LandscapeTurn::Clockwise => (y, physical_width - 1 - x),
            LandscapeTurn::CounterClockwise => (physical_height - 1 - y, x),
        },
    }
}

/// Rasterizes into the physical panel in an application's requested direction.
///
/// Landscape is deliberately a full logical repaint: a rotated partial damage
/// rectangle can otherwise leave stale pixels outside its transformed bounds.
pub fn render_oriented(
    screen: &Screen,
    physical_metrics: &DisplayMetrics,
    chrome: &Chrome,
    pictures: &dyn Pictures,
    surface: &mut Surface,
    dirty: Option<Rect>,
    orientation: Orientation,
) {
    render_oriented_with_turn(
        screen,
        physical_metrics,
        chrome,
        pictures,
        surface,
        dirty,
        orientation,
        LandscapeTurn::Clockwise,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_oriented_with_turn(
    screen: &Screen,
    physical_metrics: &DisplayMetrics,
    chrome: &Chrome,
    pictures: &dyn Pictures,
    surface: &mut Surface,
    dirty: Option<Rect>,
    orientation: Orientation,
    turn: LandscapeTurn,
) {
    if orientation == Orientation::Portrait {
        render_all(screen, physical_metrics, chrome, pictures, surface, dirty);
        return;
    }
    let metrics = physical_metrics.oriented(orientation);
    let mut logical = Surface::new(surface.height, surface.width);
    render_all(screen, &metrics, chrome, pictures, &mut logical, None);
    rotate_landscape(&logical, surface, turn);
}

fn rotate_landscape(logical: &Surface, physical: &mut Surface, turn: LandscapeTurn) {
    if logical.width != physical.height || logical.height != physical.width {
        return;
    }
    physical.chroma = logical
        .chroma
        .as_ref()
        .map(|_| vec![255; physical.pixels.len() * 3]);
    for logical_y in 0..logical.height {
        for logical_x in 0..logical.width {
            let (physical_x, physical_y) = match turn {
                LandscapeTurn::Clockwise => (physical.width - 1 - logical_y, logical_x),
                LandscapeTurn::CounterClockwise => (logical_y, physical.height - 1 - logical_x),
            };
            let destination = physical_y * physical.width + physical_x;
            let source = logical_y * logical.width + logical_x;
            physical.pixels[destination] = logical.pixels[source];
            if let (Some(from), Some(to)) = (&logical.chroma, &mut physical.chroma) {
                to[destination * 3..destination * 3 + 3]
                    .copy_from_slice(&from[source * 3..source * 3 + 3]);
            }
        }
    }
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
    let _environment = environment::TextEnvironment::enter(screen, metrics);
    with_legacy_typography(screen.legacy_typography, || {
        with_reading_font(screen.reading_font, || {
            render_all_with_selected_font(screen, metrics, chrome, pictures, surface, dirty);
        });
    });
}

fn render_all_with_selected_font(
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
    let palette = Palette::new(chrome.appearance, screen.accent.chosen());
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
            LayoutKind::Field(_, error) => {
                if error {
                    fill_clipped(surface, node.rect, palette.field_error(), clip);
                }
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
            }
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
                    fill_clipped(surface, node.rect, palette.emphasis(), clip);
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
                        palette.emphasis(),
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
            LayoutKind::PageRail { page, of } => {
                fill_clipped(surface, node.rect, tone::RULE_LIGHT, clip);
                let height = node.rect.height.saturating_div(i32::from(of)).max(1);
                let y = node.rect.y.saturating_add(
                    node.rect.height.saturating_mul(i32::from(page)) / i32::from(of),
                );
                fill_clipped(
                    surface,
                    Rect {
                        y,
                        height,
                        ..node.rect
                    },
                    tone::INK,
                    clip,
                );
            }
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
            LayoutKind::TableCell => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::TableHeaderCell => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::MUTED,
                clip,
            ),
            LayoutKind::TableRule => fill_clipped(surface, node.rect, tone::RULE, clip),
            // Paper, not surface, and a heavy border. An overlay has to look
            // like a separate sheet laid on the page, and the only two things
            // available to say so without shading everything else are the
            // brightest tone on the panel and a line thick enough to read as
            // an edge.
            // A sheet lifted off the page: rounded like the controls on it, a
            // hairline edge, and a grey shadow along the bottom and trailing
            // sides. The heavy black box it replaces read as a warning frame
            // round every menu and question. The shadow is drawn inside the
            // overlay's own rectangle, so the refresh that clears the overlay
            // clears it too and the panel keeps no grey residue.
            LayoutKind::Overlay => {
                let radius = metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM);
                let lift = max(2, metrics.rule_thickness() * 3);
                let sheet = Rect {
                    width: node.rect.width - lift,
                    height: node.rect.height - lift,
                    ..node.rect
                };
                let shadow = Rect {
                    x: node.rect.x + lift,
                    y: node.rect.y + lift,
                    ..sheet
                };
                fill_clipped(surface, node.rect, tone::PAPER, clip);
                fill_rounded_clipped(surface, shadow, radius, tone::RULE, clip);
                fill_rounded_clipped(surface, sheet, radius, tone::PAPER, clip);
                stroke_rounded_clipped(
                    surface,
                    sheet,
                    radius,
                    tone::INK,
                    metrics.rule_thickness(),
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
                    palette.emphasis(),
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
            LayoutKind::Cell(_, CellStyle::Crossword, selected) => {
                if selected {
                    fill_clipped(surface, node.rect, palette.selection_field(), clip);
                }
                // Two edges rather than four. Crossword squares are drawn
                // touching, so a box around each one puts two rules along
                // every line the grid shares and one around the outside: the
                // inside of the grid came out twice as heavy as its border,
                // which is the opposite of how a printed grid is ruled. The
                // board draws the outside, and each square draws the two edges
                // no neighbour above or to its left has drawn already.
                let rule = metrics.rule_thickness();
                fill_clipped(
                    surface,
                    Rect {
                        height: rule,
                        ..node.rect
                    },
                    tone::INK,
                    clip,
                );
                fill_clipped(
                    surface,
                    Rect {
                        width: rule,
                        ..node.rect
                    },
                    tone::INK,
                    clip,
                );
            }
            LayoutKind::CrosswordBoard => {
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness(),
                    clip,
                );
            }
            LayoutKind::Cell(_, CellStyle::CrosswordBlock, _) => {
                fill_clipped(surface, node.rect, tone::INK, clip);
            }
            // The square in hand on a board with light and dark squares takes
            // the accent's tint. A plain grid's selected cell keeps the quiet
            // grey, because what selection means there is the application's
            // business and is not always one cell.
            LayoutKind::Cell(_, CellStyle::BoardLight, true) => {
                fill_clipped(surface, node.rect, palette.selection_field(), clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness(),
                    clip,
                );
            }
            LayoutKind::Cell(_, CellStyle::BoardLight, false) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            LayoutKind::Cell(_, CellStyle::Board, true) => {
                fill_clipped(surface, node.rect, tone::SURFACE, clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::INK,
                    metrics.rule_thickness(),
                    clip,
                );
            }
            LayoutKind::Cell(_, CellStyle::Board, false) => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            LayoutKind::Cell(_, CellStyle::BoardDark, selected) => {
                fill_clipped(surface, node.rect, palette.board_dark(), clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    if selected { tone::INK } else { tone::RULE },
                    metrics.rule_thickness() * if selected { 2 } else { 1 },
                    clip,
                );
            }
            LayoutKind::Cell(_, CellStyle::BackgammonTop, _) => {
                draw_backgammon_point(surface, node.rect, true, metrics, palette, clip);
            }
            LayoutKind::Cell(_, CellStyle::BackgammonBottom, _) => {
                draw_backgammon_point(surface, node.rect, false, metrics, palette, clip);
            }
            LayoutKind::BackgammonStack(glyph, count, from_top) => {
                draw_backgammon_stack(surface, glyph, count, from_top, node.rect, clip);
            }
            LayoutKind::BackgammonBoard => draw_backgammon_board(surface, node.rect, metrics, clip),
            LayoutKind::MorrisBoard => draw_morris_board(surface, node.rect, metrics, clip),
            // A key is the field it is printed on, with no rule at all. The
            // gaps between the keys separate them, which is how a keyboard has
            // always been read, and it takes forty-five outlines off the panel.
            // Nothing at all: the picture is the whole of it.
            LayoutKind::Cell(_, CellStyle::Plain, _) => {}
            LayoutKind::Cell(_, CellStyle::Key, selected) => {
                fill_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::SURFACE,
                    clip,
                );
                if selected {
                    stroke_rounded_clipped(
                        surface,
                        node.rect,
                        metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                        tone::INK,
                        metrics.button_border(),
                        clip,
                    );
                }
            }
            LayoutKind::Cell(_, style @ (CellStyle::Pad | CellStyle::EmptyPad), _) => {
                let radius = metrics.tenth_mm(PAD_RADIUS_TENTH_MM);
                fill_rounded_clipped(surface, node.rect, radius, tone::PAPER, clip);
                stroke_rounded_clipped(
                    surface,
                    node.rect,
                    radius,
                    if style == CellStyle::Pad {
                        tone::INK
                    } else {
                        tone::RULE
                    },
                    metrics.tenth_mm(PAD_BORDER_TENTH_MM),
                    clip,
                );
            }
            LayoutKind::PencilNumber(inverted) => {
                let (size, scale) = pencil::label_style(&node);
                with_text_scale(scale, || {
                    draw_centered(
                        surface,
                        &node.text_lines,
                        node.rect,
                        size,
                        if inverted { tone::PAPER } else { tone::INK },
                        clip,
                    );
                });
            }
            LayoutKind::PencilMark(mark, selected, peer, box_mask) => {
                pencil::draw_mark(
                    surface,
                    node.rect,
                    mark,
                    pencil::MarkStyle {
                        selected,
                        peer,
                        box_mask,
                    },
                    metrics,
                    palette,
                    clip,
                );
            }
            LayoutKind::PencilEdge(state, vertical) => {
                pencil::draw_edge(surface, node.rect, state, vertical, metrics, clip);
            }
            LayoutKind::BoardMark(mark, locked) => {
                board::draw_mark(surface, node.rect, mark, locked, metrics, clip);
            }
            LayoutKind::BoardClueChip => {
                fill_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::SURFACE,
                    clip,
                );
                stroke_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::INK,
                    metrics.button_border(),
                    clip,
                );
            }
            LayoutKind::BoardClue => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                FontSize::Caption,
                tone::INK,
                clip,
            ),
            LayoutKind::CellLabel(board) => {
                // A short label on a board is a mark rather than a word: an X,
                // an O or a Sudoku digit is the content of the cell and should
                // fill it. A short label on a keyboard key is one letter among
                // several, and setting it larger than its neighbours is how a
                // row of `esc tab ctrl up down left right` came out with one
                // enormous `up` in the middle of it, and how a ten-key letter
                // row -- narrower per cell than the nine-key rows either side
                // of it, so taller than it is wide -- once came out bold and
                // oversized while every other row stayed put.
                //
                // So this is carried rather than read off the cell's own
                // rectangle: a board cell and a keyboard key can be the same
                // shape, and only the board's mark is meant to be the size of
                // the whole cell.
                let (size, scale) = if board {
                    board_label_style(&node)
                } else {
                    key_label_style(&node)
                };
                with_text_scale(scale, || {
                    draw_centered(surface, &node.text_lines, node.rect, size, tone::INK, clip);
                });
            }
            LayoutKind::Divider => fill_clipped(surface, node.rect, tone::RULE, clip),
            LayoutKind::RowRule => fill_clipped(surface, node.rect, tone::RULE_LIGHT, clip),
            // A grey track with the done part in ink, both a millimetre tall
            // and centred in the bar's two. It was a black outline the width of
            // the page with the fill butted against it, which on the panel was
            // a heavy black box louder than the title it sat under, and at a
            // few per cent read as an empty box rather than as progress.
            LayoutKind::Progress => {
                let value = node
                    .text_lines
                    .first()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(0);
                let thickness = min(node.rect.height, metrics.tenth_mm(10));
                let track = Rect {
                    x: node.rect.x,
                    y: node.rect.y + (node.rect.height - thickness) / 2,
                    width: node.rect.width,
                    height: thickness,
                };
                fill_clipped(surface, track, tone::RULE, clip);
                fill_clipped(
                    surface,
                    Rect {
                        width: track.width.saturating_mul(value.clamp(0, 100)) / 100,
                        ..track
                    },
                    palette.emphasis(),
                    clip,
                );
            }

            LayoutKind::Heading(level) => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::for_heading_level(level),
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
                draw_lines_in(
                    surface,
                    &node.text_lines,
                    node.rect.x,
                    node.rect.y + top,
                    role.size(),
                    prose,
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
            // The words are already on the panel: the paragraph drew them.
            // What a link adds is the one mark that has meant "this goes
            // somewhere" since long before anybody put it on a screen. Set
            // clear of the descenders rather than through them, and a rule
            // thick, because a single pixel at this density is not there.
            LayoutKind::InlineLink(_) => {
                let thickness = metrics.rule_thickness();
                let baseline = node
                    .rect
                    .y
                    .saturating_add(FontSize::Body.line_height_in(prose))
                    .saturating_sub(thickness.saturating_mul(2));
                fill_clipped(
                    surface,
                    Rect {
                        x: node.rect.x,
                        y: baseline,
                        width: node.rect.width,
                        height: thickness,
                    },
                    tone::INK,
                    clip,
                );
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
            LayoutKind::RichText(presentation) => {
                if let Some(text) = node.text_lines.first() {
                    if presentation.highlighted {
                        fill_clipped(surface, node.rect, tone::SURFACE, clip);
                    }
                    draw_rich_text(
                        surface,
                        text,
                        node.rect,
                        prose,
                        presentation,
                        tone::INK,
                        clip,
                    );
                }
            }
            // The bars themselves are only a background. Drawing them as
            // separate nodes is what lets a tab switch repaint the content
            // area and two small bands instead of the entire panel.
            // The bar wears the application's colour on a panel that has
            // one, with its title and controls still in ink on it; the bottom
            // bar stays paper, because two coloured bands would frame every
            // screen in the same colour and the content between them would
            // be the quietest thing on the panel.
            LayoutKind::TopBar => fill_clipped(surface, node.rect, palette.bar(), clip),
            LayoutKind::NavBar => fill_clipped(surface, node.rect, tone::PAPER, clip),
            // Paper, not surface. The band is the quietest thing on the panel
            // and a tinted strip across the top of every screen is a permanent
            // horizontal line the eye has to learn to ignore.
            LayoutKind::StatusBand => fill_clipped(surface, node.rect, tone::PAPER, clip),
            // The one string on the panel that changes while its neighbours
            // stay, so its digits go on a fixed advance and it counts without
            // stepping sideways.
            LayoutKind::StatusClock => draw_figures_crisp(
                surface,
                node.text_lines.first().map_or("", String::as_str),
                node.rect.x,
                node.rect.y + (node.rect.height - FontSize::Caption.line_height()) / 2,
                clip,
            ),
            LayoutKind::StatusMark(mark) => {
                draw_status_mark(surface, mark, node.rect, metrics, palette, clip);
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
                if FontSize::Body.line_height() <= node.rect.height {
                    FontSize::Body
                } else {
                    FontSize::Caption
                },
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
                    palette,
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
                    palette,
                    clip,
                );
            }
            LayoutKind::Tile(..) if legacy_typography() => stroke_clipped(
                surface,
                node.rect,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
            LayoutKind::Tile(..) => {}
            LayoutKind::TileOutline(state) => stroke_clipped(
                surface,
                node.rect,
                if state == ControlState::Enabled {
                    tone::INK
                } else {
                    tone::RULE
                },
                metrics.button_border(),
                clip,
            ),
            // The tap target itself draws nothing. A hairline between rows is
            // enough separation, and a box around each one would add weight
            // that a list of several entries cannot carry.
            LayoutKind::Row(_) => {}
            LayoutKind::RowTitle => draw_lines_in(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                prose,
                tone::INK,
                clip,
            ),
            LayoutKind::RowTitleDone => draw_struck_lines(
                surface,
                &node.text_lines,
                node.rect,
                metrics,
                FontSize::Body,
                prose,
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
            LayoutKind::TileGlyph(glyph) | LayoutKind::InlineGlyph(glyph, false) => {
                draw_glyph_icon(surface, glyph, node.rect, clip);
            }
            LayoutKind::InlineGlyph(glyph, true) => {
                draw_glyph_icon_in(surface, glyph, node.rect, clip, tone::PAPER);
            }
            LayoutKind::TileGlyphMuted(glyph) => {
                draw_glyph_icon_in(surface, glyph, node.rect, clip, tone::MUTED);
            }
            // Bare, because a formula is part of a sentence and a rule round
            // one would read as a box drawn in the middle of the words.
            LayoutKind::Picture(handle) => {
                if let Some(pixels) = pictures.get(handle) {
                    draw_picture(surface, node.rect, pixels, clip);
                }
            }
            // Outlined, because a cover or a plate with pale edges on white
            // paper has no boundary at all and reads as text floating in
            // space.
            LayoutKind::FramedPicture(handle) => {
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
                tile_label_size(node.rect, node.text_lines.len(), !legacy_typography()),
                tone::INK,
                clip,
            ),
            LayoutKind::TileLabelMuted => draw_centered(
                surface,
                &node.text_lines,
                node.rect,
                tile_label_size(node.rect, node.text_lines.len(), true),
                tone::MUTED,
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
            LayoutKind::TileValue => {
                let text = node.text_lines.first().map_or("", String::as_str);
                let width = figures_width(text, FontSize::Caption, Face::Text);
                draw_figures(
                    surface,
                    text,
                    node.rect.x + node.rect.width.saturating_sub(width),
                    node.rect.y,
                    FontSize::Caption,
                    Face::Text,
                    tone::MUTED,
                    clip,
                );
            }
            LayoutKind::SectionLink(_) => {
                let text = node.text_lines.first().map_or("", String::as_str);
                let width = measure_text(text, FontSize::Caption).0;
                draw_text(
                    surface,
                    text,
                    node.rect.x + node.rect.width.saturating_sub(width),
                    node.rect.y + (node.rect.height - FontSize::Caption.line_height()) / 2,
                    FontSize::Caption,
                    tone::INK,
                    clip,
                );
            }
            // Paper first, then the border, then the mark. The corner a chip
            // sits in is very often a cover, and a tick drawn straight onto a
            // dark cover is a tick nobody can see.
            LayoutKind::TileState(TileState::Busy) => {
                fill_clipped(surface, node.rect, tone::PAPER, clip);
                stroke_clipped(
                    surface,
                    node.rect,
                    tone::RULE,
                    metrics.rule_thickness(),
                    clip,
                );
                let dot = max_i32(2, node.rect.width / 3);
                fill_clipped(
                    surface,
                    Rect {
                        x: node.rect.x + (node.rect.width - dot) / 2,
                        y: node.rect.y + (node.rect.height - dot) / 2,
                        width: dot,
                        height: dot,
                    },
                    tone::INK,
                    clip,
                );
            }
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
            // The two ends carry a picture and nothing else at all: no word,
            // no outline and no field. A minus and a plus either side of a
            // reading are already a stepper on every device ever made, and
            // drawing a box round each of them turns a quiet line into two
            // more things to look at. The target stays a full touch target
            // whatever the picture inside it measures.
            LayoutKind::StepperControl(_, state, glyph) => {
                let tone = if state.is_enabled() {
                    tone::INK
                } else {
                    tone::RULE
                };
                let size = FontSize::Heading.line_height();
                draw_glyph_icon_in(
                    surface,
                    glyph,
                    Rect {
                        x: node.rect.x + (node.rect.width - size) / 2,
                        y: node.rect.y + (node.rect.height - size) / 2,
                        width: size,
                        height: size,
                    },
                    clip,
                    tone,
                );
            }
            // Centred between the two controls, because a reading that sits
            // against one of them reads as a label for that control.
            LayoutKind::StepperValue => {
                let (measured, _) = measure_text(
                    node.text_lines.first().map_or("", String::as_str),
                    FontSize::Body,
                );
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x + max(0, (node.rect.width - measured) / 2),
                    node.rect.y + (node.rect.height - FontSize::Body.line_height()) / 2,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            // A hairline track rather than an outlined bar: it says where in
            // the range the value sits and is not itself a control, so it must
            // not carry as much ink as the two things that are.
            LayoutKind::StepperTrack(fill) => {
                fill_clipped(surface, node.rect, tone::RULE, clip);
                fill_clipped(
                    surface,
                    Rect {
                        width: node.rect.width.saturating_mul(min(100, i32::from(fill))) / 100,
                        ..node.rect
                    },
                    tone::INK,
                    clip,
                );
            }
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
                fill_clipped(surface, node.rect, palette.banner(level), clip);
                let mut text_x = node.rect.x + padding;
                // A success and an error carry a mark ahead of the words, in a
                // column the layout set aside for it, so the level can be told
                // without reading. An error is also ruled round in ink: it is
                // the one banner that asks the reader to do something.
                let mark = banner_mark_column(level, metrics);
                if mark > 0 {
                    let side = FontSize::Body.line_height();
                    draw_glyph_icon_in(
                        surface,
                        if level == BannerLevel::Error {
                            Glyph::Close
                        } else {
                            Glyph::Check
                        },
                        Rect {
                            x: text_x,
                            y: node.rect.y + padding,
                            width: side,
                            height: side,
                        },
                        clip,
                        tone::INK,
                    );
                    text_x = text_x.saturating_add(mark);
                }
                if level == BannerLevel::Error {
                    stroke_clipped(surface, node.rect, tone::INK, metrics.button_border(), clip);
                }
                if level == BannerLevel::Attention {
                    let bar = metrics.rule_thickness() * 3;
                    fill_clipped(
                        surface,
                        Rect {
                            width: bar,
                            ..node.rect
                        },
                        palette.banner_edge(),
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
            // Nothing. The row is the target; its words and its switch are
            // the nodes that follow it.
            LayoutKind::Toggle(..) => {}
            LayoutKind::ToggleLabel(state) => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                if state.is_enabled() {
                    tone::INK
                } else {
                    tone::MUTED
                },
                clip,
            ),
            LayoutKind::ToggleSwitch(on, state) => {
                draw_toggle_switch(
                    surface,
                    toggle_track(node.rect, metrics),
                    on,
                    state,
                    metrics,
                    palette,
                    clip,
                );
            }
            LayoutKind::RowCheck(state) => {
                draw_check_box(surface, node.rect, state, metrics, palette, clip);
            }
            // An outline the height of a line of type and its air, centred in
            // the finger-high target, so the row keeps one quiet button rather
            // than a slab as tall as the row.
            LayoutKind::RowButton(_) => {
                let line = FontSize::Body.line_height();
                let tight = metrics.space(Space::Tight);
                let height = min(node.rect.height, line + tight * 2);
                let outline = Rect {
                    y: node.rect.y + (node.rect.height - height) / 2,
                    height,
                    ..node.rect
                };
                stroke_rounded_clipped(
                    surface,
                    outline,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::INK,
                    metrics.button_border(),
                    clip,
                );
                draw_centered(
                    surface,
                    &node.text_lines,
                    outline,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            // Reversed out of ink: the one small thing on a row that has to
            // be seen before the row is read, and small enough that inverting
            // it costs a refresh the size of a fingernail.
            LayoutKind::RowBadge => {
                fill_rounded_clipped(surface, node.rect, node.rect.height / 2, tone::INK, clip);
                draw_centered(
                    surface,
                    &node.text_lines,
                    node.rect,
                    FontSize::Caption,
                    tone::PAPER,
                    clip,
                );
            }
            LayoutKind::FieldLabel => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::INK,
                clip,
            ),
            // A hint is muted, an error is ink: the same line, saying
            // something the reader has to act on.
            LayoutKind::FieldNote(error) => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                if error { tone::INK } else { tone::MUTED },
                clip,
            ),
            LayoutKind::FieldSecret(count) => {
                let (dot, gap) = secret_dot(metrics);
                let top = node.rect.y + (node.rect.height - dot) / 2;
                for index in 0..i32::from(count) {
                    fill_rounded_clipped(
                        surface,
                        Rect {
                            x: node.rect.x + index * (dot + gap),
                            y: top,
                            width: dot,
                            height: dot,
                        },
                        dot / 2,
                        tone::INK,
                        clip,
                    );
                }
            }
            // Paper inside an ink outline, so on the banner's grey it reads as
            // a button laid on the banner rather than a hole in it.
            LayoutKind::BannerAction(_) => {
                let radius = metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM);
                fill_rounded_clipped(surface, node.rect, radius, tone::PAPER, clip);
                stroke_rounded_clipped(
                    surface,
                    node.rect,
                    radius,
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
            // The quiet surface the banner uses, so the bar reads as a strip
            // of tools laid over the list rather than one more row of it.
            LayoutKind::SelectionBar => {
                fill_rounded_clipped(
                    surface,
                    node.rect,
                    metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM),
                    tone::SURFACE,
                    clip,
                );
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x + metrics.space(Space::Small),
                    node.rect.y + (node.rect.height - FontSize::Body.line_height()) / 2,
                    FontSize::Body,
                    tone::INK,
                    clip,
                );
            }
            // A hairline before each verb, inset from the bar's edges, which
            // is all it takes to show where one target ends and the next
            // begins without boxing each of them.
            LayoutKind::SelectionAction(_) => {
                let inset = metrics.space(Space::Small);
                fill_clipped(
                    surface,
                    Rect {
                        x: node.rect.x,
                        y: node.rect.y + inset,
                        width: metrics.rule_thickness(),
                        height: max(1, node.rect.height - inset * 2),
                    },
                    tone::RULE,
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
            // A ring in the lead column, filled when the option is the answer
            // already given. The ring is drawn on every row, so the list says
            // it is one choice among many before anything is chosen.
            LayoutKind::OptionRow(_, chosen) => {
                let column = row_mark_column(metrics);
                let side = min(column, FontSize::Body.em_in(Face::Text));
                let ring = Rect {
                    x: node.rect.x + (column - side) / 2,
                    y: node.rect.y + (node.rect.height - side) / 2,
                    width: side,
                    height: side,
                };
                stroke_rounded_clipped(
                    surface,
                    ring,
                    side / 2,
                    tone::INK,
                    metrics.button_border(),
                    clip,
                );
                if chosen {
                    let inner = side / 2;
                    fill_rounded_clipped(
                        surface,
                        Rect {
                            x: ring.x + (side - inner) / 2,
                            y: ring.y + (side - inner) / 2,
                            width: inner,
                            height: inner,
                        },
                        inner / 2,
                        palette.emphasis(),
                        clip,
                    );
                }
            }
            LayoutKind::OptionLabel => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
                tone::INK,
                clip,
            ),
            LayoutKind::Code => {
                let payload = node.text_lines.first().map_or("", String::as_str);
                draw_code(surface, node.rect, payload, clip);
            }
            LayoutKind::CodeCaption => {
                let line = FontSize::Caption.line_height();
                for (index, text) in node.text_lines.iter().enumerate() {
                    let width = measure_text(text, FontSize::Caption).0;
                    draw_text(
                        surface,
                        text,
                        node.rect.x + max(0, (node.rect.width - width) / 2),
                        node.rect.y + i32::try_from(index).unwrap_or(0) * line,
                        FontSize::Caption,
                        tone::INK,
                        clip,
                    );
                }
            }
            LayoutKind::Steps(current, total) => {
                draw_lines(
                    surface,
                    &node.text_lines,
                    node.rect.x,
                    node.rect.y,
                    FontSize::Caption,
                    tone::INK,
                    clip,
                );
                draw_step_marks(surface, node.rect, current, total, metrics, palette, clip);
            }
        }
    }
}

/// The switch at a toggle's edge, in ink and paper only so that turning it
/// over is a two-level change the panel can make with its fast waveform.
///
/// On is a filled track with the knob at the trailing end, off an outlined
/// track with the knob at the leading end: position and fill both say which,
/// so neither has to be read on its own. A switch that cannot change is drawn
/// in the rule's grey instead of ink.
fn draw_toggle_switch(
    surface: &mut Surface,
    track: Rect,
    on: bool,
    state: ControlState,
    metrics: &DisplayMetrics,
    palette: Palette,
    clip: Rect,
) {
    let ink = if state.is_enabled() {
        tone::INK
    } else {
        tone::RULE
    };
    // Only the filled track takes the accent. Off is drawn in ink whatever
    // the panel, so the two states differ in colour as well as in shape and
    // a switch is never a coloured ring with a coloured knob.
    let on_track = if state.is_enabled() {
        palette.emphasis()
    } else {
        Paint::Grey(tone::RULE)
    };
    let radius = track.height / 2;
    let border = metrics.button_border();
    let inset = border * 2;
    let knob = max(1, track.height - inset * 2);
    let knob_x = if on {
        track.x + track.width - inset - knob
    } else {
        track.x + inset
    };
    let knob = Rect {
        x: knob_x,
        y: track.y + inset,
        width: knob,
        height: knob,
    };
    if on {
        fill_rounded_clipped(surface, track, radius, on_track, clip);
        fill_rounded_clipped(surface, knob, knob.height / 2, tone::PAPER, clip);
    } else {
        stroke_rounded_clipped(surface, track, radius, ink, border, clip);
        fill_rounded_clipped(surface, knob, knob.height / 2, ink, clip);
    }
}

/// The box a row in a check state carries in place of its lead.
///
/// Ticked is a filled box with the tick reversed out of it, unticked an empty
/// one, and mixed an empty one with a bar across it: three shapes that differ
/// in outline as well as fill, which is what survives a panel that has not
/// been cleaned for a while.
fn draw_check_box(
    surface: &mut Surface,
    rect: Rect,
    state: RowState,
    metrics: &DisplayMetrics,
    palette: Palette,
    clip: Rect,
) {
    let side = min(rect.width, rect.height);
    let side = side - side / 6;
    let rect = Rect {
        x: rect.x + (rect.width - side) / 2,
        y: rect.y + (rect.height - side) / 2,
        width: side,
        height: side,
    };
    let radius = metrics.tenth_mm(BUTTON_RADIUS_TENTH_MM).min(side / 4);
    match state {
        RowState::Checked => {
            fill_rounded_clipped(surface, rect, radius, palette.emphasis(), clip);
            // Whole pixels of paper rather than a blended tick, so the box
            // turning over is a two-level change the panel's fast waveform
            // can make.
            let inset = side / 6;
            blit_vector_crisp_in(
                surface,
                &vector::shapes(Glyph::Check),
                max(1, side - inset * 2),
                rect,
                clip,
                tone::PAPER,
            );
        }
        RowState::Mixed => {
            stroke_rounded_clipped(
                surface,
                rect,
                radius,
                tone::INK,
                metrics.button_border(),
                clip,
            );
            let bar = max(2, metrics.button_border() * 2);
            let inset = side / 4;
            fill_clipped(
                surface,
                Rect {
                    x: rect.x + inset,
                    y: rect.y + (side - bar) / 2,
                    width: max(1, side - inset * 2),
                    height: bar,
                },
                tone::INK,
                clip,
            );
        }
        RowState::Unchecked | RowState::Open | RowState::Done => {
            stroke_rounded_clipped(
                surface,
                rect,
                radius,
                tone::INK,
                metrics.button_border(),
                clip,
            );
        }
    }
}

/// The quiet zone a QR code needs around it, in modules, by the standard.
const CODE_QUIET_MODULES: i32 = 4;

/// The QR code for `payload`, at medium error correction and no larger than
/// version 10, or nothing when the payload is empty or does not fit.
///
/// Medium rather than low, because a reader's panel keeps a little ghosting
/// from the last page and the modules are not as black as print. The
/// correction is raised past medium when the payload fits the same version
/// anyway, so a short address gets the sturdiest code its size allows.
fn encode_code(payload: &str) -> Option<qrcodegen::QrCode> {
    if payload.is_empty() || payload.len() > MAX_CODE_BYTES {
        return None;
    }
    let segments = qrcodegen::QrSegment::make_segments(payload);
    qrcodegen::QrCode::encode_segments_advanced(
        &segments,
        qrcodegen::QrCodeEcc::Medium,
        qrcodegen::Version::MIN,
        qrcodegen::Version::new(10),
        None,
        true,
    )
    .ok()
}

/// Draws a code into its square: paper, then whole-pixel modules of ink.
///
/// Each module is the same whole number of pixels, and the code is centred in
/// whatever that leaves over. Modules of uneven width, or edges blended into
/// grey, are what make a camera misjudge where one module ends, and a code of
/// pure ink and paper is also one the panel can draw with its fast waveform.
fn draw_code(surface: &mut Surface, rect: Rect, payload: &str, clip: Rect) {
    fill_clipped(surface, rect, tone::PAPER, clip);
    let Some(code) = encode_code(payload) else {
        return;
    };
    let size = code.size();
    let total = size + CODE_QUIET_MODULES * 2;
    let module = min(rect.width, rect.height) / total.max(1);
    if module < 1 {
        return;
    }
    let drawn = module * total;
    let left = rect.x + (rect.width - drawn) / 2 + CODE_QUIET_MODULES * module;
    let top = rect.y + (rect.height - drawn) / 2 + CODE_QUIET_MODULES * module;
    for row in 0..size {
        let mut column = 0;
        while column < size {
            if !code.get_module(column, row) {
                column += 1;
                continue;
            }
            // A run of dark modules is one rectangle, which is a fifth of
            // the fills on a typical code.
            let start = column;
            while column < size && code.get_module(column, row) {
                column += 1;
            }
            fill_clipped(
                surface,
                Rect {
                    x: left + start * module,
                    y: top + row * module,
                    width: (column - start) * module,
                    height: module,
                },
                tone::INK,
                clip,
            );
        }
    }
}

/// The marks at a step indicator's trailing edge: filled for the steps done,
/// a heavy ring for the one the reader is on, and a light ring for the rest.
#[allow(clippy::too_many_arguments)]
fn draw_step_marks(
    surface: &mut Surface,
    rect: Rect,
    current: u8,
    total: u8,
    metrics: &DisplayMetrics,
    palette: Palette,
    clip: Rect,
) {
    let (mark, gap) = steps_mark(metrics);
    let mut x = rect.x + rect.width - steps_marks_width(total, metrics);
    let y = rect.y + (rect.height - mark) / 2;
    for step in 0..total {
        let dot = Rect {
            x,
            y,
            width: mark,
            height: mark,
        };
        match step.cmp(&current) {
            std::cmp::Ordering::Less => {
                fill_rounded_clipped(surface, dot, mark / 2, palette.emphasis(), clip);
            }
            std::cmp::Ordering::Equal => {
                stroke_rounded_clipped(
                    surface,
                    dot,
                    mark / 2,
                    palette.emphasis(),
                    max(2, mark / 4),
                    clip,
                );
            }
            std::cmp::Ordering::Greater => stroke_rounded_clipped(
                surface,
                dot,
                mark / 2,
                tone::RULE,
                metrics.rule_thickness(),
                clip,
            ),
        }
        x += mark + gap;
    }
}

/// The size a tile's name is drawn at, which has to be the size it was wrapped
/// at.
///
/// A tall label box used to mean a big name, and for a one line label it does.
/// A two line name in a Caption sized band is also in a box taller than one
/// Caption line, and it was drawn in Body: every line came out wider than the
/// width it was wrapped to, so the names of neighbouring books on a shelf ran
/// into each other. What the band holds per line is what the layout decided,
/// so that is what this asks.
fn tile_label_size(rect: Rect, lines: usize, body_allowed: bool) -> FontSize {
    let per_line = rect.height / max_i32(1, i32::try_from(lines).unwrap_or(1));
    if body_allowed && per_line >= FontSize::Body.line_height() {
        FontSize::Body
    } else {
        FontSize::Caption
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

#[allow(clippy::too_many_arguments)]
fn draw_nav_label(
    surface: &mut Surface,
    lines: &[String],
    rect: Rect,
    metrics: &DisplayMetrics,
    selected: bool,
    glyph: Option<Glyph>,
    palette: Palette,
    clip: Rect,
) {
    // A mark sits above the word rather than instead of it. This band is a
    // finger wide and has the room, and it is often the only way off a screen,
    // so it is the last place to make somebody guess. The word drops to the
    // foot of the slot to make space, which is the shape both phone platforms
    // draw a bottom bar in.
    let mut text = rect;
    let mark_tone = if selected || legacy_typography() {
        tone::INK
    } else {
        tone::MUTED
    };
    if let Some(glyph) = glyph {
        let line = FontSize::Caption.line_height();
        let gap = metrics.space(Space::Tight);
        let side = min(
            if legacy_typography() {
                metrics.touch_target_minimum() / 2
            } else {
                metrics.touch_target_minimum() * 2 / 5
            },
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
                mark_tone,
            );
            text = Rect {
                x: rect.x,
                y: top + side + gap,
                width: rect.width,
                height: line,
            };
        }
    }
    draw_centered(
        surface,
        lines,
        text,
        FontSize::Caption,
        if selected || legacy_typography() {
            tone::INK
        } else {
            tone::MUTED
        },
        clip,
    );
    // Selection is marked with a bar rather than a fill. An inverted
    // destination would be the largest black area on the screen and would
    // dominate the content it is meant to be subordinate to.
    if selected {
        let thickness = metrics.rule_thickness() * 2;
        let width = if legacy_typography() {
            rect.width.saturating_sub(2 * metrics.space(Space::Medium))
        } else {
            metrics.tenth_mm(20)
        };
        fill_clipped(
            surface,
            Rect {
                x: rect.x + (rect.width - width) / 2,
                y: if legacy_typography() {
                    rect.y + rect.height - thickness - metrics.space(Space::Small)
                } else {
                    rect.y + metrics.rule_thickness()
                },
                width: width.min(rect.width),
                height: thickness,
            },
            palette.emphasis(),
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
            // Set against the right of its column rather than centred in it,
            // so that a nine and a ten put their units digit in the same
            // place. The digits go on the same fixed advance the clock uses,
            // which is what keeps a column of ranks square down both edges
            // rather than only the right one.
            let text = number.to_string();
            let size = FontSize::Caption;
            let width = figures_width(&text, size, Face::Text);
            let x = rect.x + max(0, rect.width - width);
            let y = rect.y + (rect.height - size.line_height()) / 2;
            draw_figures(surface, &text, x, y, size, Face::Text, tone::MUTED, clip);
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

/// Draws one status mark into the band-high `rect` its layout reserved.
///
/// Marks are vertically centred at seven tenths of the band, the same for
/// every kind, which is what keeps them on one line however many there are.
fn draw_status_mark(
    surface: &mut Surface,
    mark: StatusMark,
    rect: Rect,
    metrics: &DisplayMetrics,
    palette: Palette,
    clip: Rect,
) {
    let size = rect.height * 7 / 10;
    let square = Rect {
        x: rect.x + rect.width - size,
        y: rect.y + (rect.height - size) / 2,
        width: size,
        height: size,
    };
    match mark {
        StatusMark::Signal(strength) => {
            blit_vector_crisp_in(
                surface,
                &vector::wifi(strength),
                size,
                square,
                clip,
                palette.signal(strength),
            );
        }
        StatusMark::Bluetooth => {
            blit_vector_crisp(
                surface,
                &vector::shapes(Glyph::Headphones),
                size,
                square,
                clip,
            );
        }
        // Nothing at all when it could not be read; the room stays reserved.
        StatusMark::Battery {
            level: Some(level),
            charging,
        } => {
            let glyph = Rect {
                x: rect.x + rect.width - battery_glyph_width(size),
                width: battery_glyph_width(size),
                ..square
            };
            let shapes = vector::battery(level, charging);
            blit_vector_crisp_in(
                surface,
                &shapes,
                glyph.width,
                glyph,
                clip,
                palette.battery(level, charging),
            );
            // Set against the battery, so a short number leaves its spare room
            // on the side away from the glyph rather than between them.
            let text = format!("{}%", level.get());
            let width = figures_width(&text, FontSize::Caption, Face::Text);
            draw_figures_crisp(
                surface,
                &text,
                glyph.x - width - metrics.space(Space::Tight),
                rect.y + (rect.height - FontSize::Caption.line_height()) / 2,
                clip,
            );
        }
        StatusMark::Battery { level: None, .. } => {}
    }
}

/// Draws status-band figures in pure ink and paper, on a fixed advance.
///
/// The band's text changes on its own, the clock every minute and the battery
/// whenever the charge moves, and blended edges would put grey on the panel
/// each time: a greyscale refresh for a digit, and a soft figure beside marks
/// that are drawn sharp. The figures are set into a scratch surface and only
/// the pixels at least half covered are inked, the same rule as the marks.
fn draw_figures_crisp(surface: &mut Surface, text: &str, x: i32, y: i32, clip: Rect) {
    let width = figures_width(text, FontSize::Caption, Face::Text);
    let height = FontSize::Caption.line_height();
    let (Ok(columns), Ok(rows)) = (usize::try_from(width), usize::try_from(height)) else {
        return;
    };
    if columns == 0 || rows == 0 {
        return;
    }
    let mut scratch = Surface::new(columns, rows);
    scratch.clear(tone::PAPER);
    let whole = Rect {
        x: 0,
        y: 0,
        width,
        height,
    };
    draw_figures(
        &mut scratch,
        text,
        0,
        0,
        FontSize::Caption,
        Face::Text,
        tone::INK,
        whole,
    );
    for (index, value) in scratch.pixels.iter().enumerate() {
        if *value >= 128 {
            continue;
        }
        let (Ok(column), Ok(row)) = (
            i32::try_from(index % columns),
            i32::try_from(index / columns),
        ) else {
            continue;
        };
        let (px, py) = (x + column, y + row);
        if px < clip.x || py < clip.y || px >= clip.x + clip.width || py >= clip.y + clip.height {
            continue;
        }
        surface.blend(px, py, tone::INK, 255);
    }
}

/// Renders the design box at `size` in pure ink and paper, centred on `rect`.
///
/// For the status marks. Blended edges are what made them look soft: at three
/// millimetres a grey fringe is a third of every stroke, and the panel then
/// spends a greyscale waveform redrawing that fringe whenever the band
/// changes. Pixels at least half covered are inked and the rest left as
/// paper, which keeps every stroke edge on a pixel boundary and lets the band
/// update with the fast two-level waveform.
fn blit_vector_crisp(
    surface: &mut Surface,
    shapes: &[vector::Shape],
    size: i32,
    rect: Rect,
    clip: Rect,
) {
    blit_vector_crisp_in(surface, shapes, size, rect, clip, tone::INK);
}

/// The same, in a tone of the caller's choosing: paper for a mark reversed
/// out of an ink box, so ticking a row is a change of ink and paper alone.
fn blit_vector_crisp_in(
    surface: &mut Surface,
    shapes: &[vector::Shape],
    size: i32,
    rect: Rect,
    clip: Rect,
    paint: impl Into<Paint>,
) {
    if size <= 0 {
        return;
    }
    let paint = paint.into();
    let coverage = vector::render(shapes, size);
    let origin_x = rect.x + (rect.width - size) / 2;
    let origin_y = rect.y + (rect.height - size) / 2;
    for row in 0..size {
        for column in 0..size {
            if coverage.at(column, row) < 128 {
                continue;
            }
            let (x, y) = (origin_x + column, origin_y + row);
            if x < clip.x || y < clip.y || x >= clip.x + clip.width || y >= clip.y + clip.height {
                continue;
            }
            match paint {
                Paint::Grey(tone) => surface.blend(x, y, tone, 255),
                Paint::Colour(colour) => surface.blend_colour(x, y, colour, 255),
            }
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

fn fill_clipped(surface: &mut Surface, rect: Rect, paint: impl Into<Paint>, clip: Rect) {
    if let Some(rect) = rect.intersection(clip) {
        surface.fill_rect_paint(rect, paint.into());
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
fn stroke_clipped(
    surface: &mut Surface,
    rect: Rect,
    paint: impl Into<Paint>,
    thickness: i32,
    clip: Rect,
) {
    let paint = paint.into();
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
        fill_clipped(surface, edge, paint, clip);
    }
}

fn draw_morris_board(surface: &mut Surface, rect: Rect, metrics: &DisplayMetrics, clip: Rect) {
    let thickness = metrics.rule_thickness().max(2);
    let point = |column: i32, row: i32| {
        (
            rect.x + (2 * column + 1) * rect.width / 14,
            rect.y + (2 * row + 1) * rect.height / 14,
        )
    };
    let mut line = |from: (i32, i32), to: (i32, i32)| {
        let x = from.0.min(to.0);
        let y = from.1.min(to.1);
        let width = (from.0 - to.0).abs().max(thickness);
        let height = (from.1 - to.1).abs().max(thickness);
        fill_clipped(
            surface,
            Rect {
                x,
                y,
                width,
                height,
            },
            tone::RULE,
            clip,
        );
    };
    for inset in 0..3 {
        let far = 6 - inset;
        let top_left = point(inset, inset);
        let top_right = point(far, inset);
        let bottom_left = point(inset, far);
        let bottom_right = point(far, far);
        line(top_left, top_right);
        line(top_right, bottom_right);
        line(bottom_right, bottom_left);
        line(bottom_left, top_left);
    }
    line(point(3, 0), point(3, 2));
    line(point(3, 4), point(3, 6));
    line(point(0, 3), point(2, 3));
    line(point(4, 3), point(6, 3));
}

fn draw_backgammon_point(
    surface: &mut Surface,
    rect: Rect,
    from_top: bool,
    metrics: &DisplayMetrics,
    palette: Palette,
    clip: Rect,
) {
    let paper_point = (rect.x / rect.width.max(1)).rem_euclid(2) == 0;
    let fill = if paper_point {
        palette.board_dark()
    } else {
        Paint::Grey(tone::PAPER)
    };
    let height = rect.height.max(1);
    let centre = rect.x + rect.width / 2;
    for row in 0..height {
        let depth = if from_top { row } else { height - 1 - row };
        let half = (rect.width * (height - depth) / (2 * height)).max(1);
        fill_clipped(
            surface,
            Rect {
                x: centre - half,
                y: rect.y + row,
                width: half * 2,
                height: 1,
            },
            fill,
            clip,
        );
    }
    let rule = metrics.rule_thickness();
    let mut edge = |x0: i32, y0: i32, x1: i32, y1: i32| {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        for step in 0..=steps {
            fill_clipped(
                surface,
                Rect {
                    x: x0 + (x1 - x0) * step / steps,
                    y: y0 + (y1 - y0) * step / steps,
                    width: rule,
                    height: rule,
                },
                tone::RULE,
                clip,
            );
        }
    };
    let apex = if from_top {
        (centre, rect.y + height - 1)
    } else {
        (centre, rect.y)
    };
    let base_y = if from_top {
        rect.y
    } else {
        rect.y + height - 1
    };
    edge(rect.x, base_y, apex.0, apex.1);
    edge(rect.x + rect.width - 1, base_y, apex.0, apex.1);
}

fn draw_backgammon_board(surface: &mut Surface, rect: Rect, metrics: &DisplayMetrics, clip: Rect) {
    fill_clipped(surface, rect, tone::PAPER, clip);
    stroke_clipped(
        surface,
        rect,
        tone::RULE,
        metrics.rule_thickness() * 2,
        clip,
    );
    let bar_width = (rect.width / 28).max(metrics.rule_thickness() * 3);
    fill_clipped(
        surface,
        Rect {
            x: rect.x + (rect.width - bar_width) / 2,
            y: rect.y,
            width: bar_width,
            height: rect.height,
        },
        tone::MUTED,
        clip,
    );
    for (x, width) in [
        (rect.x, (rect.width / 24).max(1)),
        (
            rect.x + rect.width - (rect.width / 24).max(1),
            (rect.width / 24).max(1),
        ),
    ] {
        stroke_clipped(
            surface,
            Rect {
                x,
                y: rect.y + rect.height / 8,
                width,
                height: rect.height * 3 / 4,
            },
            tone::MUTED,
            metrics.rule_thickness(),
            clip,
        );
    }
}

fn draw_backgammon_stack(
    surface: &mut Surface,
    glyph: Glyph,
    count: u8,
    from_top: bool,
    rect: Rect,
    clip: Rect,
) {
    let side = (rect.width * 7 / 10).max(1);
    let step = (side * 2 / 3).max(1);
    for index in 0..i32::from(count) {
        let y = if from_top {
            rect.y + 4 + index * step
        } else {
            rect.y + rect.height - side - 4 - index * step
        };
        draw_vector(
            surface,
            &vector::shapes(glyph),
            Rect {
                x: rect.x + (rect.width - side) / 2,
                y,
                width: side,
                height: side,
            },
            clip,
            tone::INK,
        );
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
fn fill_rounded_clipped(
    surface: &mut Surface,
    rect: Rect,
    radius: i32,
    paint: impl Into<Paint>,
    clip: Rect,
) {
    let paint = paint.into();
    for row in 0..rect.height {
        if let Some(span) = rounded_row(rect, radius, row) {
            fill_clipped(surface, span, paint, clip);
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
    paint: impl Into<Paint>,
    thickness: i32,
    clip: Rect,
) {
    let paint = paint.into();
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
                    paint,
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
                    paint,
                    clip,
                );
            }
            None => fill_clipped(surface, outer, paint, clip),
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
    face: Face,
    clip: Rect,
) {
    let mut y = rect.y;
    let thickness = metrics.rule_thickness();
    for line in lines {
        draw_text_in(surface, line, rect.x, y, size, face, tone::MUTED, clip);
        let width = min(measure_text_in(line, size, face).0, rect.width);
        // Through the middle of the letters rather than the middle of the line
        // box, which sits under the baseline and reads as an underline.
        let middle = y
            .saturating_add(size.line_height_in(face) / 2)
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
        y = y.saturating_add(size.line_height_in(face));
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

/// The advance every digit is given when a figure has to hold still.
///
/// The text face spaces its digits proportionally: at body size a one is
/// fifteen pixels and a zero is twenty-four. That is right for a digit inside
/// a sentence, and wrong for the clock, which is the only text on the panel
/// that changes while everything around it stays. Going from 07:59 to 08:00
/// makes the figure wider, so the colon and the minutes step sideways, and the
/// panel repaints the whole field rather than the digits that actually
/// changed.
///
/// The face carries no tabular set to switch to, so the cell is synthesised:
/// the widest digit's advance, with each digit centred in it. Only figures
/// that tick are drawn this way. A one padded to the width of a zero stands in
/// a visible gap, which is a fair price for a clock that does not twitch and a
/// bad one for a chapter number in a title.
fn digit_cell(size: FontSize, face: Face) -> i32 {
    ('0'..='9')
        .map(|digit| measure_text_in(&digit.to_string(), size, face).0)
        .max()
        .unwrap_or(0)
}

/// The width `draw_figures` will take, which does not depend on which digits
/// these are.
fn figures_width(text: &str, size: FontSize, face: Face) -> i32 {
    let cell = digit_cell(size, face);
    text.chars().fold(0, |width, character| {
        let advance = if character.is_ascii_digit() {
            cell
        } else {
            measure_text_in(&character.to_string(), size, face).0
        };
        width.saturating_add(advance)
    })
}

/// Draws a figure with its digits on a common advance, so that it counts
/// without moving.
///
/// Everything that is not a digit keeps the width the face gave it, so a colon
/// stays as tight as it was drawn to be.
#[allow(clippy::too_many_arguments)]
fn draw_figures(
    surface: &mut Surface,
    text: &str,
    x: i32,
    y: i32,
    size: FontSize,
    face: Face,
    tone: u8,
    clip: Rect,
) {
    let cell = digit_cell(size, face);
    let mut pen = x;
    let mut buffer = [0_u8; 4];
    for character in text.chars() {
        let glyph = &*character.encode_utf8(&mut buffer);
        let natural = measure_text_in(glyph, size, face).0;
        if character.is_ascii_digit() {
            draw_text_in(
                surface,
                glyph,
                pen + (cell - natural) / 2,
                y,
                size,
                face,
                tone,
                clip,
            );
            pen = pen.saturating_add(cell);
        } else {
            draw_text_in(surface, glyph, pen, y, size, face, tone, clip);
            pen = pen.saturating_add(natural);
        }
    }
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
    if with_typesetter(face, |typesetter| {
        typesetter.draw(text, x, y, size, face, &mut |pixel_x, pixel_y, coverage| {
            if coverage > 0 && clip.contains(pixel_x, pixel_y) {
                surface.blend(pixel_x, pixel_y, tone, coverage);
            }
        });
    })
    .is_some()
    {
        return;
    }
    draw_fallback_text(surface, text, x, y, size, tone, clip);
}

#[allow(clippy::too_many_arguments)]
fn draw_rich_text(
    surface: &mut Surface,
    text: &str,
    rect: Rect,
    face: Face,
    presentation: TextPresentation,
    tone: u8,
    clip: Rect,
) {
    let line = FontSize::Body.line_height_in(face).max(1);
    let vertical = if presentation.superscript {
        -line / 4
    } else if presentation.subscript {
        line / 5
    } else {
        0
    };
    let y = rect.y.saturating_add(vertical);
    if with_typesetter(face, |typesetter| {
        typesetter.draw(
            text,
            rect.x,
            y,
            FontSize::Body,
            face,
            &mut |pixel_x, pixel_y, coverage| {
                let skew = if presentation.emphasis {
                    (line - (pixel_y - y)).clamp(0, line) / 7
                } else {
                    0
                };
                let x = pixel_x.saturating_add(skew);
                if coverage > 0 && clip.contains(x, pixel_y) {
                    surface.blend(x, pixel_y, tone, coverage);
                    if presentation.strong && clip.contains(x + 1, pixel_y) {
                        surface.blend(x + 1, pixel_y, tone, coverage);
                    }
                }
            },
        );
    })
    .is_none()
    {
        draw_fallback_text(surface, text, rect.x, y, FontSize::Body, tone, clip);
        if presentation.strong {
            draw_fallback_text(surface, text, rect.x + 1, y, FontSize::Body, tone, clip);
        }
    }
    if presentation.underline {
        fill_clipped(
            surface,
            Rect {
                x: rect.x,
                y: rect.y.saturating_add(line).saturating_sub(2),
                width: rect.width,
                height: 1,
            },
            tone,
            clip,
        );
    }
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
mod tests;

#[cfg(test)]
mod page_turn_tests;

#[cfg(test)]
mod chrome_tests;

#[cfg(test)]
mod row_tests;

#[cfg(test)]
mod controls_tests;

#[cfg(test)]
mod tile_tests;

#[cfg(test)]
mod choice_tests;

#[cfg(test)]
mod loading_tests;

#[cfg(test)]
mod prose_tests;

#[cfg(test)]
mod orientation_tests;

#[cfg(test)]
mod press_feedback_tests;

#[cfg(test)]
mod figure_tests;

#[cfg(test)]
#[path = "colour_orientation_tests.rs"]
mod colour_orientation_tests;

#[cfg(test)]
mod colour_tests;
