use kobo_abi::{hwtcon, mxcfb};
use kobo_profile::{DeviceProfile, FramebufferController};

/// Which panel-controller interface a device speaks.
///
/// This is not a preference. It is a property of the hardware, read from the
/// profile's framebuffer identifier, and the two interfaces are not
/// interchangeable: they number their waveforms differently, so a plan built
/// for one backend and submitted to the other draws the wrong thing without
/// reporting anything. Keeping the choice in one enum is what stops a waveform
/// constant from being written down anywhere a backend is not also named.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// `MediaTek` panel controller, as on the Clara BW.
    Hwtcon,
    /// i.MX6 EPDC, as on the Mark 7 devices.
    Mxcfb,
}

impl Backend {
    /// Resolves the backend from a framebuffer identifier, as reported by the
    /// kernel in `fb_fix_screeninfo.id`.
    ///
    /// An unknown identifier is `None` rather than a guess, in keeping with
    /// the rest of the device model: an unrecognised panel is refused, not
    /// approximated.
    #[must_use]
    pub fn from_framebuffer_id(id: &str) -> Option<Self> {
        match id {
            "hwtcon" => Some(Self::Hwtcon),
            "mxc_epdc_fb" => Some(Self::Mxcfb),
            _ => None,
        }
    }

    /// Resolves the backend a profile describes.
    #[must_use]
    pub fn from_profile(profile: &DeviceProfile) -> Option<Self> {
        Some(Self::from_controller(profile.framebuffer_controller))
    }

    /// The backend that speaks a profile's declared controller interface.
    ///
    /// Total, unlike the string lookups: a profile cannot exist without
    /// declaring its controller.
    #[must_use]
    pub const fn from_controller(controller: FramebufferController) -> Self {
        match controller {
            FramebufferController::Hwtcon => Self::Hwtcon,
            FramebufferController::MxcfbV2 => Self::Mxcfb,
        }
    }

    /// The waveform number this backend uses for `intent`.
    ///
    /// Only the three waveforms every known panel has are reachable from a
    /// [`RefreshIntent`], which is what makes this mapping safe to write down
    /// once. `GC4` in particular is not dependable across firmware versions
    /// and is deliberately not offered.
    ///
    /// On i.MX6 the driver may still not run what is asked for. There is a
    /// block in `mxc_epdc_v2_fb.c` that substitutes `GLD16` or `GLR16` for a
    /// requested `GC16`, which would mean a quality refresh never actually
    /// clears ghosting. It sits behind `NTX_WFM_MODE_OPTIMIZED`, and that
    /// macro is not defined anywhere in the Libra 2 kernel tree, so the code
    /// is dead on this device. `tools/abi/check-mxcfb.sh` fails if a kernel
    /// ever turns it on.
    #[must_use]
    pub fn waveform(self, intent: RefreshIntent) -> u32 {
        match self {
            Self::Hwtcon => match intent {
                RefreshIntent::FastFeedback => hwtcon::WAVEFORM_DU,
                RefreshIntent::TextContent => hwtcon::WAVEFORM_GL16,
                RefreshIntent::QualityContent => hwtcon::WAVEFORM_GC16,
                RefreshIntent::ColorContent => hwtcon::WAVEFORM_GCC16,
            },
            Self::Mxcfb => match intent {
                RefreshIntent::FastFeedback => mxcfb::WAVEFORM_DU,
                RefreshIntent::TextContent => mxcfb::WAVEFORM_GL16,
                RefreshIntent::QualityContent | RefreshIntent::ColorContent => mxcfb::WAVEFORM_GC16,
            },
        }
    }
}

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

    /// Whether two non-empty panel regions share at least one pixel.
    #[must_use]
    pub fn intersects(self, other: Self) -> bool {
        self.width > 0
            && self.height > 0
            && other.width > 0
            && other.height > 0
            && self.x < other.x.saturating_add(other.width)
            && other.x < self.x.saturating_add(self.width)
            && self.y < other.y.saturating_add(other.height)
            && other.y < self.y.saturating_add(self.height)
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
    /// A color image on a panel with a color filter array.
    ColorContent,
}

/// A clipped region and what is to be done to it.
///
/// The plan deliberately carries the *intent* and not a waveform number.
/// Callers describe the change they made; only the display session, which
/// knows the profile, turns that into a number for a particular panel
/// controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefreshPlan {
    pub region: Rect,
    pub intent: RefreshIntent,
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
            intent,
            full,
        })
    }

    /// The waveform number `backend` uses for this plan.
    #[must_use]
    pub fn waveform(self, backend: Backend) -> u32 {
        backend.waveform(self.intent)
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
            waveform_mode: self.waveform(Backend::Hwtcon),
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

    /// The same plan as an i.MX6 EPDC update.
    ///
    /// `temp` asks the controller for its own ambient reading, which is what
    /// every other consumer of this interface sends and the only value here
    /// that is not simply zero. Dithering is left off and the alternate buffer
    /// unused: updates come from the framebuffer itself.
    #[must_use]
    pub fn mxcfb_update_data(self, marker: u32) -> mxcfb::MxcfbUpdateData {
        mxcfb::MxcfbUpdateData {
            update_region: mxcfb::MxcfbRect {
                top: self.region.y,
                left: self.region.x,
                width: self.region.width,
                height: self.region.height,
            },
            waveform_mode: self.waveform(Backend::Mxcfb),
            update_mode: if self.full {
                mxcfb::UPDATE_MODE_FULL
            } else {
                mxcfb::UPDATE_MODE_PARTIAL
            },
            update_marker: marker,
            temp: mxcfb::TEMP_USE_AMBIENT,
            flags: 0,
            dither_mode: 0,
            quant_bit: 0,
            alt_buffer_data: mxcfb::MxcfbAltBufferData {
                phys_addr: 0,
                width: 0,
                height: 0,
                alt_update_region: mxcfb::MxcfbRect {
                    top: 0,
                    left: 0,
                    width: 0,
                    height: 0,
                },
            },
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
    use super::{Backend, Rect, RefreshIntent, RefreshPlan, UpdateMarker};
    use kobo_abi::{hwtcon, mxcfb};

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
        assert_eq!(plan.waveform(Backend::Hwtcon), hwtcon::WAVEFORM_GC16);
    }

    /// The reason the plan stopped carrying a waveform number. Both panels are
    /// being asked for the same thing, and both are being told a different
    /// number. Had the hwtcon number been sent to an i.MX6 panel, it would have
    /// run `GC4` and reported success.
    #[test]
    fn one_intent_becomes_a_different_number_on_each_backend() {
        let plan = RefreshPlan::new(
            Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            RefreshIntent::TextContent,
            false,
            1264,
            1680,
        )
        .expect("region intersects screen");

        assert_eq!(plan.waveform(Backend::Hwtcon), hwtcon::WAVEFORM_GL16);
        assert_eq!(plan.waveform(Backend::Mxcfb), mxcfb::WAVEFORM_GL16);
        assert_ne!(
            plan.waveform(Backend::Hwtcon),
            plan.waveform(Backend::Mxcfb)
        );
    }

    #[test]
    fn color_intent_uses_kaleido_waveform_only_on_hwtcon() {
        let plan = RefreshPlan::new(
            Rect {
                x: 0,
                y: 0,
                width: 1264,
                height: 1680,
            },
            RefreshIntent::ColorContent,
            true,
            1264,
            1680,
        )
        .expect("whole panel");
        assert_eq!(plan.waveform(Backend::Hwtcon), hwtcon::WAVEFORM_GCC16);
        assert_eq!(plan.waveform(Backend::Mxcfb), mxcfb::WAVEFORM_GC16);
    }

    #[test]
    fn a_backend_comes_from_the_hardware_and_is_never_guessed() {
        assert_eq!(
            Backend::from_framebuffer_id("hwtcon"),
            Some(Backend::Hwtcon)
        );
        assert_eq!(
            Backend::from_framebuffer_id("mxc_epdc_fb"),
            Some(Backend::Mxcfb)
        );
        assert_eq!(Backend::from_framebuffer_id("mxc_epdc_fb2"), None);
        assert_eq!(Backend::from_framebuffer_id(""), None);
    }

    #[test]
    fn an_mxcfb_update_asks_the_panel_for_its_own_temperature() {
        let plan = RefreshPlan::new(
            Rect {
                x: 8,
                y: 16,
                width: 32,
                height: 48,
            },
            RefreshIntent::QualityContent,
            true,
            1264,
            1680,
        )
        .expect("region intersects screen");
        let update = plan.mxcfb_update_data(0x4000_0001);

        assert_eq!(update.update_region.left, 8);
        assert_eq!(update.update_region.top, 16);
        assert_eq!(update.waveform_mode, mxcfb::WAVEFORM_GC16);
        assert_eq!(update.update_mode, mxcfb::UPDATE_MODE_FULL);
        assert_eq!(update.update_marker, 0x4000_0001);
        assert_eq!(update.temp, mxcfb::TEMP_USE_AMBIENT);
        assert_eq!(update.flags, 0);
        assert_eq!(update.dither_mode, 0);
        assert_eq!(update.quant_bit, 0);
        assert_eq!(update.alt_buffer_data, mxcfb::MxcfbAltBufferData::default());
    }

    #[test]
    fn markers_never_emit_zero() {
        let mut markers = UpdateMarker::new(u32::MAX);
        assert_eq!(markers.take(), u32::MAX);
        assert_eq!(markers.take(), 1);
    }

    #[test]
    fn region_intersection_excludes_edges_and_empty_rectangles() {
        let left = Rect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert!(left.intersects(Rect {
            x: 39,
            y: 59,
            width: 2,
            height: 2,
        }));
        assert!(!left.intersects(Rect {
            x: 40,
            y: 20,
            width: 1,
            height: 40,
        }));
        assert!(!left.intersects(Rect {
            x: 10,
            y: 20,
            width: 0,
            height: 40,
        }));
    }
}
