//! Turning pictures from the network into something an E Ink panel can show.
//!
//! Three steps, deliberately separate, because each one can fail or be refused
//! on its own: decode what arrived, fit it to the space the layout gave it, and
//! reduce it to the greys the panel can actually hold.
//!
//! Decoding is the one place in this project where bytes from a stranger are
//! parsed by code nobody here wrote, so the caps below are not decoration. A
//! JPEG header is a few bytes and can claim any size it likes; without a
//! ceiling on the decoded pixel count a hostile (or merely enormous) image
//! takes the reader's memory and, on a device with no swap and no fan, the
//! reader with it.

use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader};
use std::fmt;
use std::io::Cursor;

/// The most compressed bytes a picture may arrive as.
///
/// Comfortably above a book cover or a photograph at panel size, and far below
/// anything worth streaming to a device like this.
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

/// The most pixels a picture may decode to, whatever its header claims.
///
/// Four times the panel, so a source larger than the screen is still allowed
/// (downscaling a big photograph is exactly what this is for) while a header
/// claiming forty thousand square is refused before a buffer is allocated.
pub const MAX_PIXELS: u64 = 4 * 1072 * 1448;

/// How far [`Picture::fit_enlarging`] will blow a small picture up.
///
/// Three is where a 190 pixel book cover stops being lettering on a 300 pixel
/// per inch panel. Past it the halftone that follows has nothing left to
/// resolve and the result reads as a decoding fault.
pub const MAX_ENLARGEMENT: u32 = 3;

/// How many greys a panel can hold, unless a caller knows better.
///
/// Sixteen is what this family of controllers drives, and it is the number
/// that makes dithering worth doing: at 256 the reduction is invisible, and at
/// two everything becomes a woodcut.
pub const PANEL_GREYS: u8 = 16;

/// Wraps grey panel bytes up as a PNG.
///
/// The other direction from everything else here, and it exists for one
/// reason: a frame off the panel is 1072 by 1448 bytes of raw grey, which is
/// not a thing a person can look at or an agent can read. A screenshot that
/// has to be converted before it can be opened is a screenshot nobody takes.
///
/// Greyscale, eight bit, no palette -- the same shape the surface already
/// holds, so this is an encode and not a conversion.
///
/// # Errors
///
/// Returns [`ImageError::Undecodable`] when `grey` is not exactly
/// `width * height` bytes, and [`ImageError::TooManyPixels`] past
/// [`MAX_PIXELS`].
pub fn encode_png_grey(width: u32, height: u32, grey: &[u8]) -> Result<Vec<u8>, ImageError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(ImageError::TooManyPixels { pixels });
    }
    if grey.len() as u64 != pixels {
        return Err(ImageError::Undecodable(format!(
            "{} bytes for a {width} by {height} frame, which needs {pixels}",
            grey.len()
        )));
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(grey, width, height, image::ExtendedColorType::L8)
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    Ok(png)
}

/// How a picture should occupy the rectangle a component assigned to it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FitMode {
    /// Show the whole picture and leave any remainder as paper.
    #[default]
    Contain,
    /// Contain, but enlarge small sources by at most [`MAX_ENLARGEMENT`].
    ContainEnlarging,
    /// Fill the rectangle exactly, cropping equal amounts from opposite edges.
    Cover,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageError {
    /// More compressed bytes than [`MAX_SOURCE_BYTES`].
    TooManyBytes { bytes: usize },
    /// More pixels than [`MAX_PIXELS`].
    TooManyPixels { pixels: u64 },
    /// Nothing here recognises this as a picture.
    UnknownFormat,
    /// The decoder rejected the bytes, in its own words: "this is not a
    /// picture" and "this picture is truncated" are different things to the
    /// person looking at the screen.
    Undecodable(String),
    /// A fit was asked for into a box with no area.
    EmptyBox,
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyBytes { bytes } => write!(
                formatter,
                "the picture is {bytes} bytes, and {MAX_SOURCE_BYTES} is the most that is read"
            ),
            Self::TooManyPixels { pixels } => write!(
                formatter,
                "the picture claims {pixels} pixels, and {MAX_PIXELS} is the most that is decoded"
            ),
            Self::UnknownFormat => {
                write!(formatter, "this is not a picture in any format read here")
            }
            Self::Undecodable(why) => write!(formatter, "the picture could not be read: {why}"),
            Self::EmptyBox => write!(formatter, "a picture cannot be fitted into no space at all"),
        }
    }
}

impl std::error::Error for ImageError {}

/// One picture, in the only form the panel has any use for: eight bit grey,
/// one byte per pixel, top row first, no padding.
///
/// The same layout the drawing surface uses, so painting one is a copy rather
/// than a conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Picture {
    width: u32,
    height: u32,
    grey: Vec<u8>,
}

impl Picture {
    /// Builds a picture from grey bytes that are already the right shape.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::TooManyPixels`] when the dimensions exceed
    /// [`MAX_PIXELS`], and [`ImageError::Undecodable`] when `grey` is not
    /// exactly `width * height` bytes.
    pub fn from_grey(width: u32, height: u32, grey: Vec<u8>) -> Result<Self, ImageError> {
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_PIXELS {
            return Err(ImageError::TooManyPixels { pixels });
        }
        if grey.len() as u64 != pixels {
            return Err(ImageError::Undecodable(format!(
                "{} bytes for a {width} by {height} picture, which needs {pixels}",
                grey.len()
            )));
        }
        Ok(Self {
            width,
            height,
            grey,
        })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// The pixels, one grey byte each, row by row from the top.
    #[must_use]
    pub fn grey(&self) -> &[u8] {
        &self.grey
    }

    #[must_use]
    pub fn into_grey(self) -> Vec<u8> {
        self.grey
    }

    /// The largest size that fits inside `width` by `height` without changing
    /// the shape of the picture.
    ///
    /// Separate from [`Picture::fit`] so a layout can ask how much room a
    /// picture will really take before deciding to give it any. A cover is
    /// taller than it is wide and a screenshot is wider than it is tall, and a
    /// row that reserves a square for both wastes most of it on one of them.
    #[must_use]
    pub fn size_within(&self, width: u32, height: u32) -> (u32, u32) {
        if self.width == 0 || self.height == 0 || width == 0 || height == 0 {
            return (0, 0);
        }
        let by_width = u64::from(width) * u64::from(self.height);
        let by_height = u64::from(height) * u64::from(self.width);
        // Whichever edge runs out first decides, and the other is derived from
        // it, so the ratio survives exactly rather than to within rounding on
        // both axes independently.
        if by_width <= by_height {
            let scaled = by_width / u64::from(self.width);
            (width, u32::try_from(scaled).unwrap_or(height).max(1))
        } else {
            let scaled = by_height / u64::from(self.height);
            (u32::try_from(scaled).unwrap_or(width).max(1), height)
        }
    }

    /// Scales the picture to sit inside `width` by `height`, keeping its shape.
    ///
    /// A picture smaller than the box is returned untouched rather than blown
    /// up. On a panel with no colour and a slow refresh an enlarged thumbnail
    /// reads as a fault, while a small picture reads as a small picture.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::EmptyBox`] when the box has no area.
    pub fn fit(&self, width: u32, height: u32) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::EmptyBox);
        }
        if self.width <= width && self.height <= height {
            return Ok(self.clone());
        }
        // Lanczos rather than nearest or triangle, inside `scaled_to`.
        // Lettering on a book cover is the usual subject, and it is the only
        // filter here that keeps it legible at a third of its original size.
        self.scaled_to(width, height)
    }

    /// Scales the picture to fill `width` by `height` as closely as its shape
    /// allows, enlarging it by up to [`MAX_ENLARGEMENT`] if it is smaller.
    ///
    /// [`Picture::fit`] is the right answer for a picture shown at whatever
    /// size it happens to be. This is the right answer for a picture given a
    /// cell of its own: a book cover published at 190 by 300 sits in a third of
    /// a portrait tile on a 300 pixel-per-inch panel and reads as a stamp
    /// somebody dropped in an empty box. Enlargement is bounded because past
    /// three times a cover stops being lettering and becomes a blur, and the
    /// remainder is left as margin rather than smeared.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::EmptyBox`] when the box has no area.
    pub fn fit_enlarging(&self, width: u32, height: u32) -> Result<Self, ImageError> {
        if width == 0 || height == 0 {
            return Err(ImageError::EmptyBox);
        }
        let ceiling_width = self.width.saturating_mul(MAX_ENLARGEMENT);
        let ceiling_height = self.height.saturating_mul(MAX_ENLARGEMENT);
        self.scaled_to(width.min(ceiling_width), height.min(ceiling_height))
    }

    /// Prepares a picture with an explicit, discoverable fit policy.
    ///
    /// This is the preferred entry point for application code; the older
    /// `fit` methods remain available for source compatibility.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::EmptyBox`] for a zero-sized target and
    /// [`ImageError::TooManyPixels`] before an oversized result is allocated.
    pub fn prepare(&self, width: u32, height: u32, mode: FitMode) -> Result<Self, ImageError> {
        match mode {
            FitMode::Contain => self.fit(width, height),
            FitMode::ContainEnlarging => self.fit_enlarging(width, height),
            FitMode::Cover => self.cover(width, height),
        }
    }

    /// Fills `width` by `height`, preserving aspect ratio and cropping from the
    /// centre. Useful for tile artwork and full-bleed hero images where empty
    /// bands are more distracting than losing the outer edge of a photograph.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::EmptyBox`] for a zero-sized target and
    /// [`ImageError::TooManyPixels`] before an oversized result is allocated.
    pub fn cover(&self, width: u32, height: u32) -> Result<Self, ImageError> {
        checked_pixels(width, height)?;
        if width == 0 || height == 0 || self.width == 0 || self.height == 0 {
            return Err(ImageError::EmptyBox);
        }
        let width_limited =
            u64::from(width) * u64::from(self.height) >= u64::from(height) * u64::from(self.width);
        let (scaled_width, scaled_height) = if width_limited {
            (
                width,
                div_ceil(u64::from(width) * u64::from(self.height), self.width),
            )
        } else {
            (
                div_ceil(u64::from(height) * u64::from(self.width), self.height),
                height,
            )
        };
        checked_pixels(scaled_width, scaled_height)?;
        let source = image::GrayImage::from_raw(self.width, self.height, self.grey.clone())
            .ok_or_else(|| ImageError::Undecodable("the picture is not its own size".to_owned()))?;
        let scaled =
            image::imageops::resize(&source, scaled_width, scaled_height, FilterType::Lanczos3);
        let left = (scaled_width - width) / 2;
        let top = (scaled_height - height) / 2;
        let cropped = image::imageops::crop_imm(&scaled, left, top, width, height).to_image();
        Self::from_grey(width, height, cropped.into_raw())
    }

    /// Resamples to the largest size inside the box, in either direction.
    fn scaled_to(&self, width: u32, height: u32) -> Result<Self, ImageError> {
        let (target_width, target_height) = self.size_within(width, height);
        if target_width == 0 || target_height == 0 {
            return Err(ImageError::EmptyBox);
        }
        if target_width == self.width && target_height == self.height {
            return Ok(self.clone());
        }
        checked_pixels(target_width, target_height)?;
        let source = image::GrayImage::from_raw(self.width, self.height, self.grey.clone())
            .ok_or_else(|| ImageError::Undecodable("the picture is not its own size".to_owned()))?;
        let scaled =
            image::imageops::resize(&source, target_width, target_height, FilterType::Lanczos3);
        Ok(Self {
            width: target_width,
            height: target_height,
            grey: scaled.into_raw(),
        })
    }

    /// Reduces the picture to `levels` evenly spaced greys, spreading what is
    /// lost into the pixels not yet visited.
    ///
    /// Floyd–Steinberg, because the alternative on a panel with sixteen greys
    /// is banding, and a band across a photograph is far more obvious than the
    /// grain this leaves. Fewer than two levels is treated as two: one grey is
    /// not a picture.
    pub fn dither(&mut self, levels: u8) {
        let levels = u32::from(levels.max(2));
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 {
            return;
        }
        // Carried beside the pixels rather than inside them. Rounding the
        // error back into a byte at every step is what makes naive error
        // diffusion drift dark.
        let mut current_error = vec![0_i32; width];
        let mut next_error = vec![0_i32; width];
        for y in 0..height {
            for x in 0..width {
                let index = y * width + x;
                let wanted = i32::from(self.grey[index]) + current_error[x];
                let quantized = nearest_level(wanted, levels);
                self.grey[index] = u8::try_from(quantized.clamp(0, 255)).unwrap_or(255);
                let residue = wanted - quantized;
                if residue == 0 {
                    continue;
                }
                if x + 1 < width {
                    current_error[x + 1] += residue * 7 / 16;
                }
                if y + 1 < height {
                    if x > 0 {
                        next_error[x - 1] += residue * 3 / 16;
                    }
                    next_error[x] += residue * 5 / 16;
                    if x + 1 < width {
                        next_error[x + 1] += residue / 16;
                    }
                }
            }
            std::mem::swap(&mut current_error, &mut next_error);
            next_error.fill(0);
        }
    }
}

fn div_ceil(numerator: u64, denominator: u32) -> u32 {
    let denominator = u64::from(denominator);
    u32::try_from(numerator.div_ceil(denominator)).unwrap_or(u32::MAX)
}

fn checked_pixels(width: u32, height: u32) -> Result<u64, ImageError> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(ImageError::TooManyPixels { pixels });
    }
    Ok(pixels)
}

fn nearest_level(value: i32, levels: u32) -> i32 {
    let steps = i32::try_from(levels - 1).unwrap_or(1).max(1);
    let clamped = value.clamp(0, 255);
    let step = (clamped * steps + 127) / 255;
    (step * 255 + steps / 2) / steps
}

/// How large a picture will be, without decoding it.
///
/// Reads the header and stops. Every format here states its own dimensions in
/// the first few dozen bytes, so this costs microseconds where [`decode`] costs
/// a hundred milliseconds for a plate off a Kobo's processor -- and a layout
/// that only needs to know how much room to leave should not be paying for
/// pixels it is not going to draw yet. The reader uses it to measure an
/// illustrated book at the moment it opens and decode the plates afterwards,
/// which is the difference between a book opening at once and the panel
/// freezing for three seconds.
///
/// Orientation is applied, so a photograph the camera wrote sideways is
/// reported the way it will be drawn rather than the way it was stored.
///
/// # Errors
///
/// The same refusals as [`decode`], for the same reasons: a header claiming
/// more than [`MAX_PIXELS`] is a header worth refusing before anyone acts on
/// it.
pub fn size(bytes: &[u8]) -> Result<(u32, u32), ImageError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(ImageError::TooManyBytes { bytes: bytes.len() });
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    if reader.format().is_none() {
        return Err(ImageError::UnknownFormat);
    }
    let mut decoder = reader
        .into_decoder()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    let orientation = decoder
        .orientation()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    let (width, height) = decoder.dimensions();
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(ImageError::TooManyPixels { pixels });
    }
    // A quarter turn swaps the two. Everything else -- the flips, and no
    // orientation at all -- leaves them as they were stored.
    Ok(match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (height, width),
        _ => (width, height),
    })
}

/// What [`Picture::fit`] will make of a picture `source` pixels across.
///
/// Answered from dimensions alone, so a caller holding nothing but the reply
/// from [`size`] can work out how much room the picture will take before
/// deciding to decode it. That is what lets an illustrated book be measured
/// the moment it opens and drawn a plate at a time afterwards, with no page
/// changing size when the pixels arrive.
///
/// Not the same answer as [`Picture::size_within`], which scales a small
/// picture up to its box. `fit` deliberately leaves one alone, so this does
/// too: this predicts `fit`, and a prediction that disagreed with the thing it
/// predicts would be worse than none.
#[must_use]
pub fn fitted_size(source: (u32, u32), width: u32, height: u32) -> (u32, u32) {
    let (source_width, source_height) = source;
    if source_width == 0 || source_height == 0 || width == 0 || height == 0 {
        return (0, 0);
    }
    // A picture already inside the box is left alone, the way `fit` leaves it.
    if source_width <= width && source_height <= height {
        return source;
    }
    let by_width = u64::from(width) * u64::from(source_height);
    let by_height = u64::from(height) * u64::from(source_width);
    if by_width <= by_height {
        let scaled = by_width / u64::from(source_width);
        (width, u32::try_from(scaled).unwrap_or(height).max(1))
    } else {
        let scaled = by_height / u64::from(source_height);
        (u32::try_from(scaled).unwrap_or(width).max(1), height)
    }
}

/// Reads a picture that arrived over the network.
///
/// # Errors
///
/// Refuses anything over [`MAX_SOURCE_BYTES`] before looking at it, anything
/// claiming more than [`MAX_PIXELS`] before allocating for it, and reports
/// whatever the decoder said otherwise.
pub fn decode(bytes: &[u8]) -> Result<Picture, ImageError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(ImageError::TooManyBytes { bytes: bytes.len() });
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    if reader.format().is_none() {
        return Err(ImageError::UnknownFormat);
    }
    // Asked before decoding, not after. This is the check that stops a header
    // claiming a billion pixels from becoming a billion pixel allocation.
    let (width, height) = reader
        .into_dimensions()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_PIXELS {
        return Err(ImageError::TooManyPixels { pixels });
    }
    let mut decoder = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?
        .into_decoder()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    let orientation = decoder
        .orientation()
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|error| ImageError::Undecodable(error.to_string()))?;
    image.apply_orientation(orientation);

    // Transparency is composited onto the same white paper the renderer
    // clears to. Discarding alpha first turns a transparent black logo into a
    // black rectangle. Luminance uses perceptual channel weights rather than
    // an average, which keeps coloured title lettering distinguishable once
    // the panel has no colour left to show.
    //
    // Reduced to luminance before compositing rather than after, which is the
    // same number (luminance is an affine combination whose weights sum to
    // one, so compositing commutes with it) at half the peak memory. A source
    // at `MAX_PIXELS` is twenty-five megabytes as RGBA and twelve as grey with
    // alpha, on a device with no swap.
    let luma = image.to_luma_alpha8();
    let mut grey = Vec::with_capacity(luma.width() as usize * luma.height() as usize);
    for pixel in luma.pixels() {
        let alpha = u32::from(pixel[1]);
        let on_paper = (u32::from(pixel[0]) * alpha + 255 * (255 - alpha) + 127) / 255;
        grey.push(u8::try_from(on_paper).unwrap_or(255));
    }
    Picture::from_grey(luma.width(), luma.height(), grey)
}

#[cfg(test)]
mod tests {
    use super::{
        decode, fitted_size, size, FitMode, ImageError, Picture, MAX_ENLARGEMENT, MAX_PIXELS,
        PANEL_GREYS,
    };
    use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};
    use std::io::Cursor;

    /// A real four by four JPEG, so the decoder is exercised against a file
    /// rather than against a mock of one.
    fn tiny_jpeg() -> Vec<u8> {
        const BASE64: &str = concat!(
            "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0a",
            "HBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAAEAAQBAREA/8QAHwAAAQUBAQEB",
            "AQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1Fh",
            "ByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZ",
            "WmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXG",
            "x8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/9oACAEBAAA/APn+v//Z"
        );
        decode_base64(BASE64)
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut accumulator = 0_u32;
        let mut bits = 0_u32;
        for byte in text.bytes() {
            if byte == b'=' {
                break;
            }
            let Some(value) = ALPHABET.iter().position(|candidate| *candidate == byte) else {
                continue;
            };
            accumulator = (accumulator << 6) | u32::try_from(value).unwrap_or(0);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(u8::try_from((accumulator >> bits) & 0xff).unwrap_or(0));
            }
        }
        out
    }

    fn png(width: u32, height: u32, rgba: Vec<u8>) -> Vec<u8> {
        let image = RgbaImage::from_raw(width, height, rgba).expect("rgba fixture");
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .expect("encode png");
        bytes.into_inner()
    }

    fn oriented_jpeg() -> Vec<u8> {
        let image = RgbImage::from_raw(2, 1, vec![255, 0, 0, 0, 0, 0]).expect("rgb fixture");
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut encoded, ImageFormat::Jpeg)
            .expect("encode jpeg");
        let jpeg = encoded.into_inner();

        // Minimal little-endian Exif IFD carrying orientation 6 (rotate 90°).
        let exif: [u8; 36] = [
            0xff, 0xe1, 0x00, 0x22, b'E', b'x', b'i', b'f', 0, 0, b'I', b'I', 0x2a, 0, 8, 0, 0, 0,
            1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0,
        ];
        let mut result = Vec::with_capacity(jpeg.len() + exif.len());
        result.extend_from_slice(&jpeg[..2]);
        result.extend_from_slice(&exif);
        result.extend_from_slice(&jpeg[2..]);
        result
    }

    #[test]
    fn a_real_jpeg_decodes_to_grey_bytes() {
        let picture = decode(&tiny_jpeg()).expect("decode the jpeg");
        assert_eq!(picture.width(), 4);
        assert_eq!(picture.height(), 4);
        assert_eq!(picture.grey().len(), 16);
    }

    #[test]
    fn transparent_pixels_are_composited_onto_paper() {
        let picture = decode(&png(2, 1, vec![0, 0, 0, 0, 0, 0, 0, 255])).expect("png");
        assert_eq!(picture.grey(), &[255, 0]);
    }

    #[test]
    fn colour_is_reduced_by_perceptual_weight_rather_than_by_average() {
        // An averaged red and an averaged blue come out the same grey, which
        // on a book cover is the title disappearing into its background.
        let picture = decode(&png(
            3,
            1,
            vec![255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255],
        ))
        .expect("png");
        let [red, green, blue] = <[u8; 3]>::try_from(picture.grey()).expect("three pixels");
        assert!(green > red && red > blue, "{red} {green} {blue}");
    }

    #[test]
    fn a_half_transparent_pixel_lands_between_its_colour_and_the_paper() {
        let opaque = decode(&png(1, 1, vec![0, 0, 0, 255])).expect("png");
        let half = decode(&png(1, 1, vec![0, 0, 0, 128])).expect("png");
        assert_eq!(opaque.grey(), &[0]);
        assert_eq!(half.grey(), &[127]);
    }

    #[test]
    fn jpeg_exif_orientation_is_applied_before_layout() {
        let picture = decode(&oriented_jpeg()).expect("oriented jpeg");
        assert_eq!((picture.width(), picture.height()), (1, 2));
    }

    #[test]
    fn something_that_is_not_a_picture_is_refused_rather_than_guessed_at() {
        let error = decode(b"<!doctype html><title>not a picture</title>").expect_err("refused");
        assert!(matches!(
            error,
            ImageError::UnknownFormat | ImageError::Undecodable(_)
        ));
    }

    /// The decoder reads bytes from a stranger, so the ceiling has to hold
    /// before anything is allocated rather than after.
    #[test]
    fn an_enormous_claim_is_refused_before_it_is_believed() {
        let error = Picture::from_grey(100_000, 100_000, Vec::new()).expect_err("refused");
        assert!(matches!(error, ImageError::TooManyPixels { .. }));
        assert!(u64::from(100_000_u32) * u64::from(100_000_u32) > MAX_PIXELS);
    }

    #[test]
    fn grey_bytes_that_are_not_the_size_they_claim_are_refused() {
        let error = Picture::from_grey(4, 4, vec![0; 15]).expect_err("refused");
        assert!(matches!(error, ImageError::Undecodable(_)));
    }

    #[test]
    fn fitting_keeps_the_shape_of_the_picture() {
        // A book cover: taller than it is wide, which is the case a square box
        // gets wrong if it scales both axes independently.
        let picture = Picture::from_grey(190, 300, vec![128; 190 * 300]).expect("build");
        let fitted = picture.fit(120, 120).expect("fit");
        assert_eq!(fitted.height(), 120, "the tall edge is the one that binds");
        assert_eq!(fitted.width(), 76, "190/300 of 120, kept exactly");
        assert_eq!(fitted.grey().len(), 76 * 120);
    }

    #[test]
    fn cover_fills_the_box_and_crops_from_the_centre() {
        let picture =
            Picture::from_grey(4, 2, vec![10, 20, 30, 40, 10, 20, 30, 40]).expect("build");
        let covered = picture.prepare(2, 2, FitMode::Cover).expect("cover");
        assert_eq!((covered.width(), covered.height()), (2, 2));
        assert_eq!(covered.grey(), &[20, 30, 20, 30]);
    }

    #[test]
    fn resize_targets_are_bounded_before_allocation() {
        let picture = Picture::from_grey(1, 1, vec![0]).expect("build");
        assert!(matches!(
            picture.cover(u32::MAX, u32::MAX),
            Err(ImageError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn a_picture_smaller_than_its_box_is_left_alone() {
        let picture = Picture::from_grey(20, 30, vec![7; 600]).expect("build");
        let fitted = picture.fit(400, 400).expect("fit");
        assert_eq!((fitted.width(), fitted.height()), (20, 30));
        assert_eq!(fitted.grey(), picture.grey(), "and not resampled either");
    }

    /// The defect this exists for: a Gutenberg cover is 190 by 300, a portrait
    /// tile on the Clara is more than twice that, and `fit` left the cover at
    /// its published size floating in an empty cell.
    #[test]
    fn a_cover_given_a_tile_of_its_own_is_enlarged_to_most_of_it() {
        let picture = Picture::from_grey(190, 300, vec![128; 190 * 300]).expect("build");
        let filled = picture.fit_enlarging(480, 660).expect("fit");
        assert_eq!(filled.height(), 660, "the tall edge is the one that binds");
        assert_eq!(filled.width(), 418, "190/300 of 660, kept exactly");
        assert_eq!(filled.grey().len(), 418 * 660);
    }

    #[test]
    fn enlargement_stops_before_a_picture_becomes_a_blur() {
        let picture = Picture::from_grey(40, 40, vec![9; 1600]).expect("build");
        let filled = picture.fit_enlarging(4000, 4000).expect("fit");
        assert_eq!(
            (filled.width(), filled.height()),
            (40 * MAX_ENLARGEMENT, 40 * MAX_ENLARGEMENT),
            "the cap decides, not the box"
        );
    }

    #[test]
    fn enlarging_still_never_overflows_a_box_a_picture_already_exceeds() {
        let picture = Picture::from_grey(1000, 1000, vec![9; 1_000_000]).expect("build");
        let filled = picture.fit_enlarging(100, 100).expect("fit");
        assert_eq!((filled.width(), filled.height()), (100, 100));
    }

    /// The whole point of reading a header: the answer has to be the same one
    /// decoding would have given, or a page measured from it moves when the
    /// pixels turn up -- which is the thing this was written to stop.
    #[test]
    fn the_size_read_from_a_header_is_the_size_the_decoder_returns() {
        let jpeg = tiny_jpeg();
        let decoded = decode(&jpeg).expect("decode");
        assert_eq!(
            size(&jpeg).expect("header"),
            (decoded.width(), decoded.height())
        );

        let png = png(37, 91, vec![200; 37 * 91 * 4]);
        let decoded = decode(&png).expect("decode");
        assert_eq!(
            size(&png).expect("header"),
            (decoded.width(), decoded.height())
        );
    }

    /// And the same for the size it will be drawn at, which is what a layout
    /// actually reserves room for.
    #[test]
    fn a_predicted_fit_is_the_fit_that_happens() {
        for (width, height) in [(37, 91), (400, 200), (16, 16)] {
            let picture = Picture::from_grey(width, height, vec![64; (width * height) as usize])
                .expect("build");
            for (box_width, box_height) in [(100, 100), (945, 1063), (10, 4000)] {
                let fitted = picture.fit(box_width, box_height).expect("fit");
                assert_eq!(
                    fitted_size((width, height), box_width, box_height),
                    (fitted.width(), fitted.height()),
                    "a {width} by {height} picture in a {box_width} by {box_height} box"
                );
            }
        }
    }

    #[test]
    fn a_box_with_no_area_is_refused_rather_than_producing_nothing() {
        let picture = Picture::from_grey(4, 4, vec![0; 16]).expect("build");
        assert_eq!(
            picture.fit(0, 10).expect_err("refused"),
            ImageError::EmptyBox
        );
    }

    #[test]
    fn dithering_leaves_only_the_greys_the_panel_can_hold() {
        // A gradient, which is exactly what bands when it is simply rounded.
        let grey = (0..=255_u8).collect::<Vec<_>>();
        let mut picture = Picture::from_grey(16, 16, grey).expect("build");
        picture.dither(PANEL_GREYS);
        let allowed = (0..u32::from(PANEL_GREYS))
            .map(|step| u8::try_from((step * 255 + 7) / 15).unwrap_or(255))
            .collect::<Vec<_>>();
        for value in picture.grey() {
            assert!(
                allowed.contains(value),
                "{value} is not one of the {PANEL_GREYS} greys: {allowed:?}"
            );
        }
    }

    /// Error diffusion that rounds inside the pixel buffer drifts, and a
    /// photograph that comes out visibly darker than it went in is the usual
    /// symptom. The mean has to survive the reduction.
    #[test]
    fn dithering_keeps_the_overall_brightness() {
        let grey = (0..1024)
            .map(|index| u8::try_from(index % 256).unwrap_or(0))
            .collect::<Vec<_>>();
        let picture = Picture::from_grey(32, 32, grey).expect("build");
        let before = average(picture.grey());
        let mut dithered = picture.clone();
        dithered.dither(PANEL_GREYS);
        let after = average(dithered.grey());
        assert!(
            (before - after).abs() < 3.0,
            "brightness moved from {before} to {after}"
        );
    }

    #[test]
    fn one_grey_is_not_a_picture_so_the_floor_is_two() {
        let mut picture = Picture::from_grey(4, 4, vec![90; 16]).expect("build");
        picture.dither(0);
        for value in picture.grey() {
            assert!(
                *value == 0 || *value == 255,
                "{value} is not black or white"
            );
        }
    }

    fn average(values: &[u8]) -> f64 {
        let count = u32::try_from(values.len()).expect("a short fixture");
        values.iter().map(|value| f64::from(*value)).sum::<f64>() / f64::from(count)
    }
}
