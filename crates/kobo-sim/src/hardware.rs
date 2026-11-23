//! Explicit observations supplied by the simulator operator.
use kobo_policy::DeviceState;
use kobo_ui::Orientation;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Battery {
        percent: u8,
        charging: bool,
    },
    Frontlight(u8),
    /// The warmth of the front light, 0 at the coolest and 100 at the
    /// warmest. Taken only by a reader whose profile maps a warmth.
    Warmth(u8),
    Cover(bool),
    Orientation(Orientation),
}
impl Change {
    pub fn parse(command: &str) -> Option<Self> {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let percent = |value: &str| value.parse::<u8>().ok().filter(|value| *value <= 100);
        match parts.as_slice() {
            ["battery", value, "charging"] => Some(Self::Battery {
                percent: percent(value)?,
                charging: true,
            }),
            ["battery", value, "unplugged"] => Some(Self::Battery {
                percent: percent(value)?,
                charging: false,
            }),
            ["frontlight", value] => Some(Self::Frontlight(percent(value)?)),
            ["warmth", value] => Some(Self::Warmth(percent(value)?)),
            ["cover", "closed"] => Some(Self::Cover(true)),
            ["cover", "open"] => Some(Self::Cover(false)),
            ["orientation", "portrait"] => Some(Self::Orientation(Orientation::Portrait)),
            ["orientation", "landscape"] => Some(Self::Orientation(Orientation::Landscape)),
            _ => None,
        }
    }
}
/// The device state as the inspector and a route's `expect-state` read it.
///
/// `warmth_steps` is how many settings the selected reader's warmth is
/// modelled with. Without one both warmth fields are null and not a number,
/// so a route cannot mistake a reader whose warmth cannot be set for one
/// that happens to sit at some value.
pub fn json(
    state: DeviceState,
    orientation: Orientation,
    warmth_steps: Option<u8>,
) -> kobo_json::Value {
    let warmth = |value: u8| {
        warmth_steps.map_or(kobo_json::Value::Null, |_| {
            kobo_json::Value::from(u32::from(value))
        })
    };
    kobo_json::ObjectBuilder::new()
        .set("batteryPercent", u32::from(state.battery_percent))
        .set("charging", state.charging)
        .set("frontlightPercent", u32::from(state.frontlight_percent))
        .set("warmthPercent", warmth(state.warmth_percent))
        .set("warmthSteps", warmth(warmth_steps.unwrap_or_default()))
        .set("coverClosed", state.magnet_present)
        .set(
            "orientation",
            if orientation == Orientation::Portrait {
                "portrait"
            } else {
                "landscape"
            },
        )
        .set("frontlightAppearanceCalibrated", false)
        .set("orientationControl", "display-composition")
        .build()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_reject_out_of_range_ambiguous_and_extra_fields() {
        for command in [
            "battery 101 charging",
            "battery -1 unplugged",
            "battery 40 yes",
            "frontlight 256",
            "warmth 101",
            "warmth",
            "warmth warm",
            "cover close",
            "cover open extra",
            "orientation upside-down",
        ] {
            assert_eq!(Change::parse(command), None);
        }
        assert_eq!(
            Change::parse("battery 0 unplugged"),
            Some(Change::Battery {
                percent: 0,
                charging: false
            })
        );
        assert_eq!(
            Change::parse("frontlight 100"),
            Some(Change::Frontlight(100))
        );
        assert_eq!(Change::parse("warmth 0"), Some(Change::Warmth(0)));
        assert_eq!(Change::parse("warmth 100"), Some(Change::Warmth(100)));
    }

    #[test]
    fn a_reader_whose_warmth_cannot_be_set_reports_no_number_for_it() {
        let state = DeviceState {
            warmth_percent: 40,
            ..DeviceState::default()
        };
        let mapped = json(state, Orientation::Portrait, Some(11));
        assert_eq!(
            mapped.get("warmthPercent"),
            Some(&kobo_json::Value::from(40_u32))
        );
        assert_eq!(
            mapped.get("warmthSteps"),
            Some(&kobo_json::Value::from(11_u32))
        );
        let unmapped = json(state, Orientation::Portrait, None);
        assert_eq!(unmapped.get("warmthPercent"), Some(&kobo_json::Value::Null));
        assert_eq!(unmapped.get("warmthSteps"), Some(&kobo_json::Value::Null));
        // The light itself is still there to be set.
        assert!(unmapped.get("frontlightPercent").is_some());
    }
}
