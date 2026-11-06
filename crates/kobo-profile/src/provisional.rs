//! A profile assembled from what an unmeasured device says about itself.
//!
//! Every profile in [`crate::SUPPORTED_PROFILES`] is a record of somebody
//! holding the hardware: the panel was written to, corners were tapped, the
//! reader was handed back. That is why the table is short, and why owners of a
//! Kobo nobody here has ever touched were refused outright.
//!
//! Refusing them was the wrong trade. Almost everything a profile states is
//! read back from the kernel at probe time and needs no witness: the
//! framebuffer's geometry, its pixel layout, the touch controller's name and
//! the ranges it reports. What genuinely cannot be derived is how the digitiser
//! is mounted underneath the panel, which is a fact about solder and no
//! register records it.
//!
//! So a provisional profile carries the derived fields and leaves that one gap
//! honest. It is never `write_ready`, so it cannot reach a panel until the
//! owner has been told what it is and has said yes. This path also requires
//! qualified display ABI evidence; consent cannot qualify an unknown driver.

use crate::{
    firmware_branch, DeviceProfile, DeviceSnapshot, FramebufferController, GeometryRule,
    LightLayout, TouchMapping, TouchTransform, SUPPORTED_PROFILES,
};

/// The pixel density assumed when no measured profile shares this resolution.
///
/// Six-inch panels at 1072x1448 and 1264x1680 are the overwhelming majority of
/// the readers Kobo has shipped, and 300 is what both report. Guessing it wrong
/// costs text drawn at the wrong size, which is legible and obviously wrong,
/// rather than anything that touches the hardware.
const ASSUMED_PIXELS_PER_INCH: u16 = 300;

/// The `id` every provisional profile carries.
///
/// Callers that have to tell a measured profile from a derived one compare
/// against this rather than against a null field, so the distinction cannot be
/// lost by a struct that gets copied around.
pub const PROVISIONAL_ID: &str = "provisional";

/// Builds a profile for a device no entry in the table claims.
///
/// # Errors
///
/// When the probe returned no framebuffer or no touch device, since neither
/// can be guessed, or when the device tree names a `SoC` family whose update
/// interface this build does not implement.
pub fn profile_from_probe(
    snapshot: &DeviceSnapshot,
    touch_transform: TouchTransform,
) -> Result<&'static DeviceProfile, String> {
    let framebuffer = snapshot
        .framebuffer
        .as_ref()
        .ok_or_else(|| "the framebuffer could not be probed".to_owned())?;
    let touch = snapshot
        .touch
        .as_ref()
        .ok_or_else(|| "the touch controller could not be probed".to_owned())?;
    let identity = &snapshot.identity;
    let firmware = identity
        .firmware_version
        .as_deref()
        .ok_or_else(|| "the firmware version could not be read".to_owned())?;
    if firmware_branch(firmware).is_none() {
        return Err(format!(
            "the firmware version {firmware} names no release branch"
        ));
    }
    let controller = controller_for(&snapshot.compatible, &framebuffer.id)?;

    let profile = DeviceProfile {
        id: PROVISIONAL_ID,
        model: leak(
            snapshot
                .model
                .clone()
                .unwrap_or_else(|| "Unrecognised Kobo".to_owned()),
        ),
        device_code: identity.device_code.unwrap_or_default(),
        device_tree_model: leak(snapshot.model.clone().unwrap_or_default()),
        // Nothing to require. The fragments exist so a measured profile can
        // refuse a device that merely shares a resolution with it, and a
        // profile derived from this device cannot mistake it for another one.
        compatible_fragments: &[],
        framebuffer_id: leak(framebuffer.id.clone()),
        framebuffer_controller: controller,
        width: framebuffer.width,
        height: framebuffer.height,
        pixels_per_inch: assumed_density(framebuffer.width, framebuffer.height),
        virtual_width: framebuffer.virtual_width,
        virtual_height: framebuffer.virtual_height,
        x_offset: framebuffer.x_offset,
        y_offset: framebuffer.y_offset,
        bits_per_pixel: framebuffer.bits_per_pixel,
        grayscale: framebuffer.grayscale,
        stride: framebuffer.stride,
        memory_length: framebuffer.memory_length,
        framebuffer_kind: framebuffer.kind,
        framebuffer_visual: framebuffer.visual,
        rotation: framebuffer.rotation,
        red: framebuffer.red,
        green: framebuffer.green,
        blue: framebuffer.blue,
        alpha: framebuffer.alpha,
        // The one field with no derivation. Supplied by the caller from the
        // two-tap calibration the owner performed, or left at the caller's
        // starting point until they have done one.
        touch_transform,
        // The pose the device is in right now is the only one anything here
        // has been seen at, so it is both the anchor and the whole verified
        // set. Turning an unmeasured reader over is refused by
        // [`crate::PanelPose::resolve`] rather than guessed at.
        reference_rotation: framebuffer.rotation,
        verified_rotations: leak_rotations(framebuffer.rotation),
        // The derived rules belong to driver families that were read out of
        // vendor source for a known SoC. Nothing here knows this device well
        // enough to apply one, and exact-matching what was probed cannot be
        // wrong about a device it was probed from.
        geometry_rule: GeometryRule::Fixed,
        touch_name: leak(touch.name.clone()),
        touch_x_min: touch.x_min,
        touch_x_max: touch.x_max,
        touch_y_min: touch.y_min,
        touch_y_max: touch.y_max,
        serial_prefix: leak(identity.serial_prefix.clone().unwrap_or_default()),
        firmware_versions: leak_versions(firmware),
        kernel_release: leak(identity.kernel_release.clone().unwrap_or_default()),
        // Never. This is the field that says a person watched the panel take a
        // write, and deriving it from a probe would make the word meaningless.
        // The consent path strips the resulting blocker once the owner has
        // been told what they are agreeing to.
        write_ready: false,
        // Which daemons Nickel leaves holding the radio is a per-model
        // observation. Getting it wrong on a device nobody has watched would
        // kill a supplicant that the reader still wanted, so the hand-back
        // reaps nothing and the worst case is Wi-Fi that needs a restart.
        leftover_radio_daemons: &[],
        reap_nickel_supplicant: false,
        // Claiming a Kaleido filter that is not there tints every page of a
        // monochrome reader, while missing one that is costs only colour. The
        // asymmetry decides this on hardware nobody has looked at.
        colour_panel: false,
        // The i.MX layout keeps the warmth untouched, which is the safe side
        // on a board nobody has watched. The aggregate layout is what every
        // other family has been measured with.
        front_light: LightLayout::from_compatible(&snapshot.compatible),
    };
    Ok(Box::leak(Box::new(profile)))
}

/// Rebuilds a provisional profile around the owner's touch calibration.
///
/// Refuses any profile that is not provisional. A measured profile's
/// transform was checked against the hardware by somebody holding it, and two
/// taps from a session that resolved to that profile are not evidence enough
/// to overrule it.
///
/// # Errors
///
/// When `profile` is a measured one, or the probe no longer derives a profile.
pub fn recalibrated(
    profile: &DeviceProfile,
    snapshot: &DeviceSnapshot,
    mapping: TouchMapping,
) -> Result<&'static DeviceProfile, String> {
    if profile.id != PROVISIONAL_ID {
        return Err(format!(
            "{} is a measured profile, and its touch transform is never replaced by a calibration",
            profile.id
        ));
    }
    profile_from_probe(snapshot, TouchTransform::Calibrated(mapping))
}

/// The i.MX device-tree roots whose EPDC driver takes the 72-byte update.
///
/// The i.MX6SLL readers and the i.MX6ULL Nia ship the 4.1.15 EPDC driver with
/// the 72-byte request. The i.MX6SL and i.MX50 readers before them use the
/// 68-byte request under the same `mxc_epdc_fb` name, which is why a family
/// prefix cannot stand in for these exact roots.
const MXCFB_V2_ROOTS: [&str; 3] = ["fsl,imx6sll", "fsl,imx6sll-lpddr3-arm2", "fsl,imx6ull"];

/// A platform family whose display driver this build does not speak.
///
/// Neither family is attempted: a wrong guess at an update struct is read
/// past by the driver, and nothing catches that. The reason is worded once,
/// here, so the reader and the setup command on the computer say the same
/// thing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedFamily {
    /// The Allwinner readers, the Elipsa and the Sage.
    Allwinner,
    /// The i.MX50 and i.MX6SL readers on the older 16-bit display driver.
    OlderImx,
}

impl UnsupportedFamily {
    /// The plain reason an owner is given.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Allwinner => {
                "this Kobo model is not supported yet. It uses an Allwinner display controller, \
                 which Cobalt cannot drive"
            }
            Self::OlderImx => {
                "this Kobo model is not supported yet. It uses an older i.MX display driver with \
                 16-bit pixels, which Cobalt cannot drive"
            }
        }
    }

    /// The family a Kobo product id belongs to, when it is one of these.
    ///
    /// The product id is the device code at the end of the reader's version
    /// file. Any id not listed here is left to the checks on the reader, so
    /// a model this table does not know is never refused by it.
    #[must_use]
    pub const fn from_product_id(id: u16) -> Option<Self> {
        match id {
            // Sage and Elipsa.
            383 | 387 => Some(Self::Allwinner),
            // Touch, Glo, Mini, Aura HD, Aura, Aura H2O, Glo HD, Touch 2.0,
            // Aura One, the first revisions of the Aura H2O Edition 2 and
            // Aura Edition 2, and the Aura One Limited Edition.
            310 | 320 | 330 | 340 | 350 | 360 | 370 | 371 | 372 | 373 | 374 | 375 | 381 => {
                Some(Self::OlderImx)
            }
            _ => None,
        }
    }

    /// The family a device tree's compatible list names, when it is one of
    /// these.
    #[must_use]
    pub fn from_compatible(compatible: &[String]) -> Option<Self> {
        let allwinner = compatible.iter().any(|name| {
            name.starts_with("allwinner,") || name.contains("sun8i") || name.contains("sunxi")
        });
        let older_imx = compatible.iter().any(|name| {
            name.starts_with("fsl,imx5")
                || ["fsl,imx6sl", "fsl,imx6sx", "fsl,imx6dl", "fsl,imx6q"].contains(&name.as_str())
        });
        let qualified_imx = MXCFB_V2_ROOTS
            .iter()
            .any(|root| compatible.iter().any(|name| name == root));
        if allwinner {
            Some(Self::Allwinner)
        } else if older_imx && !qualified_imx {
            Some(Self::OlderImx)
        } else {
            None
        }
    }
}

/// Select only qualified compatible tokens and the matching framebuffer ID.
/// The same framebuffer ID exists on older, incompatible i.MX drivers.
fn controller_for(
    compatible: &[String],
    framebuffer_id: &str,
) -> Result<FramebufferController, String> {
    if let Some(family) = UnsupportedFamily::from_compatible(compatible) {
        return Err(family.reason().to_owned());
    }
    let has = |token: &str| compatible.iter().any(|name| name == token);
    let imx = MXCFB_V2_ROOTS.iter().any(|root| has(root));
    let mtk = has("mediatek,mt8110") || has("mediatek,mt8512");
    let conflicting = compatible.iter().any(|name| {
        (name.starts_with("fsl,imx") && !MXCFB_V2_ROOTS.contains(&name.as_str()))
            || name.starts_with("allwinner,")
            || (imx && name.starts_with("mediatek,"))
            || (mtk && (name.starts_with("fsl,") || name.starts_with("freescale,")))
    });
    match (imx, mtk, conflicting, framebuffer_id) {
        (true, false, false, "mxc_epdc_fb") => Ok(FramebufferController::MxcfbV2),
        (false, true, false, "hwtcon") => Ok(FramebufferController::Hwtcon),
        _ => Err(
            "no qualified, unambiguous display ABI matches the device tree and framebuffer"
                .to_owned(),
        ),
    }
}

/// The density to assume, borrowed from a measured profile of the same size.
///
/// Resolution tracks panel size closely across the readers Kobo has shipped,
/// so a device that reports another profile's exact geometry almost certainly
/// shares its panel. This borrows only the density: nothing about how that
/// profile's digitiser is mounted comes with it, which is the fact that must
/// never be inherited by proximity.
fn assumed_density(width: u32, height: u32) -> u16 {
    SUPPORTED_PROFILES
        .iter()
        .find(|profile| profile.width == width && profile.height == height)
        .map_or(ASSUMED_PIXELS_PER_INCH, |profile| profile.pixels_per_inch)
}

/// Promotes a probed string to the `'static` every profile field is written in.
///
/// A session builds at most one of these and then runs until the reader is
/// handed back, so the allocation lasts exactly as long as the process that
/// needs it. Threading a lifetime through `DeviceProfile` instead would put one
/// on every measured profile, all of which really are `'static`, to describe a
/// case that occurs once.
fn leak(value: String) -> &'static str {
    Box::leak(value.into_boxed_str())
}

fn leak_versions(firmware: &str) -> &'static [&'static str] {
    Box::leak(vec![leak(firmware.to_owned())].into_boxed_slice())
}

fn leak_rotations(rotation: u32) -> &'static [u32] {
    Box::leak(vec![rotation].into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::{profile_from_probe, ASSUMED_PIXELS_PER_INCH, PROVISIONAL_ID};
    use crate::{
        Bitfield, DeviceProfile, DeviceSnapshot, FramebufferController, FramebufferSnapshot,
        IdentitySnapshot, TouchSnapshot, TouchTransform, WRITE_EVIDENCE_PENDING,
    };

    fn byte(offset: u32) -> Bitfield {
        Bitfield {
            offset,
            length: 8,
            msb_right: 0,
        }
    }

    /// A reader with no entry in the table: an unmeasured i.MX6SLL panel size, and a
    /// firmware branch nothing here has been measured on.
    fn unmeasured_snapshot() -> DeviceSnapshot {
        DeviceSnapshot {
            compatible: vec!["test,unmeasured".to_owned(), "fsl,imx6sll".to_owned()],
            model: Some("Synthetic SLL reader".to_owned()),
            framebuffer: Some(FramebufferSnapshot {
                id: "mxc_epdc_fb".to_owned(),
                width: 758,
                height: 1024,
                virtual_width: 768,
                virtual_height: 2048,
                x_offset: 0,
                y_offset: 0,
                bits_per_pixel: 32,
                grayscale: 0,
                stride: 3072,
                memory_length: 6_291_456,
                kind: 0,
                visual: 2,
                rotation: 3,
                red: byte(0),
                green: byte(8),
                blue: byte(16),
                alpha: byte(24),
            }),
            touch: Some(TouchSnapshot {
                path: "/dev/input/event1".to_owned(),
                name: "elan-touchscreen".to_owned(),
                x_min: 0,
                x_max: 757,
                y_min: 0,
                y_max: 1023,
            }),
            identity: IdentitySnapshot {
                serial_prefix: Some("TEST".into()),
                firmware_version: Some("4.28.17623".into()),
                kernel_release: Some("4.1.15".into()),
                device_code: Some(999),
            },
        }
    }

    #[test]
    fn ambiguous_legacy_and_mismatched_displays_are_refused() {
        for tokens in [
            vec!["fsl,imx50"],
            vec!["fsl,imx6sl"],
            vec!["fsl,imx6ull", "fsl,imx6sl"],
            vec!["fsl,generic"],
            vec!["freescale"],
            vec!["mediatek,mt8113"],
            vec!["mt8"],
            vec!["mediatek"],
            vec!["prefix-fsl,imx6sll"],
            vec!["fsl,imx6sll", "fsl,imx6sl"],
            vec!["fsl,imx6sll", "mediatek,mt8110"],
        ] {
            let names = tokens.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
            for id in ["mxc_epdc_fb", "hwtcon"] {
                assert!(
                    super::controller_for(&names, id).is_err(),
                    "{tokens:?} {id}"
                );
            }
        }
        assert!(super::controller_for(&["fsl,imx6sll".into()], "hwtcon").is_err());
        assert!(super::controller_for(&["fsl,imx6ull".into()], "hwtcon").is_err());
        assert!(super::controller_for(&["mediatek,mt8512".into()], "mxc_epdc_fb").is_err());
        assert_eq!(
            super::controller_for(&["mediatek,mt8512".into()], "hwtcon").unwrap(),
            FramebufferController::Hwtcon
        );
    }

    #[test]
    fn the_nia_root_selects_the_72_byte_update_its_driver_takes() {
        assert_eq!(
            super::controller_for(&["kobo,nia".into(), "fsl,imx6ull".into()], "mxc_epdc_fb")
                .unwrap(),
            FramebufferController::MxcfbV2
        );
    }

    #[test]
    fn a_derived_profile_agrees_with_the_probe_it_was_derived_from() {
        // The whole safety argument for running on unmeasured hardware: the
        // geometry and touch checks stay as strict as they are for every
        // measured device, and a profile taken from this device passes them
        // because it is describing this device.
        let snapshot = unmeasured_snapshot();
        let profile = profile_from_probe(&snapshot, TouchTransform::Direct).expect("derivable");
        assert!(
            profile.validate(&snapshot).mismatches.is_empty(),
            "{:?}",
            profile.validate(&snapshot).mismatches
        );
    }

    #[test]
    fn a_derived_profile_is_blocked_only_by_the_evidence_nobody_has_gathered() {
        // Identity is copied off the device, so none of it can refuse. What
        // remains is the one blocker that means a person has not watched this
        // panel take a write, which is exactly what consent is asked about.
        let snapshot = unmeasured_snapshot();
        let profile = profile_from_probe(&snapshot, TouchTransform::Direct).expect("derivable");
        assert_eq!(
            profile.validate(&snapshot).write_blockers,
            vec![WRITE_EVIDENCE_PENDING.to_owned()]
        );
        assert!(!profile.write_ready);
        assert_eq!(profile.id, PROVISIONAL_ID);
    }

    #[test]
    fn the_update_interface_follows_the_soc_family_the_device_tree_names() {
        let mut imx = unmeasured_snapshot();
        assert_eq!(
            profile_from_probe(&imx, TouchTransform::Direct)
                .expect("derivable")
                .framebuffer_controller,
            FramebufferController::MxcfbV2
        );

        imx.compatible = vec!["kobo,clara".to_owned(), "mediatek,mt8110".to_owned()];
        imx.framebuffer.as_mut().unwrap().id = "hwtcon".to_owned();
        assert_eq!(
            profile_from_probe(&imx, TouchTransform::Direct)
                .expect("derivable")
                .framebuffer_controller,
            FramebufferController::Hwtcon
        );
    }

    #[test]
    fn an_unrecognised_soc_family_is_refused_rather_than_guessed_at() {
        // Picking the wrong one submits an update struct of the wrong size to
        // a driver that will read past it, which is not a refusal anything
        // could catch and retry.
        let mut snapshot = unmeasured_snapshot();
        snapshot.compatible = vec!["kobo,something-new".to_owned()];
        assert!(profile_from_probe(&snapshot, TouchTransform::Direct).is_err());
    }

    #[test]
    fn density_is_borrowed_from_a_measured_profile_that_shares_the_resolution() {
        let mut snapshot = unmeasured_snapshot();
        assert_eq!(
            profile_from_probe(&snapshot, TouchTransform::Direct)
                .expect("derivable")
                .pixels_per_inch,
            ASSUMED_PIXELS_PER_INCH
        );

        if let Some(framebuffer) = snapshot.framebuffer.as_mut() {
            framebuffer.width = 1072;
            framebuffer.height = 1448;
        }
        assert_eq!(
            profile_from_probe(&snapshot, TouchTransform::Direct)
                .expect("derivable")
                .pixels_per_inch,
            300
        );
    }

    #[test]
    fn only_the_pose_the_reader_was_probed_in_is_treated_as_verified() {
        let snapshot = unmeasured_snapshot();
        let profile = profile_from_probe(&snapshot, TouchTransform::Direct).expect("derivable");
        assert_eq!(profile.verified_rotations, &[3]);
        assert_eq!(profile.reference_rotation, 3);
    }

    #[test]
    fn a_firmware_version_that_names_no_branch_leaves_nothing_to_record_consent_against() {
        let mut snapshot = unmeasured_snapshot();
        snapshot.identity.firmware_version = Some("unknown".into());
        assert!(profile_from_probe(&snapshot, TouchTransform::Direct).is_err());
    }
    #[test]
    fn a_calibration_replaces_only_the_touch_transform_of_a_provisional_profile() {
        let snapshot = unmeasured_snapshot();
        let start = profile_from_probe(&snapshot, TouchTransform::Direct).expect("derivable");
        let mapping = crate::calibration::ALL_MAPPINGS[6];
        let calibrated = super::recalibrated(start, &snapshot, mapping).expect("provisional");
        assert_eq!(
            calibrated.touch_transform,
            TouchTransform::Calibrated(mapping)
        );
        assert_eq!(
            DeviceProfile {
                touch_transform: start.touch_transform,
                ..*calibrated
            },
            *start,
            "nothing but the transform moved"
        );
        assert!(calibrated.validate(&snapshot).mismatches.is_empty());
    }

    #[test]
    fn a_measured_profile_is_never_recalibrated() {
        for profile in crate::SUPPORTED_PROFILES {
            assert!(
                super::recalibrated(
                    profile,
                    &unmeasured_snapshot(),
                    crate::calibration::ALL_MAPPINGS[0]
                )
                .is_err(),
                "{} accepted a calibration",
                profile.id
            );
        }
    }
    #[test]
    fn an_allwinner_reader_is_told_plainly_that_it_is_not_supported_yet() {
        for tokens in [
            vec!["kobo,elipsa", "allwinner,sun8iw15p1"],
            vec!["allwinner,sun8i"],
        ] {
            let names = tokens.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
            let reason = super::controller_for(&names, "sunxi-fb").expect_err("refused");
            assert!(reason.contains("not supported yet"), "{reason}");
            assert!(reason.contains("Allwinner"), "{reason}");
        }
    }

    #[test]
    fn an_older_imx_reader_is_told_plainly_that_it_is_not_supported_yet() {
        for root in ["fsl,imx50", "fsl,imx6sl"] {
            let reason =
                super::controller_for(&[root.to_owned()], "mxc_epdc_fb").expect_err("refused");
            assert!(reason.contains("not supported yet"), "{root}: {reason}");
            assert!(reason.contains("older i.MX"), "{root}: {reason}");
        }
    }

    #[test]
    fn the_imx6sll_and_imx6ull_readers_take_the_provisional_path() {
        for root in ["fsl,imx6sll", "fsl,imx6ull"] {
            assert_eq!(
                super::controller_for(&[root.to_owned()], "mxc_epdc_fb"),
                Ok(FramebufferController::MxcfbV2),
                "{root}"
            );
        }
    }
    #[test]
    fn product_ids_name_the_unsupported_families_and_nothing_else() {
        use super::UnsupportedFamily;
        assert_eq!(
            UnsupportedFamily::from_product_id(383),
            Some(UnsupportedFamily::Allwinner)
        );
        assert_eq!(
            UnsupportedFamily::from_product_id(387),
            Some(UnsupportedFamily::Allwinner)
        );
        for older in [
            310, 320, 330, 340, 350, 360, 370, 371, 372, 373, 374, 375, 381,
        ] {
            assert_eq!(
                UnsupportedFamily::from_product_id(older),
                Some(UnsupportedFamily::OlderImx),
                "{older}"
            );
        }
        // The i.MX6SLL and i.MX6ULL readers, every measured profile, and ids
        // nobody has listed are left to the reader's own checks.
        for allowed in [376, 377, 378, 379, 380, 382, 384, 386, 999] {
            assert_eq!(
                UnsupportedFamily::from_product_id(allowed),
                None,
                "{allowed}"
            );
        }
        for profile in crate::SUPPORTED_PROFILES {
            assert_eq!(
                UnsupportedFamily::from_product_id(profile.device_code),
                None,
                "{}",
                profile.id
            );
        }
    }

    #[test]
    fn the_reader_and_the_product_id_refuse_with_the_same_words() {
        use super::UnsupportedFamily;
        let on_reader =
            super::controller_for(&["allwinner,sun8iw15p1".to_owned()], "fb").expect_err("refused");
        assert_eq!(
            on_reader,
            UnsupportedFamily::from_product_id(387)
                .expect("listed")
                .reason()
        );
        let on_reader =
            super::controller_for(&["fsl,imx6sl".to_owned()], "mxc_epdc_fb").expect_err("refused");
        assert_eq!(
            on_reader,
            UnsupportedFamily::from_product_id(373)
                .expect("listed")
                .reason()
        );
    }

    #[test]
    fn a_provisional_imx_reader_keeps_its_warmth_and_others_use_the_aggregate() {
        let imx =
            profile_from_probe(&unmeasured_snapshot(), TouchTransform::Direct).expect("derivable");
        assert_eq!(imx.front_light, crate::LightLayout::Imx);
        let mut mediatek = unmeasured_snapshot();
        mediatek.compatible = vec!["mediatek,mt8110".to_owned()];
        mediatek.framebuffer.as_mut().unwrap().id = "hwtcon".to_owned();
        let mediatek = profile_from_probe(&mediatek, TouchTransform::Direct).expect("derivable");
        assert_eq!(mediatek.front_light, crate::LightLayout::Aggregate);
    }
}
