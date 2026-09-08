//! Page geometry shared by comic and score readers. Coordinates are normalized
//! so a resize or rotation preserves the part of the page the owner chose.

use kobo_image::{ImageError, Picture};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Fit {
    #[default]
    Page,
    Width,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    pub fit: Fit,
    zoom: u16,
    x: u16,
    y: u16,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            fit: Fit::Page,
            zoom: 100,
            x: 5000,
            y: 0,
        }
    }
}

impl Viewport {
    #[must_use]
    pub fn new(fit: Fit, zoom: u16, x: u16, y: u16) -> Self {
        Self {
            fit,
            zoom: zoom.clamp(100, 400),
            x: x.min(10_000),
            y: y.min(10_000),
        }
    }
    #[must_use]
    pub const fn zoom(self) -> u16 {
        self.zoom
    }
    #[must_use]
    pub const fn position(self) -> (u16, u16) {
        (self.x, self.y)
    }
    pub fn zoom_by(&mut self, delta: i16) {
        self.zoom = self.zoom.saturating_add_signed(delta).clamp(100, 400);
    }
    /// Bounded keyboard/button pan: 2500 moves one quarter of the available
    /// travel. A control may disable itself when this returns false.
    pub fn pan(&mut self, x: i16, y: i16) -> bool {
        let prior = *self;
        self.x = self.x.saturating_add_signed(x).min(10_000);
        self.y = self.y.saturating_add_signed(y).min(10_000);
        *self != prior
    }
    pub fn reset(&mut self, fit: Fit) {
        *self = Self {
            fit,
            ..Self::default()
        };
    }

    /// The visible source rectangle, calculated before any image allocation.
    ///
    /// # Errors
    /// Refuses empty or excessive source/target geometry.
    pub fn crop(
        self,
        source: (u32, u32),
        target: (u32, u32),
    ) -> Result<(u32, u32, u32, u32), ImageError> {
        for (width, height) in [source, target] {
            if width == 0 || height == 0 {
                return Err(ImageError::EmptyBox);
            }
            if u64::from(width) * u64::from(height) > kobo_image::MAX_PIXELS {
                return Err(ImageError::TooManyPixels {
                    pixels: u64::from(width) * u64::from(height),
                });
            }
        }
        let (sw, sh) = source;
        let base_height = match self.fit {
            Fit::Page => sh,
            Fit::Width => u32::try_from(u64::from(sw) * u64::from(target.1) / u64::from(target.0))
                .unwrap_or(u32::MAX)
                .clamp(1, sh),
        };
        let width = (sw * 100 / u32::from(self.zoom)).max(1);
        let height = (base_height * 100 / u32::from(self.zoom)).max(1);
        let x = u32::try_from(u64::from(sw - width) * u64::from(self.x) / 10_000).unwrap_or(0);
        let y = u32::try_from(u64::from(sh - height) * u64::from(self.y) / 10_000).unwrap_or(0);
        Ok((x, y, width, height))
    }

    /// Produces only the visible page area at the requested display size.
    ///
    /// # Errors
    /// Returns bounded crop or resampling errors. No full enlarged page is allocated.
    pub fn render(self, source: &Picture, target: (u32, u32)) -> Result<Picture, ImageError> {
        let (x, y, width, height) = self.crop((source.width(), source.height()), target)?;
        source
            .crop(x, y, width, height)?
            .fit_enlarging(target.0, target.1)
    }
}

/// Composes two already decoded pages into one display-sized spread. The first
/// page is placed on the right for RTL. A caller can always keep single-page
/// mode by rendering its normal viewport instead.
///
/// # Errors
/// Refuses tiny or oversized targets and image resampling errors.
pub fn spread(
    first: &Picture,
    second: &Picture,
    target: (u32, u32),
    rtl: bool,
) -> Result<Picture, ImageError> {
    let (width, height) = target;
    if width < 2 || height == 0 {
        return Err(ImageError::EmptyBox);
    }
    if u64::from(width) * u64::from(height) > kobo_image::MAX_PIXELS {
        return Err(ImageError::TooManyPixels {
            pixels: u64::from(width) * u64::from(height),
        });
    }
    let (left, right) = if rtl {
        (second, first)
    } else {
        (first, second)
    };
    let left = left.fit_enlarging(width / 2, height)?;
    let right = right.fit_enlarging(width - width / 2, height)?;
    let mut pixels = vec![255; width as usize * height as usize];
    for (page, offset, area) in [
        (&left, 0, width / 2),
        (&right, width / 2, width - width / 2),
    ] {
        let x = offset + (area - page.width()) / 2;
        let y = (height - page.height()) / 2;
        for row in 0..page.height() {
            let from = row as usize * page.width() as usize;
            let to = (y + row) as usize * width as usize + x as usize;
            pixels[to..to + page.width() as usize]
                .copy_from_slice(&page.grey()[from..from + page.width() as usize]);
        }
    }
    Picture::from_grey(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn width_fit_reaches_final_row_and_normalized_pan_survives_resize() {
        let mut view = Viewport::new(Fit::Width, 100, 0, 0);
        assert_eq!(view.crop((100, 400), (100, 100)).unwrap(), (0, 0, 100, 100));
        view.pan(0, 10_000);
        assert_eq!(
            view.crop((100, 400), (100, 100)).unwrap(),
            (0, 300, 100, 100)
        );
        assert_eq!(
            view.crop((100, 400), (100, 200)).unwrap(),
            (0, 200, 100, 200)
        );
        view.zoom_by(300);
        let (x, y, w, h) = view.crop((100, 400), (100, 200)).unwrap();
        assert_eq!((x, y, w, h), (0, 350, 25, 50));
        assert!(!view.pan(0, 1));
    }
    #[test]
    fn page_fit_and_spreads_preserve_art_and_reading_order() {
        let a = Picture::from_grey(2, 2, vec![0; 4]).unwrap();
        let b = Picture::from_grey(2, 2, vec![128; 4]).unwrap();
        assert_eq!(Viewport::default().render(&a, (2, 2)).unwrap(), a);
        assert_eq!(
            spread(&a, &b, (4, 2), false).unwrap().grey(),
            &[0, 0, 128, 128, 0, 0, 128, 128]
        );
        assert_eq!(
            spread(&a, &b, (4, 2), true).unwrap().grey(),
            &[128, 128, 0, 0, 128, 128, 0, 0]
        );
        assert!(Viewport::default()
            .crop((1, 1), (u32::MAX, u32::MAX))
            .is_err());
    }
}
