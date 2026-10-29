//! What a Kaleido 3 panel makes of the colour it is sent.
//!
//! Every colour Kobo sold so far, the Clara Colour and the Libra Colour, has
//! the same display: E Ink's Kaleido 3, which is a greyscale Carta film with a
//! colour filter printed over it. Three facts about it are published by E Ink
//! and repeated in every review, and the simulator models exactly those three
//! and labels the result as a model rather than a measurement.
//!
//! The filter resolves sixteen levels in each of red, green and blue, which is
//! the "4096 colours" on the box. A channel value that is not one of those
//! sixteen is rounded to the nearest, so the first thing this model does is
//! what the controller does: snap every channel to a level.
//!
//! Colour is drawn at 150 pixels per inch on a panel that draws grey at 300.
//! Each filter element sits over one grey pixel, so a full colour takes a
//! group of them and colour detail is half as fine as grey detail, while a
//! black letter under the filter stays as sharp as it ever was. The model
//! keeps luminance per pixel and averages the colour difference over two by
//! two blocks, which is why coloured small text in a capture comes out soft
//! and why the palette never asks for any.
//!
//! The filter also lets through light that is not its own colour, so every
//! colour is paler than the value sent. E Ink quotes Kaleido 3 as thirty
//! percent more saturated than Kaleido Plus and publishes no absolute figure,
//! and nobody has measured a Cobalt screen against the renderer, so the
//! factor here is a stated approximation chosen to err towards the panel:
//! a colour that is legible in the simulator with this applied is legible
//! on the reader. Replace it with a measured value when there is one; the
//! capture metadata names the model so a reviewer knows which they have.

/// How many levels the panel resolves in each channel: the fourth root of
/// the 4096 colours E Ink quotes.
pub const LEVELS_PER_CHANNEL: u8 = 16;

/// The side of the block colour is averaged over, in grey pixels: 300 pixels
/// per inch of grey against 150 of colour.
pub const CHROMA_BLOCK: usize = 2;

/// How much of the colour difference the filter is taken to pass, in
/// percent. An approximation, as the module note says.
pub const SATURATION_PERCENT: i32 = 60;

/// The name the capture metadata carries, so a frame that went through this
/// model is never mistaken for a measured panel or for the renderer's own
/// output.
pub const APPEARANCE: &str = "Kaleido 3 model: 16 levels per channel, colour averaged over \
     2x2 pixels, saturation 60%; approximation, not a measured panel";

/// Snaps one channel to the nearest of the panel's levels, as an 8-bit value.
#[must_use]
pub fn level(channel: u8) -> u8 {
    let steps = u32::from(LEVELS_PER_CHANNEL - 1);
    let step = (u32::from(channel) * steps + 127) / 255;
    u8::try_from(step * 255 / steps).unwrap_or(u8::MAX)
}

/// The panel's rendering of an RGB frame whose luminance plane is `grey`.
///
/// `rgb` is three bytes a pixel and `grey` one, both `width * height` long
/// and both for the same frame: the renderer keeps them in step, so the grey
/// plane is the luminance the panel's ink layer shows at full resolution and
/// the colour plane is what the filter has to make of it. Returns three bytes
/// a pixel. A frame whose colour plane is grey comes back as that grey in
/// every channel, so a Clara BW's capture is unchanged by passing through.
#[must_use]
pub fn appearance(width: usize, height: usize, rgb: &[u8], grey: &[u8]) -> Vec<u8> {
    let pixels = width.saturating_mul(height);
    if rgb.len() < pixels * 3 || grey.len() < pixels {
        return grey.iter().flat_map(|value| [*value; 3]).collect();
    }
    let mut out = vec![0; pixels * 3];
    let block = CHROMA_BLOCK;
    let mut y = 0;
    while y < height {
        let mut x = 0;
        while x < width {
            // The colour difference the filter group carries, averaged over
            // the block and scaled by what the filter passes.
            let mut sum = [0_i32; 3];
            let mut count = 0_i32;
            for row in y..(y + block).min(height) {
                for column in x..(x + block).min(width) {
                    let index = row * width + column;
                    let colour = [
                        level(rgb[index * 3]),
                        level(rgb[index * 3 + 1]),
                        level(rgb[index * 3 + 2]),
                    ];
                    let luminance = i32::from(kobo_ui::luma(colour));
                    for channel in 0..3 {
                        sum[channel] += i32::from(colour[channel]) - luminance;
                    }
                    count += 1;
                }
            }
            let difference = sum.map(|total| total * SATURATION_PERCENT / (100 * count.max(1)));
            for row in y..(y + block).min(height) {
                for column in x..(x + block).min(width) {
                    let index = row * width + column;
                    let luminance = i32::from(grey[index]);
                    for channel in 0..3 {
                        let value = (luminance + difference[channel]).clamp(0, 255);
                        out[index * 3 + channel] = level(u8::try_from(value).unwrap_or(0));
                    }
                }
            }
            x += block;
        }
        y += block;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_channel_lands_on_one_of_sixteen_levels() {
        for value in 0..=u8::MAX {
            let snapped = level(value);
            assert_eq!(snapped % 17, 0, "{value} snapped to {snapped}");
            assert!(u16::from(snapped).abs_diff(u16::from(value)) <= 9);
        }
        assert_eq!(level(0), 0);
        assert_eq!(level(255), 255);
        assert_eq!(level(17), 17);
        assert_eq!(level(20), 17);
        assert_eq!(level(26), 34);
    }

    #[test]
    fn a_grey_frame_passes_through_as_the_same_grey() {
        let grey: Vec<u8> = (0..16).map(|value| value * 17).collect();
        let rgb: Vec<u8> = grey.iter().flat_map(|value| [*value; 3]).collect();
        let shown = appearance(4, 4, &rgb, &grey);
        for (index, value) in grey.iter().enumerate() {
            assert_eq!(&shown[index * 3..index * 3 + 3], &[*value; 3]);
        }
    }

    #[test]
    fn colour_is_paler_than_sent_and_shared_across_its_block() {
        // Four pixels, one saturated red among three greys of the same
        // luminance: the block shares the red out and tones it down, while
        // each pixel keeps its own luminance.
        let red = [255, 0, 0];
        let luminance = kobo_ui::luma(red);
        let grey = [luminance; 4];
        let mut rgb = vec![luminance; 12];
        rgb[..3].copy_from_slice(&red);
        let shown = appearance(2, 2, &rgb, &grey);
        let first = &shown[..3];
        assert!(first[0] > first[1] && first[0] > first[2], "red was lost");
        assert!(first[0] < 255, "the filter passed full saturation");
        assert_eq!(&shown[3..6], first, "the block did not share its colour");
        assert!(
            first.iter().all(|channel| channel % 17 == 0),
            "the result is not one of the panel's colours"
        );
    }

    #[test]
    fn luminance_detail_stays_per_pixel_inside_a_colour_block() {
        // A black letter stroke over a tinted field: the stroke pixel stays
        // dark even though the block around it is coloured.
        let tint = kobo_ui::Accent::Orange.shades().tint;
        let mut rgb: Vec<u8> = (0..4).flat_map(|_| tint).collect();
        let mut grey = [kobo_ui::luma(tint); 4];
        rgb[..3].copy_from_slice(&[0, 0, 0]);
        grey[0] = 0;
        let shown = appearance(2, 2, &rgb, &grey);
        assert!(
            shown[..3].iter().all(|channel| *channel < 60),
            "{:?}",
            &shown[..3]
        );
        assert!(shown[3] > 150, "the tint around the stroke went dark");
    }

    #[test]
    fn a_short_colour_plane_falls_back_to_luminance() {
        let grey = [10, 20, 30, 40];
        let shown = appearance(2, 2, &[1, 2, 3], &grey);
        assert_eq!(shown, vec![10, 10, 10, 20, 20, 20, 30, 30, 30, 40, 40, 40]);
    }
}
