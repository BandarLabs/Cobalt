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

/// The most rows one list may declare.
///
/// A bound exists so a screen cannot become unboundedly tall from data; a list
/// longer than this wants paging, which is a different primitive.
pub const MAX_ROWS: usize = 32;

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
}

/// The only panel with hardware support today.
pub const CLARA_BW_METRICS: DisplayMetrics = DisplayMetrics {
    width: 1072,
    height: 1448,
    pixels_per_inch: 300,
};

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

    #[must_use]
    pub const fn top_bar_height(&self) -> i32 {
        self.tenth_mm(110)
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

/// A single tappable label in a bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BarAction {
    pub action: ActionId,
    pub label: String,
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

/// The fixed bar at the bottom of a screen, equivalent to the reader's own.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NavBar {
    pub id: NodeId,
    pub destinations: Vec<BarAction>,
    pub selected: usize,
}

impl NavBar {
    #[must_use]
    pub fn new(id: NodeId, destinations: Vec<BarAction>, selected: usize) -> Self {
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
            page_turns: None,
        }
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
        let mut layout = Layout::default();

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
        let content_bottom = self.nav_bar.as_ref().map_or(metrics.height, |_| {
            metrics.height - metrics.nav_bar_height()
        });

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
                &mut layout,
            );
            cursor = cursor.saturating_add(gap);
        }

        if let Some(nav_bar) = &self.nav_bar {
            layout_nav_bar(nav_bar, metrics, &mut layout);
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

    let control = metrics.touch_target_default();
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

    let title = wrap_text(&top_bar.title, title_width, FontSize::Title);
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
        text_lines: vec![title.into_iter().next().unwrap_or_default()],
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
            kind: if index == nav_bar.selected {
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
    Button {
        id: NodeId,
        action: ActionId,
        label: String,
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
}

/// One tile in a [`Node::TileGrid`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tile {
    pub action: ActionId,
    pub label: String,
    pub glyph: Glyph,
}

impl Tile {
    #[must_use]
    pub fn new(action: ActionId, label: impl Into<String>, glyph: Glyph) -> Self {
        Self {
            action,
            label: label.into(),
            glyph,
        }
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
    pub glyph: Glyph,
}

impl Row {
    #[must_use]
    pub fn new(
        action: ActionId,
        title: impl Into<String>,
        summary: impl Into<String>,
        glyph: Glyph,
    ) -> Self {
        Self {
            action,
            title: title.into(),
            summary: summary.into(),
            glyph,
        }
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
}

impl Node {
    #[must_use]
    pub const fn id(&self) -> NodeId {
        match self {
            Self::Heading { id, .. }
            | Self::Text { id, .. }
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
            | Self::Activity { id, .. } => *id,
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
    Button(ActionId),
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
    RowSummary,
    RowGlyph(Glyph),
    Tile(ActionId),
    TileLabel,
    TileGlyph(Glyph),
    ChoicePrompt,
    ChoiceOption(ActionId),
    ChoiceFreeform(ActionId),
    Banner(BannerLevel),
    Skeleton,
    ActivityLabel,
    ActivityProgress,
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
}

impl Layout {
    #[must_use]
    pub fn hit_test(&self, x: i32, y: i32) -> Option<ActionId> {
        // Controls first, always. A page turn is what a tap means when it
        // means nothing else, so a button, a row or a keyboard key can never
        // be shadowed by a zone sitting underneath it.
        if let Some(action) = self.hit_control(x, y) {
            return Some(action);
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
            LayoutKind::Button(action)
            | LayoutKind::BarAction(action)
            | LayoutKind::NavDestination(action)
            | LayoutKind::NavDestinationSelected(action)
            | LayoutKind::Tile(action)
            | LayoutKind::Row(action)
            | LayoutKind::Cell(action)
            | LayoutKind::ChoiceOption(action)
            | LayoutKind::ChoiceFreeform(action)
                if node.rect.contains(x, y) =>
            {
                Some(action)
            }
            LayoutKind::Back if node.rect.contains(x, y) => Some(ActionId::BACK),
            _ => None,
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
                LayoutKind::Button(candidate)
                | LayoutKind::BarAction(candidate)
                | LayoutKind::NavDestination(candidate)
                | LayoutKind::NavDestinationSelected(candidate)
                | LayoutKind::Tile(candidate)
                | LayoutKind::ChoiceOption(candidate)
                | LayoutKind::Cell(candidate)
                | LayoutKind::ChoiceFreeform(candidate) => candidate == action,
                _ => false,
            })
            .map(|node| node.rect)
    }
}

fn layout_node(
    node: &Node,
    x: i32,
    y: i32,
    width: i32,
    depth: usize,
    metrics: &DisplayMetrics,
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
            let lines = wrap_text(text, width, FontSize::Body);
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
                kind: LayoutKind::Text,
                text_lines: lines,
            });
            y.saturating_add(height)
        }
        Node::Button { id, action, label } => {
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
                kind: LayoutKind::Button(*action),
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
                    kind: LayoutKind::RowGlyph(row.glyph),
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
                    kind: LayoutKind::RowTitle,
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
        Node::TileGrid { id, tiles } => {
            let columns = metrics.max_grid_columns() as i32;
            let gutter = metrics.space(Space::Small);
            let cell = (width - gutter * (columns - 1)) / columns;
            // Square cells, plus a band beneath for the label. A tile shorter
            // than it is wide reads as a button, not a destination.
            let label_band = FontSize::Caption.line_height() + metrics.space(Space::Tight);
            let cell_height = cell.saturating_add(label_band);
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
                let glyph_size = metrics.tenth_mm(110);
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x + (cell - glyph_size) / 2,
                        y: cell_y + (cell - glyph_size) / 2,
                        width: glyph_size,
                        height: glyph_size,
                    },
                    kind: LayoutKind::TileGlyph(tile.glyph),
                    text_lines: Vec::new(),
                });
                let label = wrap_text(&tile.label, cell, FontSize::Caption)
                    .into_iter()
                    .next()
                    .unwrap_or_default();
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x: cell_x,
                        y: cell_y + cell + metrics.space(Space::Tight),
                        width: cell,
                        height: FontSize::Caption.line_height(),
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
            for option in options.iter().take(MAX_CHOICE_OPTIONS) {
                if layout.nodes.len() >= MAX_LAYOUT_NODES {
                    break;
                }
                layout.nodes.push(LayoutNode {
                    id: *id,
                    rect: Rect {
                        x,
                        y: cursor,
                        width,
                        height: row_height,
                    },
                    kind: LayoutKind::ChoiceOption(option.action),
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
    }
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
        TYPESETTER
            .get()
            .map_or_else(|| self.fallback_line_height(), |t| t.line_height(self))
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

/// Supplies real type to the layout and the renderer.
///
/// This layer knows what a heading is; it does not know what a font file is.
/// The runtime installs one implementation at startup, which is why the
/// application-facing size is a semantic name rather than a pixel count and
/// why replacing the typeface changes no application code at all.
pub trait Typesetter: Send + Sync {
    /// The width and height in pixels that `text` will occupy.
    fn measure(&self, text: &str, size: FontSize) -> (i32, i32);
    /// The baseline-to-baseline distance for a size.
    fn line_height(&self, size: FontSize) -> i32;
    /// Draws `text` with its top-left corner at `x`, `y`.
    ///
    /// Coverage runs from 0 for untouched to 255 for solid, so a renderer can
    /// antialias against whatever it is drawing onto.
    fn draw(&self, text: &str, x: i32, y: i32, size: FontSize, plot: &mut dyn FnMut(i32, i32, u8));
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
    if let Some(typesetter) = TYPESETTER.get() {
        return typesetter.measure(text, size);
    }
    let scale = size.scale();
    let glyphs = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
    (glyphs.saturating_mul(6).saturating_mul(scale), 7 * scale)
}

/// The width of one average character, used to wrap without measuring every
/// candidate line.
fn average_advance(size: FontSize) -> i32 {
    TYPESETTER.get().map_or(6 * size.scale(), |typesetter| {
        // Measuring a representative run is closer to the truth than any one
        // character, and proportional type has no single answer.
        const SAMPLE: &str = "abcdefghijklmnopqrstuvwxyz";
        let (width, _) = typesetter.measure(SAMPLE, size);
        max(1, width / 26)
    })
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
}

impl DisplayMetrics {
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
    let line_height = FontSize::Body.line_height();
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut page: Vec<String> = Vec::new();
    let mut used = 0;
    if area.width <= 0 || area.height < line_height {
        return pages;
    }

    // Line endings are normalised first. Project Gutenberg serves CRLF, so a
    // split on "\n\n" alone never matched and an entire novel arrived as one
    // paragraph: a solid wall of text with no space anywhere in it. A lone CR
    // is folded too, because some of the older files use it.
    let text = normalise_breaks(text);
    for paragraph in text.split("\n\n") {
        // Line breaks inside a paragraph are the source file's, not the
        // author's; Gutenberg's plain text is hard wrapped at seventy columns
        // and honouring that would give a column of ragged short lines.
        let paragraph = paragraph.replace('\n', " ");
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }
        let mut lines = wrap_text(paragraph, area.width, FontSize::Body);
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
                page.push(lines.join(" "));
                break;
            }
            // A paragraph longer than a whole page cannot be kept whole. It is
            // split rather than dropped, because a book whose preface is one
            // enormous block would otherwise open at chapter two.
            if page.is_empty() {
                let rest = lines.split_off(fits);
                page.push(lines.join(" "));
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

#[must_use]
pub fn paginate_rows(
    rows: &[(&str, &str)],
    metrics: &DisplayMetrics,
    area: ProseArea,
) -> Vec<Vec<usize>> {
    let padding = metrics.space(Space::Small);
    let icon = metrics.touch_target_default();
    let text_width = max(1, area.width - icon - padding * 2);
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

/// Wraps at word boundaries and splits exceptionally long words deterministically.
#[must_use]
pub fn wrap_text(text: &str, max_width: i32, size: FontSize) -> Vec<String> {
    if text.is_empty() || max_width <= 0 {
        return vec![String::new()];
    }
    let max_chars = max(1, max_width / average_advance(size)) as usize;
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = word.to_owned();
        }
        while current.chars().count() > max_chars {
            let head = current.chars().take(max_chars).collect::<String>();
            let tail = current.chars().skip(max_chars).collect::<String>();
            lines.push(head);
            current = tail;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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

/// Rasterizes a retained screen. `dirty` limits writes to a changed rectangle when supplied.
pub fn render(screen: &Screen, surface: &mut Surface, dirty: Option<Rect>) {
    render_with(screen, &CLARA_BW_METRICS, Chrome::default(), surface, dirty);
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
    let clip = dirty.unwrap_or(Rect {
        x: 0,
        y: 0,
        width: i32::try_from(surface.width).unwrap_or(i32::MAX),
        height: i32::try_from(surface.height).unwrap_or(i32::MAX),
    });
    surface.fill_rect(clip, tone::PAPER);
    for node in screen.layout_with(metrics, chrome).nodes {
        if node.rect.intersection(clip).is_none() {
            continue;
        }
        match node.kind {
            LayoutKind::Card => {
                fill_clipped(surface, node.rect, tone::SURFACE, clip);
                stroke_clipped(surface, node.rect, tone::RULE, clip);
            }
            LayoutKind::Button(_) => {
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
            // The cell is outlined rather than filled, so a board reads as
            // ruled squares and an empty cell stays paper white. Filling would
            // make every move a full-cell change, which is slow on E Ink and
            // looks like a mistake.
            LayoutKind::Cell(_) => stroke_clipped(surface, node.rect, tone::RULE, clip),
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
                stroke_clipped(surface, node.rect, tone::INK, clip);
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
            LayoutKind::Text | LayoutKind::PagedList => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Body,
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
            LayoutKind::Tile(_) => stroke_clipped(surface, node.rect, tone::RULE, clip),
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
            LayoutKind::RowSummary => draw_lines(
                surface,
                &node.text_lines,
                node.rect.x,
                node.rect.y,
                FontSize::Caption,
                tone::MUTED,
                clip,
            ),
            LayoutKind::RowGlyph(glyph) => draw_glyph_icon(surface, glyph, node.rect, clip),
            LayoutKind::TileGlyph(glyph) => draw_glyph_icon(surface, glyph, node.rect, clip),
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
            LayoutKind::ChoiceOption(_) => {
                stroke_clipped(surface, node.rect, tone::INK, clip);
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
            }
            // Outlined in a lighter tone and set in muted ink, so the escape
            // hatch reads as secondary to the options above it.
            LayoutKind::ChoiceFreeform(_) => {
                stroke_clipped(surface, node.rect, tone::RULE, clip);
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
                stroke_clipped(surface, node.rect, tone::RULE, clip);
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

fn draw_back_arrow(surface: &mut Surface, rect: Rect, clip: Rect) {
    draw_vector(surface, &vector::back_arrow(), rect, clip);
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

fn stroke_clipped(surface: &mut Surface, rect: Rect, tone: u8, clip: Rect) {
    for edge in [
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: 1,
        },
        Rect {
            x: rect.x,
            y: rect.y.saturating_add(rect.height).saturating_sub(1),
            width: rect.width,
            height: 1,
        },
        Rect {
            x: rect.x,
            y: rect.y,
            width: 1,
            height: rect.height,
        },
        Rect {
            x: rect.x.saturating_add(rect.width).saturating_sub(1),
            y: rect.y,
            width: 1,
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
    mut y: i32,
    size: FontSize,
    tone: u8,
    clip: Rect,
) {
    for line in lines {
        draw_text(surface, line, x, y, size, tone, clip);
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
    if let Some(typesetter) = TYPESETTER.get() {
        typesetter.draw(text, x, y, size, &mut |pixel_x, pixel_y, coverage| {
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
            },
        ),
        (
            "libra-2",
            DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
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
        (
            "elipsa",
            DisplayMetrics {
                width: 1404,
                height: 1872,
                pixels_per_inch: 227,
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
            }],
        )
        .with_page_turns(ActionId(10), ActionId(20));
        let layout = screen.layout();
        let button = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::Button(_)))
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
            0,
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
        let screen =
            Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(NodeId(1), destinations(3), 0));
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
            let screen =
                Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(NodeId(1), destinations(5), 0));
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
            Screen::new(1, nodes).with_nav_bar(NavBar::new(NodeId(99), destinations(3), 0));
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
        let screen =
            Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(NodeId(1), destinations(3), 2));
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
    fn a_list_is_denser_than_the_same_entries_as_tiles() {
        // The reason this primitive exists: three tiles filled the panel.
        let metrics = CLARA_BW_METRICS;
        let rows = list(3, "A one line summary of the entry.").layout_for(&metrics);
        let tiles = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                tiles: (0..3)
                    .map(|index| Tile::new(ActionId(index + 1), "Entry", Glyph::App))
                    .collect(),
            }],
        )
        .layout_for(&metrics);
        let (Some(rows), Some(tiles)) = (rows.bounds(), tiles.bounds()) else {
            panic!("both layouts should have bounds");
        };
        assert!(
            rows.height * 2 < tiles.height,
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
                first_row <= metrics.max_grid_columns(),
                "{name}: {first_row} tiles on a row, budget is {}",
                metrics.max_grid_columns()
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
                    LayoutKind::ChoiceOption(_) | LayoutKind::ChoiceFreeform(_)
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
            .rposition(|node| matches!(node.kind, LayoutKind::ChoiceOption(_)))
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
            .filter(|node| matches!(node.kind, LayoutKind::ChoiceOption(_)))
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
                    LayoutKind::ChoiceOption(_) | LayoutKind::ChoiceFreeform(_)
                )
            })
            .collect::<Vec<_>>();
        for (index, row) in rows.iter().enumerate() {
            for other in rows.iter().skip(index + 1) {
                assert!(row.rect.intersection(other.rect).is_none(), "rows overlap");
            }
            let (LayoutKind::ChoiceOption(expected) | LayoutKind::ChoiceFreeform(expected)) =
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
                id: NodeId(12),
                tiles: vec![Tile::new(ActionId(12), "Tile", Glyph::App)],
            },
            Node::Choice {
                id: NodeId(13),
                prompt: "Pick one".into(),
                options: vec![BarAction::new(ActionId(13), "Option")],
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
                    },
                ],
            );
            let layout = screen.layout();
            let after = layout
                .nodes
                .iter()
                .find(|candidate| candidate.kind == LayoutKind::Button(ActionId(900)))
                .expect("the following button was laid out")
                .rect;
            for other in &layout.nodes {
                if other.kind == LayoutKind::Button(ActionId(900)) {
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
                LayoutKind::Button(action)
                | LayoutKind::BarAction(action)
                | LayoutKind::Tile(action)
                | LayoutKind::Row(action)
                | LayoutKind::Cell(action)
                | LayoutKind::ChoiceOption(action)
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
                usize::MAX,
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
            },
        );
        assert!(pages.is_empty());
    }
}
