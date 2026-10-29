//! Colour, for the panels that have it, as a rendering of the grey design.
//!
//! Cobalt was drawn in grey first and every screen still is. The grey plane is
//! what a Clara BW shows, what an application measures against, and what the
//! frame planner reasons about. A colour panel does not get a different
//! design; it gets the same screen with a handful of its roles painted in a
//! colour whose luminance stands in for the grey they would otherwise have
//! been. Nothing here moves a pixel: layout never sees colour, so a page an
//! application paginated on its own is the same page on both kinds of panel.
//!
//! The panels in question are Kaleido 3, which is what every colour Kobo
//! carries: sixteen greys under a printed colour filter, 4096 colours, and
//! colour at half the resolution of the greys. Three things follow and shape
//! everything in this module. Colours are pale, because the filter sits over
//! ink that is never brighter than paper, so a palette tuned for a phone comes
//! out as mud; every shade below is one of the 4096 the panel can resolve, so
//! what is sent is what is drawn rather than what a controller rounds it to.
//! Colour is coarse, so it goes on bands and fills a few millimetres across
//! and never on text, whose tone is always decided by what it sits on. And
//! colour is never the only signal: a banner keeps its mark and a selection
//! keeps its bar, because a reader who cannot tell red from green, or whose
//! panel has faded, loses nothing.

use crate::{tone, BannerLevel};

/// How the panel in front of the painter shows the roles a screen is drawn in.
///
/// Runtime-owned, like the rest of [`crate::Chrome`], and never on the wire:
/// an application does not know, and does not need to know, which panel it is
/// drawn on. The default is grey so that every caller that measures or paints
/// without saying otherwise gets exactly the frame it got before colour
/// existed, byte for byte, with no colour plane allocated.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Appearance {
    /// Every role in the grey it has always been drawn in.
    #[default]
    Grey,
    /// Roles that carry a colour are drawn in it. The grey plane keeps each
    /// colour's luminance, so a greyscale reading of the frame is the grey
    /// design and the planner's grey arithmetic still holds.
    Colour,
}

/// A colour an application may ask for, by name and never by value.
///
/// Closed, like the glyph set, and for the same reason: an application that
/// could send a colour could send an illegible one, and nothing short of
/// drawing every screen on every panel would catch it. Each name carries two
/// shades chosen here, once, against the panel: a tint for a band that carries
/// ink text, and a strong shade for a fill that carries paper text, each with
/// at least seven to one contrast against its text before the filter takes
/// its share.
///
/// An accent is the application's own colour, the way Hacker News is orange,
/// and it is one decision rather than several: the top bar, the primary
/// control and every selected state follow it together, so a Store of
/// applications reads as one platform rather than a bag of themes. A screen
/// that names none is drawn in the platform's own [`Accent::Cobalt`], with
/// its bar left on paper. A greyscale panel ignores the accent entirely.
///
/// The wire value is the discriminant, from one; zero on the wire means no
/// accent. A runtime that reads a value it has no name for draws grey rather
/// than refusing the screen, because grey is the right answer to every accent
/// and an application should not stop opening over the colour of its bar.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum Accent {
    /// The platform's own blue, from the logo. What every screen is drawn in
    /// when its application has no colour of its own.
    Cobalt = 1,
    Red = 2,
    Orange = 3,
    Amber = 4,
    Green = 5,
    Teal = 6,
    Blue = 7,
    Purple = 8,
    Brown = 9,
}

/// The two shades every accent is made of.
///
/// Two and not one, because one colour cannot carry both ink and paper text at
/// a contrast a reflective panel needs. The tint is light enough for ink and
/// the strong shade dark enough for paper, and the painter never puts either
/// under the other kind of text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Shades {
    /// For a band with ink text on it: the top bar, a banner.
    pub tint: [u8; 3],
    /// For a fill with paper text on it, and for marks: the primary control,
    /// a selected destination, a switch that is on.
    pub strong: [u8; 3],
}

/// The sixteen levels a Kaleido 3 panel resolves in each channel, as the
/// 8-bit values that land exactly on them.
///
/// A channel value that is not one of these is rounded by the controller to
/// the nearest that is, so a shade built from any other values is not the
/// shade that reaches the panel. Every shade in this module is checked
/// against this at compile time.
pub const PANEL_LEVEL: u8 = 17;

const fn on_panel(colour: [u8; 3]) -> bool {
    colour[0] % PANEL_LEVEL == 0 && colour[1] % PANEL_LEVEL == 0 && colour[2] % PANEL_LEVEL == 0
}

const fn shades(tint: [u8; 3], strong: [u8; 3]) -> Shades {
    assert!(
        on_panel(tint),
        "a tint must be one of the panel's 4096 colours"
    );
    assert!(
        on_panel(strong),
        "a strong shade must be one of the panel's 4096 colours"
    );
    Shades { tint, strong }
}

impl Accent {
    /// Every accent, in wire order, so a test can hold a rule over all of them.
    pub const ALL: [Self; 9] = [
        Self::Cobalt,
        Self::Red,
        Self::Orange,
        Self::Amber,
        Self::Green,
        Self::Teal,
        Self::Blue,
        Self::Purple,
        Self::Brown,
    ];

    /// The accent a wire byte names, or `None` for zero and for any value this
    /// build has no name for. See the type's note on why unknown is not an
    /// error.
    #[must_use]
    pub const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Cobalt),
            2 => Some(Self::Red),
            3 => Some(Self::Orange),
            4 => Some(Self::Amber),
            5 => Some(Self::Green),
            6 => Some(Self::Teal),
            7 => Some(Self::Blue),
            8 => Some(Self::Purple),
            9 => Some(Self::Brown),
            _ => None,
        }
    }

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    /// The name an accent is written under in a manifest or a diagnostic.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cobalt => "cobalt",
            Self::Red => "red",
            Self::Orange => "orange",
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Teal => "teal",
            Self::Blue => "blue",
            Self::Purple => "purple",
            Self::Brown => "brown",
        }
    }

    /// The accent's two shades.
    ///
    /// The tints sit between a fifth and a half of the way down from paper and
    /// the strong shades in the bottom third, which is where the contrast
    /// rules in the tests put them. Hues were chosen to stay apart from one
    /// another after the filter has flattened them: a warm red, an orange that
    /// is not a red, an amber that is not an orange, and a blue kept away from
    /// the platform's cobalt.
    #[must_use]
    pub const fn shades(self) -> Shades {
        match self {
            Self::Cobalt => shades([0x66, 0x99, 0xDD], [0x11, 0x44, 0xAA]),
            Self::Red => shades([0xFF, 0x77, 0x66], [0x99, 0x11, 0x11]),
            Self::Orange => shades([0xFF, 0x88, 0x33], [0x99, 0x33, 0x00]),
            Self::Amber => shades([0xFF, 0xCC, 0x33], [0x77, 0x44, 0x00]),
            Self::Green => shades([0x88, 0xDD, 0x88], [0x11, 0x66, 0x22]),
            Self::Teal => shades([0x77, 0xDD, 0xDD], [0x00, 0x55, 0x55]),
            Self::Blue => shades([0x88, 0xBB, 0xFF], [0x11, 0x33, 0x88]),
            Self::Purple => shades([0xBB, 0x99, 0xEE], [0x44, 0x22, 0x88]),
            Self::Brown => shades([0xDD, 0xBB, 0x99], [0x55, 0x33, 0x11]),
        }
    }
}

/// What one role is painted with: a grey, or a colour.
///
/// A colour paint carries its own luminance into the grey plane, so a frame
/// is one picture whichever plane is read. Greys convert from the tone
/// constants, so every painter call that still names a tone is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Paint {
    Grey(u8),
    Colour([u8; 3]),
}

impl From<u8> for Paint {
    fn from(tone: u8) -> Self {
        Self::Grey(tone)
    }
}

impl Paint {
    /// The grey this paint lands in the grey plane as.
    #[must_use]
    pub const fn luma(self) -> u8 {
        match self {
            Self::Grey(value) => value,
            Self::Colour(colour) => crate::luma(colour),
        }
    }
}

/// The roles of one screen resolved for one panel.
///
/// Built once per paint from the chrome's appearance and the screen's accent,
/// so every arm of the painter asks for a role and none of them decides for
/// itself whether the panel has colour. On a grey panel every role answers
/// with the tone it has always been, which is what keeps the greyscale frame
/// identical to the one drawn before this type existed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Palette {
    appearance: Appearance,
    /// The screen's own accent, or `None` for the platform's.
    accent: Option<Accent>,
}

impl Palette {
    #[must_use]
    pub const fn new(appearance: Appearance, accent: Option<Accent>) -> Self {
        Self { appearance, accent }
    }

    /// Every role in grey, whatever the screen asked for.
    #[must_use]
    pub const fn grey() -> Self {
        Self::new(Appearance::Grey, None)
    }

    const fn colour(self) -> bool {
        matches!(self.appearance, Appearance::Colour)
    }

    const fn shades(self) -> Shades {
        match self.accent {
            Some(accent) => accent.shades(),
            None => Accent::Cobalt.shades(),
        }
    }

    /// The top bar's field. Paper unless the screen named an accent: the
    /// platform's own screens keep a quiet bar, and only an application with a
    /// colour of its own wears it across the top.
    #[must_use]
    pub const fn bar(self) -> Paint {
        match self.accent {
            Some(accent) if self.colour() => Paint::Colour(accent.shades().tint),
            _ => Paint::Grey(tone::PAPER),
        }
    }

    /// The one loud thing on the screen, and the marks that say "this one":
    /// the primary control's fill, the selected destination's bar, a chip
    /// that is on, a switch that is on, a ticked box, a chosen option, the
    /// filled part of a progress bar, the steps already taken.
    ///
    /// Ink on a grey panel. The accent's strong shade on a colour panel, and
    /// the platform's cobalt when the screen has no accent, so that a control
    /// is the same control in every application.
    #[must_use]
    pub const fn emphasis(self) -> Paint {
        if self.colour() {
            Paint::Colour(self.shades().strong)
        } else {
            Paint::Grey(tone::INK)
        }
    }

    /// A banner's field, by what it is telling the reader.
    ///
    /// These are the platform's colours rather than the application's, for
    /// the same reason every phone draws its own red for an error: a reader
    /// learns them once. A neutral banner stays on the quiet surface it has
    /// always sat on, and on a grey panel so do all of them.
    #[must_use]
    pub const fn banner(self, level: BannerLevel) -> Paint {
        if !self.colour() {
            return Paint::Grey(tone::SURFACE);
        }
        match level {
            BannerLevel::Info => Paint::Grey(tone::SURFACE),
            BannerLevel::Attention => Paint::Colour(Accent::Amber.shades().tint),
            BannerLevel::Success => Paint::Colour(Accent::Green.shades().tint),
            BannerLevel::Error => Paint::Colour(Accent::Red.shades().tint),
        }
    }

    /// The heavy bar down an attention banner's leading edge. Ink on a grey
    /// panel; amber's strong shade on a colour one, so the edge and the field
    /// are one colour at two weights rather than a black bar on yellow.
    #[must_use]
    pub const fn banner_edge(self) -> Paint {
        if self.colour() {
            Paint::Colour(Accent::Amber.shades().strong)
        } else {
            Paint::Grey(tone::INK)
        }
    }
}
