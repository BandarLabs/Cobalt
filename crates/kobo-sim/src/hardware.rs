//! Explicit observations supplied by the simulator operator.
use kobo_policy::DeviceState;
use kobo_ui::Orientation;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Battery { percent: u8, charging: bool },
    Frontlight(u8),
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
            ["cover", "closed"] => Some(Self::Cover(true)),
            ["cover", "open"] => Some(Self::Cover(false)),
            ["orientation", "portrait"] => Some(Self::Orientation(Orientation::Portrait)),
            ["orientation", "landscape"] => Some(Self::Orientation(Orientation::Landscape)),
            _ => None,
        }
    }
}
pub fn json(state: DeviceState, orientation: Orientation) -> kobo_json::Value {
    kobo_json::ObjectBuilder::new()
        .set("batteryPercent", u32::from(state.battery_percent))
        .set("charging", state.charging)
        .set("frontlightPercent", u32::from(state.frontlight_percent))
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
    }
}
