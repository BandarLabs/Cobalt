#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

//! Real type for the panel.
//!
//! # Why this is a separate crate
//!
//! `kobo-ui` decides what a heading is; this crate decides what a heading looks
//! like. Keeping the split means the layout engine, the SDK, the protocol and
//! every application stay free of external dependencies, and the rasteriser can
//! be replaced without any of them changing. This is the same containment used
//! for `kobo-net`, and for the same reason: one crate carries the risk.
//!
//! # Why the device's own font
//!
//! The panel is 300 pixels per inch. The built-in fallback in `kobo-ui` is a
//! 5x7 bitmap with no lowercase at all, which is legible on a terminal and
//! insulting on an e-reader. The device already ships 47 TrueType faces,
//! including Atkinson Hyperlegible, which the Braille Institute designed
//! specifically so that similar letterforms cannot be confused. Reading a file
//! that is already on the device is not redistribution, so there is no
//! licensing question to answer, and there is nothing extra to install.
//!
//! If no face is found, `kobo-ui` keeps its fallback rather than failing. Text
//! that is ugly is better than an application that will not start.
//!
//! # Why the monospace face is embedded instead
//!
//! The same argument does not survive contact with the device. Of the 40-odd
//! faces the firmware ships, **not one is monospaced**, checked, not assumed.
//! A character grid cannot be faked from a proportional face: forcing a common
//! advance leaves `i` swimming in space and `m` touching its neighbours, and a
//! terminal is precisely where column alignment carries meaning.
//!
//! So this one face travels with us. `DejaVu Sans Mono` is redistributable, and
//! its licence is shipped beside it in `fonts/`. It also covers the box-drawing
//! block, which is what stops a full-screen program drawing its frame as a
//! column of question marks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fontdue::{Font, FontSettings};
use kobo_ui::{BreakOpportunity, DisplayMetrics, Face, FontSize, Typesetter};
use unicode_linebreak::{linebreaks, BreakOpportunity as UnicodeBreakOpportunity};
use unicode_segmentation::UnicodeSegmentation;

/// The monospace face, compiled in because the device has none.
///
/// See `fonts/LICENSE-DejaVu.txt`, which travels with it.
pub const MONO_FONT: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");

/// Faces to look for on the device, best first.
///
/// Atkinson Hyperlegible leads because it was drawn for legibility rather than
/// for style. The rest are ordinary, well-hinted text faces, so a firmware that
/// drops one still produces a readable interface.
pub const DEVICE_FONT_CANDIDATES: &[&str] = &[
    // Both names verified present on firmware 4.45.23697 rather than guessed.
    "/usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts/AtkinsonHyperlegible-Regular.ttf",
    "/usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts/Ubuntu-Regular.ttf",
];

/// The proportional face, compiled in so that every machine agrees.
///
/// This is the same face the device carries, taken from upstream rather than
/// from the device, under the SIL Open Font License; see
/// `fonts/LICENSE-AtkinsonHyperlegible.txt`, which travels with it.
///
/// It exists because the simulator used to fall back to whatever the developer
/// happened to have installed (Verdana on a Mac, `DejaVu` on a Linux box) so two
/// people previewing the same screen saw different line breaks, and neither
/// saw the device's. A compiled-in face makes the preview identical everywhere
/// and close to the panel. The device's own copy still wins when it is
/// present, because that one is exact by definition.
pub const TEXT_FONT: &[u8] = include_bytes!("../fonts/AtkinsonHyperlegible-Regular.ttf");

/// Faces to look for for prose, best first.
///
/// A different job from the interface, so a different list. The interface face
/// is chosen so a label glanced at once cannot be misread; a book is read in
/// sequence for an hour and wants type the eye stops noticing. Every one of
/// these is a serif drawn for continuous reading, and all of them are already
/// on the device.
///
/// `KoboNickel` leads because it is the face the reader's own software sets
/// books in, so a book here looks like a book there. Bitter follows: a slab
/// serif drawn specifically for screens, whose sturdy stems survive a panel
/// that resolves few tones. Vollkorn is a classic book face and the last serif
/// resort. Nothing is guessed, every path was listed off the device.
///
/// If none is present the interface face is used, which is legible and merely
/// not bookish.
pub const READING_FONT_CANDIDATES: &[&str] = &[
    // Bitter first because it was drawn for screens at reading sizes, and it is
    // on the device already.
    //
    // KoboNickel.ttf is deliberately absent. It is the obvious choice by name --
    // it is the face the stock reader sets books in -- but on firmware
    // 4.45.23697 the file begins `51 54 44 00`, "QTD\0", followed by a zlib
    // header. It is a Qt Embedded font blob, not a TrueType file, whatever the
    // extension says, and no outline parser can read it. Checking that a font
    // exists is not the same as checking that it is a font; this list is now
    // the record of which is which.
    "/usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts/Bitter-Regular.ttf",
    "/usr/local/Trolltech/QtEmbedded-4.6.2-arm/lib/fonts/Vollkorn-Regular.ttf",
];

/// The environment variable that overrides the prose face alone.
pub const READING_FONT_OVERRIDE: &str = "KOBO_READING_FONT";

/// The environment variable that overrides every search.
pub const FONT_OVERRIDE: &str = "KOBO_FONT";

/// What went wrong while loading a face.
#[derive(Debug)]
pub enum Error {
    /// No candidate path existed. Only reachable if the compiled-in face is
    /// removed, which is why nothing constructs it any more.
    NoFontFound,
    /// The file could not be read.
    Unreadable(PathBuf, std::io::Error),
    /// The file was not a font this rasteriser understands.
    Malformed(PathBuf, String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoFontFound => write!(formatter, "no usable font was found on this device"),
            Self::Unreadable(path, error) => {
                write!(formatter, "could not read {}: {error}", path.display())
            }
            Self::Malformed(path, reason) => {
                write!(
                    formatter,
                    "{} is not a usable font: {reason}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for Error {}

/// One rasterised glyph, kept so a repeated character is never rasterised twice.
struct Raster {
    width: usize,
    height: usize,
    left: i32,
    top: i32,
    advance: i32,
    coverage: Vec<u8>,
}

/// A loaded face, sized for one panel.
pub struct Typeface {
    font: Font,
    metrics: DisplayMetrics,
    source: PathBuf,
    cache: Mutex<HashMap<(char, u32), Raster>>,
}

impl Typeface {
    /// Loads the device's own face if it has one, and the compiled-in copy of
    /// the same face otherwise.
    ///
    /// There is no host-specific fallback any more. A preview that used
    /// whatever font the developer happened to have installed produced
    /// different line breaks on different machines and matched neither the
    /// panel nor a colleague.
    ///
    /// # Errors
    ///
    /// Returns an error only if `KOBO_FONT` names something unusable, or a
    /// device path exists but cannot be read or parsed.
    pub fn discover(metrics: DisplayMetrics) -> Result<Self, Error> {
        if let Some(override_path) = std::env::var_os(FONT_OVERRIDE) {
            return Self::load(PathBuf::from(override_path), metrics);
        }
        for candidate in DEVICE_FONT_CANDIDATES {
            let path = Path::new(candidate);
            if path.exists() {
                // A candidate that exists but will not parse is skipped rather
                // than raised. Some of what the device keeps in its font
                // directory, under a .ttf name, is not an outline font at all,
                // and one of those must not cost the interface every glyph it
                // has.
                if let Ok(face) = Self::load(path, metrics) {
                    return Ok(face);
                }
            }
        }
        Self::from_bytes(TEXT_FONT, "AtkinsonHyperlegible-Regular.ttf", metrics)
    }

    /// Loads the device's own face for prose, falling back to the interface
    /// face when it carries no serif.
    ///
    /// # Errors
    ///
    /// Returns an error only if `KOBO_READING_FONT` names something unusable,
    /// or a device path exists but cannot be read or parsed.
    pub fn discover_reading(metrics: DisplayMetrics) -> Result<Self, Error> {
        if let Some(override_path) = std::env::var_os(READING_FONT_OVERRIDE) {
            return Self::load(PathBuf::from(override_path), metrics);
        }
        for candidate in READING_FONT_CANDIDATES {
            let path = Path::new(candidate);
            if path.exists() {
                if let Ok(face) = Self::load(path, metrics) {
                    return Ok(face);
                }
            }
        }
        // Not an error. A machine with no serif still reads perfectly well in
        // the interface face, and refusing to start over typography would be
        // absurd.
        Self::discover(metrics)
    }

    /// Loads one specific face.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a usable font.
    pub fn load(path: impl AsRef<Path>, metrics: DisplayMetrics) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let bytes = std::fs::read(&path).map_err(|error| Error::Unreadable(path.clone(), error))?;
        let font = Font::from_bytes(bytes.as_slice(), FontSettings::default())
            .map_err(|reason| Error::Malformed(path.clone(), reason.to_string()))?;
        Ok(Self {
            font,
            metrics,
            source: path,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Loads a face already in memory, for the compiled-in monospace font.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] if the bytes are not a usable font.
    pub fn from_bytes(bytes: &[u8], name: &str, metrics: DisplayMetrics) -> Result<Self, Error> {
        let path = PathBuf::from(name);
        let font = Font::from_bytes(bytes, FontSettings::default())
            .map_err(|reason| Error::Malformed(path.clone(), reason.to_string()))?;
        Ok(Self {
            font,
            metrics,
            source: path,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// The file this face was read from.
    #[must_use]
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The em size in pixels for a semantic size on this panel.
    fn pixels(&self, size: FontSize) -> f32 {
        // `tenth_mm` is the panel-independent definition; this is the only place
        // it becomes a pixel count, so a different panel needs no other change.
        //
        // The scale comes from the ambient setting rather than from the metrics
        // this face was built with. A face is installed once and lives for the
        // life of the process, so a reader who makes a book larger would
        // otherwise be changing a value nothing reads: the glyphs would come
        // back the same size while the layout around them moved.
        let tenths = (size.tenth_mm() * kobo_ui::text_scale().percent() + 50) / 100;
        let pixels = self.metrics.tenth_mm(tenths);
        pixels.max(1) as f32
    }

    fn raster(&self, character: char, pixels: f32) -> Option<Raster> {
        let key = (character, pixels.to_bits());
        let mut cache = self.cache.lock().ok()?;
        if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(key) {
            let (metrics, coverage) = self.font.rasterize(character, pixels);
            slot.insert(Raster {
                width: metrics.width,
                height: metrics.height,
                left: metrics.xmin,
                // `ymin` measures up from the baseline to the bottom of the
                // bitmap, so the top edge is the baseline minus both.
                top: -(metrics.ymin + i32::try_from(metrics.height).unwrap_or(0)),
                advance: metrics.advance_width.round() as i32,
                coverage,
            });
        }
        let raster = cache.get(&key)?;
        Some(Raster {
            width: raster.width,
            height: raster.height,
            left: raster.left,
            top: raster.top,
            advance: raster.advance,
            coverage: raster.coverage.clone(),
        })
    }

    /// What this face should actually draw for `character`.
    ///
    /// Returns the character itself when the face has a glyph for it, a near
    /// equivalent when it does not but one is available, `None` when the
    /// character is meant to be invisible, and the original otherwise so that
    /// the empty box still appears for anything genuinely unrepresentable.
    ///
    /// Text off the network is full of characters a text face has no reason to
    /// carry. Atkinson Hyperlegible, the face the reader ships and the one
    /// compiled in here, has no U+2011 non-breaking hyphen, so a Hacker News
    /// title reading "one-to-one" arrived on the panel as "one[]to[]one". A
    /// hyphen drawn for a hyphen is right in every way that matters: it is what
    /// the author wrote, and the only thing lost is that it may now be broken
    /// across lines.
    fn resolve(&self, character: char) -> Option<char> {
        if self.font.lookup_glyph_index(character) != 0 {
            return Some(character);
        }
        if is_invisible(character) {
            return None;
        }
        substitute(character)
            .filter(|glyph| self.font.lookup_glyph_index(*glyph) != 0)
            .or(Some(character))
    }

    /// The distance from the top of a line to the baseline.
    fn ascent(&self, pixels: f32) -> i32 {
        self.font
            .horizontal_line_metrics(pixels)
            .map_or((pixels * 0.8) as i32, |line| line.ascent.round() as i32)
    }
}

impl Typeface {
    /// The width and height `text` occupies in this face.
    ///
    /// The pen is accumulated in floating point and rounded **once**, which is
    /// the difference between text that looks even and text that does not. A
    /// glyph advance is fractional at every real size; rounding each one before
    /// adding it pushes the error the same direction every time, so by the end
    /// of a line the drift is several pixels. That drift is visible twice over:
    /// as uneven word spacing, and as a disagreement between what wrapping
    /// measured and what the renderer then drew.
    fn measure_run(&self, text: &str, size: FontSize, cell: Option<i32>) -> (i32, i32) {
        if let Some(cell) = cell {
            // A grid is measured by counting, not by adding. The exact sum of
            // 16 advances of 25.875 is 414, but the sixteenth column is drawn
            // at 16 x 26 = 416, and a terminal in which the measured width and
            // the drawn column disagree is a terminal that corrupts its own
            // display the first time it repaints part of a line.
            let cells = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
            return (cells.saturating_mul(cell), self.height(size));
        }
        let pixels = self.pixels(size);
        let mut width = 0f32;
        let mut previous = None;
        for character in text.chars() {
            let Some(character) = self.resolve(character) else {
                continue;
            };
            if let Some(previous) = previous {
                width += kern(&self.font, previous, character, pixels);
            }
            width += self.font.metrics(character, pixels).advance_width;
            previous = Some(character);
        }
        (width.round() as i32, self.height(size))
    }

    /// The baseline-to-baseline distance for this face.
    fn height(&self, size: FontSize) -> i32 {
        let pixels = self.pixels(size);
        self.font.horizontal_line_metrics(pixels).map_or_else(
            || (pixels * 1.3) as i32,
            |line| (line.ascent - line.descent + line.line_gap).ceil() as i32,
        )
    }

    /// Draws one run, with its top-left corner at `x`, `y`.
    ///
    /// Accumulates the same way [`Self::measure_run`] does, so a run drawn here
    /// ends exactly where measuring said it would.
    fn draw_run(
        &self,
        text: &str,
        x: i32,
        y: i32,
        size: FontSize,
        cell: Option<i32>,
        plot: &mut dyn FnMut(i32, i32, u8),
    ) {
        let pixels = self.pixels(size);
        let baseline = y.saturating_add(self.ascent(pixels));
        let mut pen = x as f32;
        let mut previous = None;
        for character in text.chars() {
            let Some(character) = self.resolve(character) else {
                // An invisible character owns a column in a grid and nothing
                // at all in a proportional run. Advancing here in both cases
                // would open a hole in the middle of a word.
                if let Some(cell) = cell {
                    pen += cell as f32;
                }
                continue;
            };
            // Kerning is a proportional idea. Applying it in a grid would move
            // a character out of its own column depending on its neighbour.
            if let (Some(previous), None) = (previous, cell) {
                pen += kern(&self.font, previous, character, pixels);
            }
            if let Some(raster) = self.raster(character, pixels) {
                let origin_x = (pen.round() as i32).saturating_add(raster.left);
                let origin_y = baseline.saturating_add(raster.top);
                for row in 0..raster.height {
                    for column in 0..raster.width {
                        let coverage = raster
                            .coverage
                            .get(row.saturating_mul(raster.width).saturating_add(column))
                            .copied()
                            .unwrap_or(0);
                        if coverage > 0 {
                            plot(
                                origin_x.saturating_add(i32::try_from(column).unwrap_or(0)),
                                origin_y.saturating_add(i32::try_from(row).unwrap_or(0)),
                                coverage,
                            );
                        }
                    }
                }
                pen += cell.map_or_else(
                    || self.font.metrics(character, pixels).advance_width,
                    |cell| cell as f32,
                );
            } else if let Some(cell) = cell {
                // A character with no outline, a space above all, still owns
                // its column. Skipping it would shift the rest of the row left.
                pen += cell as f32;
            }
            previous = Some(character);
        }
    }

    /// The advance every glyph in this face shares, if it is monospaced.
    ///
    /// Returns `None` for a proportional face rather than an average, because
    /// an average is exactly the wrong answer for a grid: it is right for no
    /// character at all.
    fn fixed_advance(&self, size: FontSize) -> Option<i32> {
        let pixels = self.pixels(size);
        let reference = self.font.metrics('0', pixels).advance_width;
        for probe in ['i', 'm', 'W', '.'] {
            let advance = self.font.metrics(probe, pixels).advance_width;
            if (advance - reference).abs() > 0.01 {
                return None;
            }
        }
        Some(reference.round().max(1.0) as i32)
    }
}

/// The two faces the runtime installs, together.
///
/// One object rather than two globals, because the pair has to be chosen at the
/// same moment: a screen laid out with one face and drawn with another is a
/// screen that overlaps itself.
pub struct SystemFonts {
    text: Typeface,
    mono: Typeface,
    reading: Typeface,
}

impl SystemFonts {
    /// Finds the reader's own face for prose and compiles in the one for grids.
    ///
    /// # Errors
    ///
    /// Returns an error only when a face that was found cannot be read or
    /// parsed. A machine with no fonts at all still gets the compiled-in
    /// pair.
    pub fn discover(metrics: DisplayMetrics) -> Result<Self, Error> {
        let text = Typeface::discover(metrics)?;
        // Typography is the one thing here that must never be load-bearing. A
        // prose face that cannot be loaded means books are set in the interface
        // face, which is a slightly worse read; letting it fail the whole set
        // means *nothing* is installed and `kobo-ui` falls back to its built-in
        // bitmap for every character on the device, which is a far worse one.
        // That is not hypothetical -- it is what shipped, because the first
        // prose candidate turned out not to be a font.
        let reading = match Typeface::discover_reading(metrics) {
            Ok(face) => face,
            Err(_) => Typeface::discover(metrics)?,
        };
        Ok(Self {
            text,
            mono: Typeface::from_bytes(MONO_FONT, "DejaVuSansMono.ttf", metrics)?,
            reading,
        })
    }

    /// The file the proportional face was read from.
    #[must_use]
    pub fn text_source(&self) -> &Path {
        self.text.source()
    }

    fn face(&self, face: Face) -> &Typeface {
        match face {
            Face::Text => &self.text,
            Face::Mono => &self.mono,
            Face::Reading => &self.reading,
        }
    }

    /// The fixed cell a face lays out on, if it lays out on one.
    fn cell(&self, size: FontSize, face: Face) -> Option<i32> {
        match face {
            Face::Text | Face::Reading => None,
            Face::Mono => Some(self.cell_width(size)),
        }
    }
}

impl Typesetter for SystemFonts {
    fn measure(&self, text: &str, size: FontSize, face: Face) -> (i32, i32) {
        self.face(face)
            .measure_run(text, size, self.cell(size, face))
    }

    fn line_height(&self, size: FontSize, face: Face) -> i32 {
        let natural = self.face(face).height(size);
        match face {
            // A font's own line height is set for a paragraph in a document,
            // not for a page of a novel. Typesetters have always opened books
            // up further than that, and on a reflective panel it matters more
            // than on paper: with only a few tones to work with, tight lines
            // let the eye drift onto the wrong one and re-read it. A fifth
            // again is the usual book measure and is what the reader's own
            // software is doing on the next screen over.
            Face::Reading => natural + natural / 5,
            Face::Text | Face::Mono => natural,
        }
    }

    fn draw(
        &self,
        text: &str,
        x: i32,
        y: i32,
        size: FontSize,
        face: Face,
        plot: &mut dyn FnMut(i32, i32, u8),
    ) {
        self.face(face)
            .draw_run(text, x, y, size, self.cell(size, face), plot);
    }

    fn has_glyph(&self, character: char, face: Face) -> bool {
        // Answers for what would actually reach the panel, not for what the
        // face happens to contain. A character that is substituted or that is
        // deliberately invisible draws no empty box, so reporting it as
        // missing would condemn text that comes out perfectly readable.
        let typeface = self.face(face);
        // Index zero is `.notdef`, the empty box, which is exactly the thing
        // worth catching before it reaches a panel.
        typeface
            .resolve(character)
            .is_none_or(|glyph| typeface.font.lookup_glyph_index(glyph) != 0)
    }

    fn line_breaks(&self, text: &str) -> Vec<(usize, BreakOpportunity)> {
        linebreaks(text)
            .map(|(offset, opportunity)| {
                let opportunity = match opportunity {
                    UnicodeBreakOpportunity::Allowed => BreakOpportunity::Allowed,
                    UnicodeBreakOpportunity::Mandatory => BreakOpportunity::Mandatory,
                };
                (offset, opportunity)
            })
            .collect()
    }

    fn grapheme_boundaries(&self, text: &str) -> Vec<usize> {
        text.grapheme_indices(true)
            .map(|(offset, grapheme)| offset + grapheme.len())
            .collect()
    }

    fn cell_width(&self, size: FontSize) -> i32 {
        // Falls back to measuring rather than refusing, so a future face that
        // is very nearly fixed pitch still produces a usable grid.
        self.mono
            .fixed_advance(size)
            .unwrap_or_else(|| self.mono.measure_run("0", size, None).0.max(1))
    }
}

/// Whether a character is meant to leave no mark at all.
///
/// A face carries a glyph for a character it is expected to draw. It carries
/// nothing for a zero-width space or a variation selector, because there is
/// nothing to draw, and an empty box in place of one is a fault the author
/// never wrote. These are the invisible characters that turn up in ordinary
/// web text: word joiners, directional marks, and the byte order mark that
/// leads a great many files.
fn is_invisible(character: char) -> bool {
    matches!(character,
        '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{feff}'
            | '\u{fe00}'..='\u{fe0f}'
    )
}

/// A character close enough to stand in for one the face does not carry.
///
/// Only substitutions that keep the author's meaning are listed. A hyphen for
/// a non-breaking hyphen reads identically; a question mark for an ideograph
/// would be a lie. Anything absent from this table keeps the empty box, which
/// is at least honest about there being a character there.
fn substitute(character: char) -> Option<char> {
    Some(match character {
        // Dashes. Every one of these is drawn as a stroke on the same line.
        '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
        | '\u{2212}' | '\u{fe58}' | '\u{fe63}' | '\u{ff0d}' => '-',
        // Quotation. Typographic quotes are the single most common thing to
        // survive a copy and paste into a title.
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' | '\u{ff07}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' | '\u{ff02}' => '"',
        // Spaces of every width, including the ones a typesetter inserts.
        '\u{00a0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200a}'
        | '\u{202f}'
        | '\u{205f}'
        | '\u{3000}' => ' ',
        '\u{2022}' | '\u{2023}' | '\u{2043}' | '\u{25cf}' | '\u{25aa}' => '*',
        '\u{2190}' | '\u{27f5}' => '<',
        '\u{2192}' | '\u{27f6}' => '>',
        '\u{2044}' | '\u{2215}' | '\u{ff0f}' => '/',
        '\u{02dc}' | '\u{ff5e}' => '~',
        _ => return None,
    })
}

fn kern(font: &Font, previous: char, current: char, pixels: f32) -> f32 {
    font.horizontal_kern(previous, current, pixels)
        .unwrap_or(0.0)
}

/// Installs the best available face into `kobo-ui`.
///
/// Returns the path that was loaded, or an error explaining why the built-in
/// fallback is still in use. A failure here is never fatal.
///
/// # Errors
///
/// Returns an error when no face can be found or loaded.
pub fn install(metrics: DisplayMetrics) -> Result<PathBuf, Error> {
    let fonts = SystemFonts::discover(metrics)?;
    let source = fonts.text_source().to_path_buf();
    // A second install means something already chose a face; that is not a
    // failure worth reporting to a caller that only wanted text to look right.
    let _ = kobo_ui::install_typesetter(Box::new(fonts));
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_ui::TextScale;

    #[test]
    fn a_shelf_of_tiles_never_reaches_the_page_position_under_it() {
        // The same failure as the row test below, in the other node that
        // draws more than one thing per line. A shelf of six portrait covers
        // was measured against the panel before a page position took a band
        // out from under it; the grid then drew its second row of captions
        // through the "1 of 6" beneath them, on the device, with every test
        // passing. Measured with the real face for the same reason.
        let _ = install(kobo_ui::CLARA_BW_METRICS);
        let books = [
            ("Moby Dick; Or, The Whale", "Melville, Herman"),
            ("Pride and Prejudice", "Austen, Jane"),
            ("Romeo and Juliet", "Shakespeare, William"),
            ("A Room with a View", "Forster, E. M."),
            ("Crime and Punishment", "Dostoyevsky, Fyodor"),
            ("Alice's Adventures in Wonderland", "Carroll, Lewis"),
        ];
        let tiles = books
            .iter()
            .enumerate()
            .map(|(index, (title, author))| {
                kobo_ui::Tile::new(
                    kobo_ui::ActionId(index as u32 + 1),
                    *title,
                    kobo_ui::Glyph::Book,
                )
                .with_subtitle(*author)
            })
            .collect::<Vec<_>>();
        let screen = kobo_ui::Screen::new(
            1,
            vec![kobo_ui::Node::TileGrid {
                id: kobo_ui::NodeId(1),
                tiles,
                shape: kobo_ui::TileShape::Portrait,
            }],
        )
        .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(0), "Gutenbird"))
        .with_page_turns(kobo_ui::ActionId(90), kobo_ui::ActionId(91));
        let screen = kobo_ui::Screen {
            page_turns: screen.page_turns.map(|turns| turns.with_position(1, 6)),
            ..screen
        };
        // With the status bar the device actually draws. Without it the
        // content starts high enough that the shelf fits and the test agrees
        // with the bug, which is how this shipped.
        let chrome = kobo_ui::Chrome::default().with_status(kobo_ui::Status {
            clock: "20:36".to_string(),
            signal: kobo_ui::Signal::Strong,
            battery: Some(kobo_ui::Percent::new(74)),
            charging: false,
            bluetooth: false,
        });
        let layout = screen.layout_with(&kobo_ui::CLARA_BW_METRICS, &chrome);
        let position = layout
            .nodes
            .iter()
            .find(|node| node.kind == kobo_ui::LayoutKind::PagePosition)
            .expect("a page position");
        let mut tiles_drawn = 0;
        for node in &layout.nodes {
            if matches!(node.kind, kobo_ui::LayoutKind::Tile(..)) {
                tiles_drawn += 1;
                assert!(
                    node.rect.y + node.rect.height <= position.rect.y,
                    "a tile ran to {}, under the page position at {}",
                    node.rect.y + node.rect.height,
                    position.rect.y
                );
            }
        }
        // Fitting by dropping the second row would also satisfy the assertion
        // above and would be a worse shelf than the one that collided. The
        // cell shrinks; the books stay.
        assert_eq!(tiles_drawn, books.len(), "the shelf lost a book to fit");
    }

    #[test]
    fn a_paginated_page_of_rows_never_reaches_the_page_position_under_it() {
        // Measured with the real face, because the built-in bitmap
        // fallback wraps nothing like it and the page fits either way
        // under the fallback -- which is why the arithmetic tests in
        // kobo-ui passed while the panel drew a row through its own
        // page position.
        let _ = install(kobo_ui::CLARA_BW_METRICS);
        // The arithmetic version of this test agreed with the bug: it counted
        // separators the same wrong way the paginator did. So the engine is
        // the oracle here -- paginate, lay the page out, and ask where the
        // rows actually landed.
        let stories: Vec<(&str, &str, &str)> = vec![
            (
                "You Could Have Come Up with Kimi Delta Attention",
                "blog.doubleword.ai \u{b7} 78 comments \u{b7} 2h ago",
                "218 points",
            ),
            (
                "Steel Bank Common Lisp version 2.6.7",
                "sbcl.org \u{b7} 12 comments \u{b7} 1h ago",
                "72 points",
            ),
            (
                "Delayed Gratification \u{2013} Proud to Be 'Last to Breaking News'",
                "slow-journalism.com \u{b7} 55 comments \u{b7} 3h ago",
                "121 points",
            ),
            (
                "Kimi K3 Architecture Overview and Notes",
                "sebastianraschka.com \u{b7} 12 comments \u{b7} 3h ago",
                "104 points",
            ),
            (
                "Zig's Incremental Compilation Internals",
                "mluqg.co.uk \u{b7} 54 comments \u{b7} 3h ago",
                "105 points",
            ),
        ];
        let mut area = kobo_ui::CLARA_BW_METRICS.prose_area(true, true);
        area.height = area
            .height
            .saturating_sub(kobo_ui::CLARA_BW_METRICS.page_position_band())
            .max(1);
        let pages =
            kobo_ui::paginate_rows_with_trailing(&stories, &kobo_ui::CLARA_BW_METRICS, area);
        let first = &pages[0];
        let rows = first
            .iter()
            .enumerate()
            .map(|(place, index)| {
                let (title, summary, trailing) = stories[*index];
                kobo_ui::Row::new(
                    kobo_ui::ActionId(*index as u32 + 1),
                    title,
                    summary,
                    kobo_ui::RowLead::Number(place as u16 + 1),
                )
                .with_trailing(trailing)
            })
            .collect::<Vec<_>>();
        let screen = kobo_ui::Screen::new(
            1,
            vec![kobo_ui::Node::Rows {
                id: kobo_ui::NodeId(1),
                rows,
            }],
        )
        .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(0), "Top"))
        .with_page_turns(kobo_ui::ActionId(90), kobo_ui::ActionId(91))
        .with_nav_bar(kobo_ui::NavBar::actions(
            kobo_ui::NodeId(9),
            vec![
                kobo_ui::BarAction::new(kobo_ui::ActionId(80), "Top"),
                kobo_ui::BarAction::new(kobo_ui::ActionId(81), "New"),
            ],
        ));
        let screen = kobo_ui::Screen {
            page_turns: screen
                .page_turns
                .map(|turns| turns.with_position(1, pages.len() as u16)),
            ..screen
        };
        let layout = screen.layout();
        let position = layout
            .nodes
            .iter()
            .find(|node| node.kind == kobo_ui::LayoutKind::PagePosition)
            .expect("a page position");
        for node in &layout.nodes {
            if matches!(node.kind, kobo_ui::LayoutKind::Row(_)) {
                assert!(
                    node.rect.y + node.rect.height <= position.rect.y,
                    "a row ran to {}, under the page position at {}",
                    node.rect.y + node.rect.height,
                    position.rect.y
                );
            }
        }
    }

    const CLARA: DisplayMetrics = DisplayMetrics {
        width: 1072,
        height: 1448,
        pixels_per_inch: 300,
        text_scale: TextScale::Default,
    };

    fn face() -> Option<Typeface> {
        // Any real face proves the arithmetic; the host may have none.
        for candidate in [
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Helvetica.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ] {
            if Path::new(candidate).exists() {
                if let Ok(face) = Typeface::load(candidate, CLARA) {
                    return Some(face);
                }
            }
        }
        None
    }

    #[test]
    fn body_text_is_a_readable_physical_size() {
        // Bracketed by the two references the scale is set between: no smaller
        // than a phone's body text (iOS 17pt, about 2.6 mm, 31 pixels here) and
        // no larger than a printed paperback (about 3.7 mm, 44 pixels). Below
        // the floor is the defect the built-in bitmap had; above the ceiling is
        // an interface set larger than the books it sits next to.
        let pixels = CLARA.tenth_mm(FontSize::Body.tenth_mm());
        assert!(
            (31..=44).contains(&pixels),
            "body text resolved to {pixels} pixels"
        );
    }

    #[test]
    fn sizes_increase_with_prominence() {
        assert!(FontSize::Caption.tenth_mm() < FontSize::Body.tenth_mm());
        assert!(FontSize::Body.tenth_mm() < FontSize::Title.tenth_mm());
        assert!(FontSize::Title.tenth_mm() < FontSize::Heading.tenth_mm());
    }

    #[test]
    fn accessibility_scale_changes_real_glyph_metrics() {
        // On the *installed* face, because that is the only face there is. A
        // typeface is loaded once and lives as long as the process, so a
        // reader who asks for larger type is asking this face to set larger --
        // not for a second face to be built, which nothing is in a position to
        // do by the time they press the button.
        let face = Typeface::from_bytes(TEXT_FONT, "text.ttf", CLARA).expect("font");
        let at = |scale| {
            kobo_ui::with_text_scale(scale, || {
                (
                    face.measure_run("Readable", FontSize::Body, None).0,
                    face.height(FontSize::Body),
                )
            })
        };
        let (default_width, default_height) = at(TextScale::Default);
        let (large_width, large_height) = at(TextScale::Large);
        let (largest_width, largest_height) = at(TextScale::ExtraLarge);

        assert!(large_width > default_width, "larger type did not set wider");
        assert!(
            large_height > default_height,
            "larger type did not set taller"
        );
        assert!(largest_width > large_width);
        assert!(largest_height > large_height);

        // And it goes back, so one screen asking for large does not leave
        // every screen after it large.
        assert_eq!(at(TextScale::Default), (default_width, default_height));
    }

    #[test]
    fn unicode_breaks_and_graphemes_are_not_ascii_approximations() {
        let fonts = SystemFonts::discover(CLARA).expect("fonts");
        let cjk_breaks = fonts.line_breaks("漢字");
        assert_eq!(cjk_breaks[0], ("漢".len(), BreakOpportunity::Allowed));

        let combined = "e\u{301}x";
        assert_eq!(fonts.grapheme_boundaries(combined)[0], "e\u{301}".len());
    }

    #[test]
    fn production_wrapper_never_exceeds_the_measured_width() {
        let fonts = SystemFonts::discover(CLARA).expect("fonts");
        let maximum = fonts.measure("WWW iii", FontSize::Body, Face::Text).0;
        let _ = kobo_ui::install_typesetter(Box::new(fonts));
        let lines = kobo_ui::wrap_text("WWW iii WWW iii", maximum, FontSize::Body);
        assert_eq!(lines.join(" "), "WWW iii WWW iii");
        assert!(lines
            .iter()
            .all(|line| kobo_ui::measure_text(line, FontSize::Body).0 <= maximum));
    }

    #[test]
    fn a_missing_font_is_an_error_rather_than_a_panic() {
        let outcome = Typeface::load("/nonexistent/font.ttf", CLARA);
        assert!(matches!(outcome, Err(Error::Unreadable(..))));
    }

    #[test]
    fn a_file_that_is_not_a_font_is_rejected() {
        let path = std::env::temp_dir().join("kobo-text-not-a-font.ttf");
        std::fs::write(&path, b"this is definitely not a font").expect("write the decoy");
        let outcome = Typeface::load(&path, CLARA);
        let _ = std::fs::remove_file(&path);
        assert!(matches!(outcome, Err(Error::Malformed(..))));
    }

    /// The regression that put a bitmap font on a shipped device.
    ///
    /// `KoboNickel.ttf` is a Qt font blob rather than an outline font, so the
    /// prose face failed to load; because that failure was raised rather than
    /// absorbed, `install` returned an error, no typesetter was installed at
    /// all, and every character in the interface came out of `kobo-ui`'s
    /// built-in bitmap. A bad prose face may cost prose its serif. It may not
    /// cost the interface its font.
    #[test]
    fn an_unreadable_prose_face_still_leaves_the_interface_with_a_real_one() {
        let decoy = std::env::temp_dir().join("kobo-text-decoy-prose.ttf");
        std::fs::write(&decoy, b"QTD\0 and then some compressed nonsense").expect("write");

        // Proves the file really is unusable, so the test below is not passing
        // for the boring reason that the decoy happened to parse.
        assert!(matches!(
            Typeface::load(&decoy, CLARA),
            Err(Error::Malformed(..))
        ));

        let reading = match Typeface::discover_reading(CLARA) {
            Ok(face) => face,
            Err(_) => Typeface::discover(CLARA).expect("the interface face"),
        };
        let _ = std::fs::remove_file(&decoy);
        assert!(
            reading.font.lookup_glyph_index('a') != 0,
            "whatever the prose face ends up being, it has to be able to draw"
        );

        let fonts = SystemFonts::discover(CLARA).expect("a full set despite a bad prose candidate");
        assert!(fonts.text.font.lookup_glyph_index('a') != 0);
        assert!(fonts.reading.font.lookup_glyph_index('a') != 0);
    }

    /// Existence is not usability. This is the check that was missing.
    #[test]
    fn every_named_font_candidate_is_actually_an_outline_font() {
        for candidate in DEVICE_FONT_CANDIDATES
            .iter()
            .chain(READING_FONT_CANDIDATES.iter())
        {
            let path = Path::new(candidate);
            if !path.exists() {
                continue;
            }
            assert!(
                Typeface::load(path, CLARA).is_ok(),
                "{candidate} is listed as a font but cannot be parsed as one"
            );
        }
    }

    /// The check an application's own tests lean on: a tick character looked
    /// fine in source and rendered as an empty box on the panel, because the
    /// bundled face has no glyph for it.
    #[test]
    fn the_face_reports_what_it_cannot_draw() {
        let bundled =
            Typeface::from_bytes(TEXT_FONT, "bundled", CLARA).expect("the compiled-in face");
        for present in "Aa1,.\u{2026}\u{e9}".chars() {
            assert!(
                bundled.font.lookup_glyph_index(present) != 0,
                "the face should draw {present:?}"
            );
        }
        assert_eq!(bundled.font.lookup_glyph_index('\u{2713}'), 0);
    }

    /// The Hacker News tofu, reproduced from the character that caused it.
    ///
    /// A title reading "one-to-one" with U+2011 non-breaking hyphens arrived on
    /// the panel as "one[]to[]one". This asserts the gap is real in the face and
    /// that nothing reaches the panel because of it.
    #[test]
    fn a_hyphen_the_face_lacks_is_drawn_as_the_hyphen_it_is() {
        let face = Typeface::from_bytes(TEXT_FONT, "bundled", CLARA).expect("the compiled-in face");
        assert_eq!(
            face.font.lookup_glyph_index('\u{2011}'),
            0,
            "this test is pointless unless the face really lacks the character"
        );

        let (plain, _) = face.measure_run("one-to-one", FontSize::Body, None);
        let (fancy, _) = face.measure_run("one\u{2011}to\u{2011}one", FontSize::Body, None);
        assert_eq!(plain, fancy, "the substitute must measure as what it draws");
        assert_eq!(
            ink(&face, "one\u{2011}to\u{2011}one"),
            ink(&face, "one-to-one")
        );
    }

    /// A character with nothing to draw must draw nothing.
    ///
    /// The face carries no zero-width space, so before substitution one drew an
    /// empty box in the middle of a word: the opposite of invisible.
    #[test]
    fn an_invisible_character_takes_no_room_and_leaves_no_mark() {
        let face = Typeface::from_bytes(TEXT_FONT, "bundled", CLARA).expect("the compiled-in face");
        for invisible in ['\u{200b}', '\u{feff}', '\u{fe0f}', '\u{2060}'] {
            assert_eq!(
                face.font.lookup_glyph_index(invisible),
                0,
                "{invisible:?} would not exercise the substitution"
            );
            let text = format!("wo{invisible}rd");
            assert_eq!(
                face.measure_run(&text, FontSize::Body, None),
                face.measure_run("word", FontSize::Body, None),
                "{invisible:?} widened the line it should have left alone"
            );
            assert_eq!(ink(&face, &text), ink(&face, "word"));
        }
    }

    /// An invisible character still owns its column in a grid.
    ///
    /// A terminal measures by counting cells. Dropping a character in the
    /// drawing pass but not in the counting pass would slide the rest of the
    /// row left of where the emulator believes it is.
    #[test]
    fn a_grid_gives_every_character_a_column_even_an_invisible_one() {
        let mono = Typeface::from_bytes(MONO_FONT, "mono", CLARA).expect("mono");
        let cell = mono.fixed_advance(FontSize::Body).expect("a fixed advance");
        let text = "a\u{200b}b";
        assert_eq!(
            mono.measure_run(text, FontSize::Body, Some(cell)).0,
            cell * 3
        );
        let mut columns = Vec::new();
        mono.draw_run(
            text,
            0,
            0,
            FontSize::Body,
            Some(cell),
            &mut |x, _, coverage| {
                if coverage > 0 {
                    columns.push(x);
                }
            },
        );
        let last = columns.iter().copied().max().expect("ink");
        assert!(
            last >= cell * 2,
            "the third column starts at {}, but the last ink is at {last}",
            cell * 2
        );
    }

    /// Every substitute must be a character the face can actually draw.
    ///
    /// A table entry pointing at a second missing glyph would swap one empty
    /// box for another while looking like a fix. This walks the whole table
    /// against the compiled-in face, so an entry added for a character this
    /// face happens to lack cannot pass unnoticed.
    #[test]
    fn every_substitute_is_a_character_the_face_carries() {
        let face = Typeface::from_bytes(TEXT_FONT, "bundled", CLARA).expect("the compiled-in face");
        let mut checked = 0;
        for code in 0..=0x1_ffff_u32 {
            let Some(character) = char::from_u32(code) else {
                continue;
            };
            let Some(stand_in) = substitute(character) else {
                continue;
            };
            checked += 1;
            assert!(
                face.font.lookup_glyph_index(stand_in) != 0,
                "{character:?} is replaced by {stand_in:?}, which the face cannot draw either"
            );
        }
        assert!(checked > 20, "only {checked} substitutions were examined");
    }

    /// Nothing in ordinary web text should reach the panel as an empty box.
    ///
    /// The punctuation a title, a feed or an article is made of, gathered in
    /// one place so that the day a face is swapped, this says which character
    /// stopped working rather than a reader noticing it.
    #[test]
    fn the_punctuation_of_ordinary_web_text_is_all_drawable() {
        let fonts = SystemFonts::discover(CLARA).expect("fonts");
        let text = "\u{2018}\u{2019}\u{201c}\u{201d}\u{2013}\u{2014}\u{2010}\u{2011}\u{2012}\
                    \u{2026}\u{00a0}\u{2009}\u{202f}\u{200b}\u{feff}\u{2060}\u{fe0f}\u{2032}\
                    \u{2033}\u{2192}\u{2190}\u{2022}\u{00b7}\u{00d7}\u{2212}\u{2044}\u{00ad}";
        for face in [Face::Text, Face::Reading, Face::Mono] {
            for character in text.chars() {
                assert!(
                    fonts.has_glyph(character, face),
                    "U+{:04X} would draw an empty box in {face:?}",
                    character as u32
                );
            }
        }
    }

    /// Every pixel a run puts down, so two runs can be compared exactly.
    fn ink(face: &Typeface, text: &str) -> Vec<(i32, i32)> {
        let mut marks = Vec::new();
        face.draw_run(text, 0, 0, FontSize::Body, None, &mut |x, y, coverage| {
            if coverage > 0 {
                marks.push((x, y));
            }
        });
        marks
    }

    #[test]
    fn lowercase_is_distinct_from_uppercase() {
        let Some(face) = face() else {
            return;
        };
        let lower = face.measure_run("aaaa", FontSize::Body, None);
        let upper = face.measure_run("AAAA", FontSize::Body, None);
        // The built-in bitmap folded case away entirely, so these were equal.
        assert_ne!(lower, upper, "case is still being folded away");
    }

    #[test]
    fn proportional_widths_differ_between_characters() {
        let Some(face) = face() else {
            return;
        };
        let narrow = face.measure_run("iiii", FontSize::Body, None);
        let wide = face.measure_run("mmmm", FontSize::Body, None);
        assert!(narrow.0 < wide.0, "text is still monospaced");
    }

    #[test]
    fn measuring_is_additive_across_a_string() {
        let Some(face) = face() else {
            return;
        };
        let once = face.measure_run("kobo", FontSize::Body, None).0;
        let twice = face.measure_run("kobokobo", FontSize::Body, None).0;
        let drift = (twice - once * 2).abs();
        assert!(
            drift <= once / 10,
            "measurement drifts: {once} then {twice}"
        );
    }

    #[test]
    fn glyphs_land_inside_the_measured_box() {
        let Some(face) = face() else {
            return;
        };
        let text = "Reading";
        let (width, height) = face.measure_run(text, FontSize::Body, None);
        let mut out_of_bounds = 0;
        face.draw_run(text, 0, 0, FontSize::Body, None, &mut |x, y, _| {
            if x < 0 || y < 0 || x > width || y > height {
                out_of_bounds += 1;
            }
        });
        assert_eq!(out_of_bounds, 0, "glyphs escaped the box they measured");
    }

    #[test]
    fn drawing_produces_coverage() {
        let Some(face) = face() else {
            return;
        };
        let mut covered = 0;
        face.draw_run(
            "Hello",
            0,
            0,
            FontSize::Body,
            None,
            &mut |_, _, coverage| {
                if coverage > 0 {
                    covered += 1;
                }
            },
        );
        assert!(covered > 100, "only {covered} pixels were inked");
    }

    #[test]
    fn edges_are_antialiased_rather_than_binary() {
        let Some(face) = face() else {
            return;
        };
        let mut partial = 0;
        face.draw_run("Ss", 0, 0, FontSize::Title, None, &mut |_, _, coverage| {
            if coverage > 0 && coverage < 255 {
                partial += 1;
            }
        });
        assert!(partial > 0, "no antialiased edge pixels were produced");
    }

    #[test]
    fn a_larger_size_draws_larger_text() {
        let Some(face) = face() else {
            return;
        };
        let caption = face.measure_run("Chapter", FontSize::Caption, None);
        let heading = face.measure_run("Chapter", FontSize::Heading, None);
        assert!(caption.0 < heading.0);
        assert!(caption.1 < heading.1);
    }

    #[test]
    fn a_book_is_set_on_more_open_lines_than_an_interface() {
        // The whole point of a separate reading face. A font's own line height
        // is set for a paragraph in a document, not for a page of a novel, and
        // on a panel resolving few tones tight lines let the eye drop onto the
        // wrong one and read it twice.
        let Some(fonts) = fonts() else {
            return;
        };
        let interface = fonts.line_height(FontSize::Body, Face::Text);
        let book = fonts.line_height(FontSize::Body, Face::Reading);
        assert!(
            book > interface,
            "prose is set at {book} and the interface at {interface}; a book should be the more open of the two"
        );
        // And not so open that the page holds nothing. A fifth again is the
        // usual book measure; double would be a poster.
        assert!(
            book < interface * 3 / 2,
            "prose at {book} against an interface at {interface} is too loose to be a book"
        );
    }

    #[test]
    fn the_prose_face_is_a_real_face_rather_than_a_missing_one() {
        // A machine with no serif falls back to the interface face rather than
        // failing, so this asserts only that something usable was loaded and
        // that it can draw the punctuation a book is full of.
        let Some(fonts) = fonts() else {
            return;
        };
        for character in ['\u{201C}', '\u{201D}', '\u{2019}', '\u{2014}', '\u{2026}'] {
            assert!(
                fonts.has_glyph(character, Face::Reading),
                "the reading face cannot draw {character:?}, which every book contains"
            );
        }
    }

    fn fonts() -> Option<SystemFonts> {
        SystemFonts::discover(CLARA).ok()
    }

    #[test]
    fn the_monospace_face_is_always_available() {
        // The device ships no monospaced face at all, so this one is compiled
        // in. If it ever stops loading, a terminal has no grid to stand on.
        let mono = Typeface::from_bytes(MONO_FONT, "mono", CLARA);
        assert!(mono.is_ok(), "the embedded monospace face did not parse");
    }

    #[test]
    fn every_monospace_glyph_has_the_same_advance() {
        let mono = Typeface::from_bytes(MONO_FONT, "mono", CLARA).expect("mono");
        let cell = mono.fixed_advance(FontSize::Body).expect("fixed pitch");
        for probe in ["i", "m", "W", ".", "0", "|"] {
            let (width, _) = mono.measure_run(probe, FontSize::Body, Some(cell));
            assert_eq!(width, cell, "{probe} is not one cell wide");
        }
    }

    #[test]
    fn a_monospace_run_is_exactly_its_length_in_cells() {
        let mono = Typeface::from_bytes(MONO_FONT, "mono", CLARA).expect("mono");
        let cell = mono.fixed_advance(FontSize::Body).expect("fixed pitch");
        // A grid is addressed by column, so this has to hold exactly rather
        // than approximately, or column 60 is not where column 60 was drawn.
        let (width, _) = mono.measure_run("cat /proc/uptime", FontSize::Body, Some(cell));
        assert_eq!(width, cell * 16);
    }

    #[test]
    fn the_proportional_face_is_not_reported_as_fixed_pitch() {
        let Some(face) = face() else {
            return;
        };
        assert!(
            face.fixed_advance(FontSize::Body).is_none(),
            "a proportional face claimed a single cell width"
        );
    }

    #[test]
    fn the_two_faces_are_addressed_separately() {
        let Some(fonts) = fonts() else {
            return;
        };
        let text = fonts.measure("iiiiiiii", FontSize::Body, Face::Text).0;
        let mono = fonts.measure("iiiiiiii", FontSize::Body, Face::Mono).0;
        assert_ne!(text, mono, "both faces resolved to the same file");
        assert_eq!(mono, fonts.cell_width(FontSize::Body) * 8);
    }

    #[test]
    fn measuring_does_not_drift_across_a_long_line() {
        let Some(face) = face() else {
            return;
        };
        // Rounding each advance before summing pushed the error the same way
        // every time. The exact sum is the only honest reference: rounding once
        // lands within a pixel of it, rounding per glyph is tens of pixels out
        // over a line, which shows up as uneven spacing and as wrapping that
        // disagrees with what is drawn.
        let line = "n".repeat(60);
        let pixels = face.pixels(FontSize::Body);
        let exact: f32 = line
            .chars()
            .map(|character| face.font.metrics(character, pixels).advance_width)
            .sum();
        let measured = face.measure_run(&line, FontSize::Body, None).0;
        assert!(
            (measured as f32 - exact).abs() <= 1.0,
            "measured {measured} against an exact {exact}"
        );
    }
    #[test]
    fn a_caption_grid_is_wide_enough_to_be_a_terminal() {
        let mono = Typeface::from_bytes(MONO_FONT, "mono", CLARA).expect("mono");
        // Measured on the Clara BW panel: Caption gives 53 columns by 37 rows,
        // Body only 41 columns. Anything much narrower than 50 and ordinary
        // command output wraps into unreadable rubble, so this is the floor a
        // future face change must not silently drop below.
        let cell = mono.fixed_advance(FontSize::Caption).expect("fixed pitch");
        let columns = 1072 / cell;
        let rows = 1448 / mono.height(FontSize::Caption);
        assert!(columns >= 50, "only {columns} columns fit");
        assert!(rows >= 30, "only {rows} rows fit");
    }

    /// The bar title is ellipsised to a width measured at one size, so it has
    /// to be drawn at that same size or the ellipsis is fitted to a sentence
    /// nobody sees. It was measured at body size and drawn at title size, and
    /// a Hacker News thread called "Our position on open-weights models" ran
    /// off the right edge of the panel, cut mid-word, with no mark to say it
    /// had been cut.
    ///
    /// Asserted in ink rather than in font sizes, because the measuring and
    /// the drawing are set in two different functions and an assertion about
    /// either one alone is exactly the assertion that missed this. It lives
    /// here rather than in `kobo-ui` because it needs the real typeface: the
    /// built-in bitmap fallback has an advance of its own that no measurement
    /// agrees with.
    #[test]
    fn a_long_bar_title_leaves_no_ink_outside_its_own_rect() {
        let fonts = SystemFonts::discover(CLARA).expect("fonts");
        let _ = kobo_ui::install_typesetter(Box::new(fonts));

        let title = "Our position on open-weights models, and on much else besides";
        let screen = kobo_ui::Screen::new(1, Vec::new())
            .with_top_bar(kobo_ui::TopBar::new(kobo_ui::NodeId(2), title));
        let chrome = kobo_ui::Chrome::with_back(true);
        let metrics = CLARA;
        let layout = screen.layout_with(&metrics, &chrome);
        let node = layout
            .nodes
            .iter()
            .find(|node| node.kind == kobo_ui::LayoutKind::TopBarTitle)
            .expect("the bar carries a title")
            .clone();

        let width = usize::try_from(metrics.width).expect("a positive panel width");
        let height = usize::try_from(metrics.height).expect("a positive panel height");
        let mut surface = kobo_ui::Surface::new(width, height);
        kobo_ui::render_all(&screen, &metrics, &chrome, &(), &mut surface, None);

        let right = node.rect.x.saturating_add(node.rect.width);
        let top = usize::try_from(node.rect.y.max(0)).expect("a title inside the panel");
        let bottom = usize::try_from(node.rect.y.saturating_add(node.rect.height).max(0))
            .expect("a title inside the panel")
            .min(height);
        // Two pixels of slack, for the antialiasing on the last glyph the
        // renderer draws right up against its own boundary. The regression
        // this guards ran forty pixels past the panel itself.
        let from = usize::try_from(right.saturating_add(2).max(0))
            .expect("a right edge inside the panel")
            .min(width);
        for y in top..bottom {
            for x in from..width {
                assert_eq!(
                    surface.pixels[y * width + x],
                    kobo_ui::tone::PAPER,
                    "the title drew ink at {x},{y}, past its own right edge at {right}"
                );
            }
        }
    }
}
