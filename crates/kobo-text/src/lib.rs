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
//! faces the firmware ships, **not one is monospaced** — checked, not assumed.
//! A character grid cannot be faked from a proportional face: forcing a common
//! advance leaves `i` swimming in space and `m` touching its neighbours, and a
//! terminal is precisely where column alignment carries meaning.
//!
//! So this one face travels with us. DejaVu Sans Mono is redistributable, and
//! its licence is shipped beside it in `fonts/`. It also covers the box-drawing
//! block, which is what stops a full-screen program drawing its frame as a
//! column of question marks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fontdue::{Font, FontSettings};
use kobo_ui::{DisplayMetrics, Face, FontSize, Typesetter};

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

/// Faces to fall back to on a development machine.
///
/// These exist only so the simulator shows real type instead of the bitmap.
/// They are **not** the same metrics as the device, so line wrapping in the
/// simulator is approximate whenever one of these is used. The fix is to embed
/// the device face itself, which its licence permits; until then, set
/// `KOBO_FONT` to a copy of the device font for an exact preview.
pub const HOST_FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Supplemental/Verdana.ttf",
    "/System/Library/Fonts/Supplemental/Arial.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

/// The environment variable that overrides every search.
pub const FONT_OVERRIDE: &str = "KOBO_FONT";

/// What went wrong while loading a face.
#[derive(Debug)]
pub enum Error {
    /// No candidate path existed.
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
    /// Loads the first candidate that exists and parses.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoFontFound`] when no candidate path exists, which is
    /// the expected result on a host machine.
    pub fn discover(metrics: DisplayMetrics) -> Result<Self, Error> {
        if let Some(override_path) = std::env::var_os(FONT_OVERRIDE) {
            return Self::load(PathBuf::from(override_path), metrics);
        }
        for candidate in DEVICE_FONT_CANDIDATES
            .iter()
            .chain(HOST_FONT_CANDIDATES.iter())
        {
            let path = Path::new(candidate);
            if path.exists() {
                return Self::load(path, metrics);
            }
        }
        Err(Error::NoFontFound)
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
        let pixels = self.metrics.tenth_mm(size.tenth_mm());
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
}

impl SystemFonts {
    /// Finds the reader's own face for prose and compiles in the one for grids.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoFontFound`] when the device has no usable text face,
    /// which is the expected result on a bare host.
    pub fn discover(metrics: DisplayMetrics) -> Result<Self, Error> {
        Ok(Self {
            text: Typeface::discover(metrics)?,
            mono: Typeface::from_bytes(MONO_FONT, "DejaVuSansMono.ttf", metrics)?,
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
        }
    }

    /// The fixed cell a face lays out on, if it lays out on one.
    fn cell(&self, size: FontSize, face: Face) -> Option<i32> {
        match face {
            Face::Text => None,
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
        self.face(face).height(size)
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

    fn cell_width(&self, size: FontSize) -> i32 {
        // Falls back to measuring rather than refusing, so a future face that
        // is very nearly fixed pitch still produces a usable grid.
        self.mono
            .fixed_advance(size)
            .unwrap_or_else(|| self.mono.measure_run("0", size, None).0.max(1))
    }
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

    const CLARA: DisplayMetrics = DisplayMetrics {
        width: 1072,
        height: 1448,
        pixels_per_inch: 300,
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
        // 3.6 mm at 300 pixels per inch is about 43 pixels. A body size far
        // below this is the defect the built-in bitmap had.
        let pixels = CLARA.tenth_mm(FontSize::Body.tenth_mm());
        assert!(
            (40..=46).contains(&pixels),
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
}
