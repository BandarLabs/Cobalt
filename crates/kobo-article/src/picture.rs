//! Turning a downloaded picture into one the reader can show.
//!
//! The reader decodes JPEG and PNG and nothing else, its panel is 1072 by
//! 1448 on the reference model, and most of them show sixteen greys. A page
//! serves WebP at four times that width in full colour, so every picture is
//! decoded here, fitted to the panel, turned grey unless the owner asked to
//! keep colour, and written in whichever of the two formats is smaller for
//! what it shows.

use std::io::Cursor;

use image::imageops::FilterType;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, Limits};

/// The largest picture decoded, in pixels. A 12 megapixel photograph is
/// common; a 40 megapixel one is a poster, and past that is a page trying
/// to exhaust memory.
const MAX_SOURCE_PIXELS: u64 = 40_000_000;

/// The most memory one decode may ask for.
const MAX_DECODE_ALLOCATION: u64 = 512 * 1024 * 1024;

/// Pictures smaller than this on their short side are icons, rules and
/// spacers, not illustrations.
const MIN_SHORT_SIDE: u32 = 24;

/// Pictures with fewer pixels than a 64 by 64 square are badges and avatars.
const MIN_AREA: u64 = 64 * 64;

/// Good enough that a photograph shows no blocks on the panel, small enough
/// that a dozen of them fit comfortably in a book.
const JPEG_QUALITY: u8 = 85;

/// A picture ready to go into the book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub extension: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Why a picture was left out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unusable {
    /// Not a format that could be decoded here, or damaged.
    Undecodable,
    /// Bigger than anything worth decoding.
    TooBig,
    /// So small it is an icon or a tracking pixel.
    TooSmall,
    /// Decoded, but could not be written again.
    Unencodable,
}

/// The largest size `source` can be drawn at on `panel` without being
/// enlarged or changing shape.
#[must_use]
pub fn fitted(source: (u32, u32), panel: (u32, u32)) -> (u32, u32) {
    let (width, height) = source;
    if width <= panel.0 && height <= panel.1 {
        return source;
    }
    // Compare the two scale factors without floating point: width-limited
    // when panel.0 / width <= panel.1 / height.
    let width_limited =
        u64::from(panel.0) * u64::from(height) <= u64::from(panel.1) * u64::from(width);
    if width_limited {
        let scaled = u64::from(height) * u64::from(panel.0) / u64::from(width);
        (panel.0, u32::try_from(scaled).unwrap_or(panel.1).max(1))
    } else {
        let scaled = u64::from(width) * u64::from(panel.1) / u64::from(height);
        (u32::try_from(scaled).unwrap_or(panel.0).max(1), panel.1)
    }
}

fn decode(bytes: &[u8]) -> Result<(DynamicImage, bool), Unusable> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| Unusable::Undecodable)?;
    let format = reader.format().ok_or(Unusable::Undecodable)?;
    // A lossless source may be line art, where a PNG is both smaller and
    // sharper than a JPEG; a JPEG source is a photograph already.
    let maybe_line_art = !matches!(format, image::ImageFormat::Jpeg);
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOCATION);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|_| Unusable::Undecodable)?;
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_SOURCE_PIXELS {
        return Err(Unusable::TooBig);
    }
    if width.min(height) < MIN_SHORT_SIDE || pixels < MIN_AREA {
        return Err(Unusable::TooSmall);
    }
    let orientation = decoder.orientation().ok();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| Unusable::Undecodable)?;
    if let Some(orientation) = orientation {
        image.apply_orientation(orientation);
    }
    Ok((image, maybe_line_art))
}

/// Lays a picture with transparency on white paper, which is what the page
/// behind it was. Left transparent, a black logo on a clear background turns
/// into a black square once the alpha is dropped.
fn on_paper(image: DynamicImage) -> DynamicImage {
    if !image.color().has_alpha() {
        return image;
    }
    let mut rgba = image.to_rgba8();
    for pixel in rgba.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            let value = u16::from(pixel[channel]);
            // value * alpha + 255 * (255 - alpha), divided by 255.
            let blended = (value * alpha + 255 * (255 - alpha) + 127) / 255;
            pixel[channel] = u8::try_from(blended).unwrap_or(u8::MAX);
        }
        pixel[3] = 255;
    }
    DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(rgba).to_rgb8())
}

/// Prepares one picture for a panel of `panel` pixels.
///
/// # Errors
///
/// [`Unusable`] when the picture cannot be read, is too large to decode, is
/// an icon or a pixel, or cannot be written again.
pub fn prepare(bytes: &[u8], panel: (u32, u32), colour: bool) -> Result<Prepared, Unusable> {
    let (image, maybe_line_art) = decode(bytes)?;
    let image = on_paper(image);
    let (width, height) = fitted((image.width(), image.height()), panel);
    let image = if (width, height) == (image.width(), image.height()) {
        image
    } else {
        image.resize_exact(width, height, FilterType::Lanczos3)
    };
    let (raw, colour_type) = if colour {
        (image.to_rgb8().into_raw(), image::ExtendedColorType::Rgb8)
    } else {
        (image.to_luma8().into_raw(), image::ExtendedColorType::L8)
    };

    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .write_image(&raw, width, height, colour_type)
        .map_err(|_| Unusable::Unencodable)?;
    if maybe_line_art {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(
            &mut png,
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        )
        .write_image(&raw, width, height, colour_type)
        .map_err(|_| Unusable::Unencodable)?;
        // A chart or a diagram compresses better losslessly and keeps its
        // edges; a photograph saved as PNG is several times the JPEG. A
        // little extra size is worth paying for sharp lines.
        if png.len() <= jpeg.len() + jpeg.len() / 4 {
            return Ok(Prepared {
                bytes: png,
                media_type: "image/png",
                extension: "png",
                width,
                height,
            });
        }
    }
    Ok(Prepared {
        bytes: jpeg,
        media_type: "image/jpeg",
        extension: "jpg",
        width,
        height,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A photograph-like picture: smooth noise in every channel.
    pub(crate) fn photograph(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            let noise =
                (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)).wrapping_mul(2_654_435_761);
            let byte = |shift: u32| u8::try_from((noise >> shift) & 0xff).unwrap_or(0);
            image::Rgb([byte(0), byte(8), byte(16)])
        });
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut out, format)
            .expect("encodes");
        out.into_inner()
    }

    /// A chart-like picture: flat colour and hard edges, with transparency.
    pub(crate) fn diagram(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            if (x / 40 + y / 40) % 2 == 0 {
                image::Rgba([200, 30, 30, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encodes");
        out.into_inner()
    }

    #[test]
    fn a_picture_wider_than_the_panel_is_scaled_to_its_width() {
        assert_eq!(fitted((2144, 1000), (1072, 1448)), (1072, 500));
        assert_eq!(fitted((1000, 2896), (1072, 1448)), (500, 1448));
    }

    #[test]
    fn a_picture_that_already_fits_is_never_enlarged() {
        assert_eq!(fitted((300, 200), (1072, 1448)), (300, 200));
    }

    #[test]
    fn a_large_colour_photograph_becomes_a_grey_jpeg_that_fits_the_panel() {
        let prepared = prepare(
            &photograph(2400, 1600, image::ImageFormat::Jpeg),
            (1072, 1448),
            false,
        )
        .expect("a photograph is usable");
        assert_eq!(prepared.media_type, "image/jpeg");
        assert_eq!((prepared.width, prepared.height), (1072, 714));
        let decoded = image::load_from_memory(&prepared.bytes).expect("decodes");
        assert_eq!(decoded.color(), image::ColorType::L8);
    }

    #[test]
    fn colour_is_kept_when_asked_for() {
        let prepared = prepare(
            &photograph(400, 300, image::ImageFormat::Png),
            (1072, 1448),
            true,
        )
        .expect("a photograph is usable");
        let decoded = image::load_from_memory(&prepared.bytes).expect("decodes");
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        assert_eq!((prepared.width, prepared.height), (400, 300));
    }

    #[test]
    fn a_flat_diagram_stays_a_png_and_its_transparency_becomes_white() {
        let prepared = prepare(&diagram(400, 400), (1072, 1448), false).expect("usable");
        assert_eq!(prepared.media_type, "image/png");
        let decoded = image::load_from_memory(&prepared.bytes)
            .expect("decodes")
            .to_luma8();
        // A transparent square, which would have been black once the alpha
        // channel was dropped.
        assert_eq!(decoded.get_pixel(60, 10)[0], 255);
    }

    #[test]
    fn a_webp_or_gif_from_the_page_is_converted_to_a_format_the_reader_decodes() {
        for format in [image::ImageFormat::WebP, image::ImageFormat::Gif] {
            let prepared =
                prepare(&photograph(320, 240, format), (1072, 1448), false).expect("usable");
            assert!(matches!(prepared.media_type, "image/jpeg" | "image/png"));
            assert!(image::load_from_memory(&prepared.bytes).is_ok());
        }
    }

    #[test]
    fn tracking_pixels_and_icons_are_refused() {
        let pixel = photograph(1, 1, image::ImageFormat::Gif);
        assert_eq!(
            prepare(&pixel, (1072, 1448), false),
            Err(Unusable::TooSmall)
        );
        let icon = photograph(48, 48, image::ImageFormat::Png);
        assert_eq!(prepare(&icon, (1072, 1448), false), Err(Unusable::TooSmall));
        let rule = photograph(900, 4, image::ImageFormat::Png);
        assert_eq!(prepare(&rule, (1072, 1448), false), Err(Unusable::TooSmall));
    }

    #[test]
    fn something_that_is_not_a_picture_is_refused() {
        assert_eq!(
            prepare(b"<html>not found</html>", (1072, 1448), false),
            Err(Unusable::Undecodable)
        );
        assert_eq!(
            prepare(&[], (1072, 1448), false),
            Err(Unusable::Undecodable)
        );
    }
}
