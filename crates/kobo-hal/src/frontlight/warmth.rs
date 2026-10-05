//! Profile-gated warmth; discovering a color file never establishes direction.
use super::{read_number, to_percent, to_raw, Balance, Frontlight, PREFERRED};
use kobo_profile::{WarmthControl, WarmthDirection};
use std::fs;
use std::io::{self, Write};

impl Frontlight {
    fn warmth_control(
        &self,
        mapping: Option<WarmthControl>,
    ) -> io::Result<(Balance, WarmthDirection, u32)> {
        let Some(WarmthControl::Lm3630aColor { direction }) = mapping else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "warmth is unmeasured",
            ));
        };
        if self
            .control
            .file_name()
            .is_none_or(|name| name != PREFERRED)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "warmth topology mismatch",
            ));
        }
        let balance = self.balance.ok_or_else(|| {
            io::Error::new(io::ErrorKind::Unsupported, "no captured warmth control")
        })?;
        let raw = read_number(&self.control.join("color"))
            .ok_or_else(|| io::Error::other("missing or invalid warmth control"))?;
        if read_number(&self.control.join("max_color")) != Some(balance.maximum)
            || raw > balance.maximum
        {
            return Err(io::Error::other("warmth control changed since capture"));
        }
        Ok((balance, direction, raw))
    }

    /// Read device-neutral warmth: 0 coolest, 100 warmest, rounded to nearest.
    ///
    /// # Errors
    /// Refuses unmeasured profiles, a different controller, missing files or
    /// changed/invalid ranges. The mapping must come from the device profile.
    pub fn warmth(&self, mapping: Option<WarmthControl>) -> io::Result<u8> {
        let (balance, direction, raw) = self.warmth_control(mapping)?;
        Ok(to_percent(
            from_direction(raw, balance.maximum, direction),
            balance.maximum,
        ))
    }

    /// Set warmth independently of brightness, clamping input to 0..=100.
    /// Explicit warmth wins over the brightness-100 midpoint rule until restore.
    /// The immutable owner capture and crash recovery record are never changed.
    ///
    /// # Errors
    /// As [`Self::warmth`], plus brightness validation and control write failures.
    /// A driver failure can leave a partial change; session recovery still owns
    /// the original values, and a successful color write remains explicit.
    pub fn set_warmth(&self, mapping: Option<WarmthControl>, percent: u8) -> io::Result<u8> {
        let (balance, direction, _) = self.warmth_control(mapping)?;
        let brightness = read_number(&self.control.join("brightness"))
            .ok_or_else(|| io::Error::other("missing or invalid brightness control"))?;
        if read_number(&self.control.join("max_brightness")) != Some(self.maximum)
            || brightness > self.maximum
        {
            return Err(io::Error::other("brightness control changed since capture"));
        }
        let raw = from_direction(to_raw(percent, balance.maximum), balance.maximum, direction);
        self.balance(raw)?;
        self.explicit_warmth.set(Some(raw));
        // The driver applies color when brightness is written. Preserve its
        // exact raw value, including off (0), without percentage rounding.
        fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(self.control.join("brightness"))?
            .write_all(format!("{brightness}\n").as_bytes())?;
        self.warmth(mapping)
    }
}

fn from_direction(raw: u32, maximum: u32, direction: WarmthDirection) -> u32 {
    match direction {
        WarmthDirection::Increasing => raw,
        WarmthDirection::Decreasing => maximum - raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontlight::tests::{banks, control, scratch};
    use kobo_profile::{DeviceProfile, CLARA_BW_391, SUPPORTED_PROFILES};

    fn profile(direction: WarmthDirection) -> DeviceProfile {
        // Synthetic fixture, not evidence about the real Clara's warm bank.
        DeviceProfile {
            warmth: Some(WarmthControl::Lm3630aColor { direction }),
            ..CLARA_BW_391
        }
    }

    #[test]
    fn every_shipped_profile_is_unmeasured_and_refuses_without_writes() {
        let root = scratch("warmth-unmeasured");
        control(&root, PREFERRED, "17", "100");
        banks(&root, PREFERRED, "3", "10");
        let light = Frontlight::open_in(&root).unwrap();
        for profile in SUPPORTED_PROFILES {
            assert_eq!(
                profile.warmth, None,
                "{} needs attended evidence",
                profile.id
            );
            assert_eq!(
                light.warmth(profile.warmth).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
            assert!(light.set_warmth(profile.warmth, 100).is_err());
        }
        assert_eq!(read_number(&root.join(PREFERRED).join("color")), Some(3));
        assert_eq!(light.percent(), Some(17));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn synthetic_profiles_map_both_directions_and_quantize_without_overflow() {
        let root = scratch("warmth-scaling");
        for maximum in [10, 255, u32::MAX] {
            for direction in [WarmthDirection::Increasing, WarmthDirection::Decreasing] {
                let profile = profile(direction);
                control(&root, PREFERRED, "1", "255");
                banks(&root, PREFERRED, "0", &maximum.to_string());
                let light = Frontlight::open_in(&root).unwrap();
                for percent in [0, 1, 25, 50, 99, 100, 101, u8::MAX] {
                    let expected = to_raw(percent, maximum);
                    let actual = light.set_warmth(profile.warmth, percent).unwrap();
                    assert_eq!(actual, to_percent(expected, maximum));
                    assert_eq!(light.warmth(profile.warmth).unwrap(), actual);
                    assert_eq!(
                        read_number(&root.join(PREFERRED).join("color")),
                        Some(match direction {
                            WarmthDirection::Increasing => expected,
                            WarmthDirection::Decreasing => maximum - expected,
                        })
                    );
                    assert_eq!(
                        read_number(&root.join(PREFERRED).join("brightness")),
                        Some(1)
                    );
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_warmth_wins_at_every_brightness_and_normal_and_crash_restore_keep_owner_values() {
        let root = scratch("warmth-restore");
        let journal = root.join("session");
        fs::create_dir(&journal).unwrap();
        control(&root, PREFERRED, "1", "255");
        banks(&root, PREFERRED, "3", "10");
        let profile = profile(WarmthDirection::Increasing);
        let light = Frontlight::open_in(&root).unwrap();
        light.save_recovery(&journal).unwrap();
        let record = fs::read(journal.join("frontlight")).unwrap();
        light.set(100).unwrap();
        light.set_warmth(profile.warmth, 80).unwrap();
        for level in [100, 40, 0, 100] {
            light.set(level).unwrap();
            assert_eq!(light.warmth(profile.warmth).unwrap(), 80);
        }
        light.set(0).unwrap();
        light.set_warmth(profile.warmth, 20).unwrap();
        assert_eq!(
            light.percent(),
            Some(0),
            "warmth must not turn an off light on"
        );
        light.restore().unwrap();
        assert_eq!(
            read_number(&root.join(PREFERRED).join("brightness")),
            Some(1)
        );
        assert_eq!(light.warmth(profile.warmth).unwrap(), 30);
        light.set(100).unwrap();
        assert_eq!(
            light.warmth(profile.warmth).unwrap(),
            50,
            "restore clears the explicit override"
        );
        light.set_warmth(profile.warmth, 100).unwrap();
        assert_eq!(fs::read(journal.join("frontlight")).unwrap(), record);
        drop(light);
        assert!(Frontlight::recover_in(&root, &journal).unwrap());
        assert_eq!(
            read_number(&root.join(PREFERRED).join("brightness")),
            Some(1)
        );
        assert_eq!(read_number(&root.join(PREFERRED).join("color")), Some(3));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_malformed_or_changed_controls_fail_without_touching_brightness() {
        let root = scratch("warmth-invalid");
        let mapping = profile(WarmthDirection::Increasing).warmth;
        for (file, value) in [
            ("color", None),
            ("max_color", None),
            ("color", Some("11")),
            ("color", Some("bad")),
            ("max_color", Some("0")),
            ("max_color", Some("20")),
        ] {
            control(&root, PREFERRED, "17", "100");
            banks(&root, PREFERRED, "3", "10");
            let light = Frontlight::open_in(&root).unwrap();
            let path = root.join(PREFERRED).join(file);
            if let Some(value) = value {
                fs::write(&path, value).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(light.warmth(mapping).is_err());
            assert!(light.set_warmth(mapping, 100).is_err());
            assert_eq!(light.percent(), Some(17));
            assert_eq!(fs::read_to_string(path).ok().as_deref(), value);
            assert_eq!(light.explicit_warmth.get(), None);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_capture_or_wrong_topology_never_enables_warmth() {
        let root = scratch("warmth-topology");
        let mapping = profile(WarmthDirection::Decreasing).warmth;
        control(&root, "other", "17", "100");
        banks(&root, "other", "3", "10");
        let light = Frontlight::open_in(&root).unwrap();
        assert!(light.set_warmth(mapping, 100).is_err());
        control(&root, PREFERRED, "17", "100");
        let light = Frontlight::open_in(&root).unwrap();
        banks(&root, PREFERRED, "3", "10");
        assert!(
            light.set_warmth(mapping, 100).is_err(),
            "late files were not captured for recovery"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
