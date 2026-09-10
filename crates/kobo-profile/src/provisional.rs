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
//! owner has been told what it is and has said yes. What it buys is that an
//! owner of a Nia or an Aura gets a screen and a choice rather than a device
//! that appears to do nothing.

use crate::{
    firmware_branch, DeviceProfile, DeviceSnapshot, FramebufferController, GeometryRule,
    TouchTransform, SUPPORTED_PROFILES,
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
    let controller = controller_for(&snapshot.compatible)?;

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
        // The one field with no derivation. Supplied by the caller from a
        // calibration the owner performed, or left at the caller's default and
        // wrong until they do one.
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
    };
    Ok(Box::leak(Box::new(profile)))
}

/// Which update ABI to use, taken from the `SoC` family the device tree names.
///
/// Derived rather than guessed. The two ABIs disagree about the size of the
/// update struct, so submitting one to a driver expecting the other is not a
/// refusal that can be caught and retried, and the device tree is the only
/// thing that says which is underneath without writing to find out.
fn controller_for(compatible: &[String]) -> Result<FramebufferController, String> {
    let names = compatible.join(" ").to_ascii_lowercase();
    if names.contains("mediatek") || names.contains("mt8") {
        return Ok(FramebufferController::Hwtcon);
    }
    if names.contains("imx6") || names.contains("freescale") || names.contains("fsl,") {
        return Ok(FramebufferController::MxcfbV2);
    }
    Err("the device tree names no SoC family this build has a framebuffer driver for".to_owned())
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
        Bitfield, DeviceSnapshot, FramebufferController, FramebufferSnapshot, IdentitySnapshot,
        TouchSnapshot, TouchTransform, WRITE_EVIDENCE_PENDING,
    };

    fn byte(offset: u32) -> Bitfield {
        Bitfield {
            offset,
            length: 8,
            msb_right: 0,
        }
    }

    /// A reader with no entry in the table: an i.MX6 Aura's panel size, and a
    /// firmware branch nothing here has been measured on.
    fn unmeasured_snapshot() -> DeviceSnapshot {
        DeviceSnapshot {
            compatible: vec!["kobo,aura".to_owned(), "fsl,imx6sl".to_owned()],
            model: Some("Kobo Aura".to_owned()),
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
                serial_prefix: Some("N514".into()),
                firmware_version: Some("4.28.17623".into()),
                kernel_release: Some("3.0.35".into()),
                device_code: Some(310),
            },
        }
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

        imx.compatible = vec!["kobo,clara".to_owned(), "mediatek,mt8113".to_owned()];
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
}
