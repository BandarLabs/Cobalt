use kobo_abi::{hwtcon, mxc_epdc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[must_use]
    pub fn clipped(self, screen_width: u32, screen_height: u32) -> Option<Self> {
        let right = self.x.saturating_add(self.width).min(screen_width);
        let bottom = self.y.saturating_add(self.height).min(screen_height);
        if self.x >= right || self.y >= bottom {
            return None;
        }
        Some(Self {
            x: self.x,
            y: self.y,
            width: right - self.x,
            height: bottom - self.y,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshIntent {
    /// A change that is purely black and white.
    ///
    /// `DU` is two-level by construction: it drives every pixel to black or to
    /// white and nothing between. That makes it the fastest waveform and the
    /// wrong one for anything with grey in it, because the panel has no way to
    /// represent the middle and smears what it cannot show.
    FastFeedback,
    /// A change containing grey: antialiased text, rules, images.
    ///
    /// `GL16` resolves sixteen levels without the black-white-black flash of a
    /// full update, which is what makes it usable for text that changes often.
    TextContent,
    /// A complete replacement that also clears accumulated ghosting.
    QualityContent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshPlan {
    pub region: Rect,
    pub waveform: u32,
    pub full: bool,
}

impl RefreshPlan {
    #[must_use]
    pub fn new(
        region: Rect,
        intent: RefreshIntent,
        full: bool,
        screen_width: u32,
        screen_height: u32,
    ) -> Option<Self> {
        Some(Self {
            region: region.clipped(screen_width, screen_height)?,
            waveform: match intent {
                RefreshIntent::FastFeedback => hwtcon::WAVEFORM_DU,
                RefreshIntent::TextContent => hwtcon::WAVEFORM_GL16,
                RefreshIntent::QualityContent => hwtcon::WAVEFORM_GC16,
            },
            full,
        })
    }

    #[must_use]
    pub fn hwtcon_update_data(self, marker: u32) -> hwtcon::HwtconUpdateData {
        hwtcon::HwtconUpdateData {
            update_region: hwtcon::HwtconRect {
                top: self.region.y,
                left: self.region.x,
                width: self.region.width,
                height: self.region.height,
            },
            waveform_mode: self.waveform,
            update_mode: if self.full {
                hwtcon::UPDATE_MODE_FULL
            } else {
                hwtcon::UPDATE_MODE_PARTIAL
            },
            update_marker: marker,
            flags: 0,
            dither_mode: 0,
        }
    }

    #[must_use]
    pub fn epdc_update_data(self, marker: u32) -> mxc_epdc::MxcfbUpdateData {
        // EPDC waveform numbers differ from HWTCON: remap the stored value.
        let waveform = match self.waveform {
            hwtcon::WAVEFORM_DU => mxc_epdc::WAVEFORM_DU,
            hwtcon::WAVEFORM_GC16 => mxc_epdc::WAVEFORM_GC16,
            hwtcon::WAVEFORM_GL16 => mxc_epdc::WAVEFORM_GL16,
            other => other,
        };
        mxc_epdc::MxcfbUpdateData {
            update_region: mxc_epdc::MxcfbRect {
                top: self.region.y,
                left: self.region.x,
                width: self.region.width,
                height: self.region.height,
            },
            waveform_mode: waveform,
            update_mode: if self.full {
                mxc_epdc::UPDATE_MODE_FULL
            } else {
                mxc_epdc::UPDATE_MODE_PARTIAL
            },
            update_marker: marker,
            temp: mxc_epdc::TEMP_USE_AMBIENT,
            flags: 0,
            dither_mode: 0,
            quant_bit: 0,
            alt_buffer_data: mxc_epdc::MxcfbAltBufferData::default(),
        }
    }
}

#[derive(Debug)]
pub struct UpdateMarker {
    next: u32,
}

impl UpdateMarker {
    #[must_use]
    pub fn new(seed: u32) -> Self {
        Self { next: seed.max(1) }
    }

    pub fn take(&mut self) -> u32 {
        let marker = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        marker
    }
}

#[cfg(test)]
mod tests {
    use super::{Rect, RefreshIntent, RefreshPlan, UpdateMarker};
    use kobo_abi::hwtcon;

    #[test]
    fn clips_regions_before_building_update() {
        let plan = RefreshPlan::new(
            Rect {
                x: 1060,
                y: 1440,
                width: 40,
                height: 40,
            },
            RefreshIntent::QualityContent,
            false,
            1072,
            1448,
        )
        .expect("region intersects screen");
        assert_eq!(plan.region.width, 12);
        assert_eq!(plan.region.height, 8);
        assert_eq!(plan.waveform, hwtcon::WAVEFORM_GC16);
    }

    #[test]
    fn markers_never_emit_zero() {
        let mut markers = UpdateMarker::new(u32::MAX);
        assert_eq!(markers.take(), u32::MAX);
        assert_eq!(markers.take(), 1);
    }
}
