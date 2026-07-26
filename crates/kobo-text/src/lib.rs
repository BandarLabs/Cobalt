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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use fontdue::{Font, FontSettings};
use kobo_ui::{DisplayMetrics, FontSize, Typesetter};

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

impl Typesetter for Typeface {
    fn measure(&self, text: &str, size: FontSize) -> (i32, i32) {
        let pixels = self.pixels(size);
        let mut width = 0i32;
        let mut previous = None;
        for character in text.chars() {
            let advance = self.font.metrics(character, pixels).advance_width.round() as i32;
            if let Some(previous) = previous {
                width = width.saturating_add(kern(&self.font, previous, character, pixels));
            }
            width = width.saturating_add(advance);
            previous = Some(character);
        }
        (width, self.line_height(size))
    }

    fn line_height(&self, size: FontSize) -> i32 {
        let pixels = self.pixels(size);
        self.font.horizontal_line_metrics(pixels).map_or_else(
            || (pixels * 1.3) as i32,
            |line| (line.ascent - line.descent + line.line_gap).ceil() as i32,
        )
    }

    fn draw(&self, text: &str, x: i32, y: i32, size: FontSize, plot: &mut dyn FnMut(i32, i32, u8)) {
        let pixels = self.pixels(size);
        let baseline = y.saturating_add(self.ascent(pixels));
        let mut pen = x;
        let mut previous = None;
        for character in text.chars() {
            if let Some(previous) = previous {
                pen = pen.saturating_add(kern(&self.font, previous, character, pixels));
            }
            if let Some(raster) = self.raster(character, pixels) {
                let origin_x = pen.saturating_add(raster.left);
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
                pen = pen.saturating_add(raster.advance);
            }
            previous = Some(character);
        }
    }
}

fn kern(font: &Font, previous: char, current: char, pixels: f32) -> i32 {
    font.horizontal_kern(previous, current, pixels)
        .map_or(0, |amount| amount.round() as i32)
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
    let typeface = Typeface::discover(metrics)?;
    let source = typeface.source().to_path_buf();
    // A second install means something already chose a face; that is not a
    // failure worth reporting to a caller that only wanted text to look right.
    let _ = kobo_ui::install_typesetter(Box::new(typeface));
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
        let lower = face.measure("aaaa", FontSize::Body);
        let upper = face.measure("AAAA", FontSize::Body);
        // The built-in bitmap folded case away entirely, so these were equal.
        assert_ne!(lower, upper, "case is still being folded away");
    }

    #[test]
    fn proportional_widths_differ_between_characters() {
        let Some(face) = face() else {
            return;
        };
        let narrow = face.measure("iiii", FontSize::Body);
        let wide = face.measure("mmmm", FontSize::Body);
        assert!(narrow.0 < wide.0, "text is still monospaced");
    }

    #[test]
    fn measuring_is_additive_across_a_string() {
        let Some(face) = face() else {
            return;
        };
        let once = face.measure("kobo", FontSize::Body).0;
        let twice = face.measure("kobokobo", FontSize::Body).0;
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
        let (width, height) = face.measure(text, FontSize::Body);
        let mut out_of_bounds = 0;
        face.draw(text, 0, 0, FontSize::Body, &mut |x, y, _| {
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
        face.draw("Hello", 0, 0, FontSize::Body, &mut |_, _, coverage| {
            if coverage > 0 {
                covered += 1;
            }
        });
        assert!(covered > 100, "only {covered} pixels were inked");
    }

    #[test]
    fn edges_are_antialiased_rather_than_binary() {
        let Some(face) = face() else {
            return;
        };
        let mut partial = 0;
        face.draw("Ss", 0, 0, FontSize::Title, &mut |_, _, coverage| {
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
        let caption = face.measure("Chapter", FontSize::Caption);
        let heading = face.measure("Chapter", FontSize::Heading);
        assert!(caption.0 < heading.0);
        assert!(caption.1 < heading.1);
    }
}
