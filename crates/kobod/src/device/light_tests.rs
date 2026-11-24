use super::light_request;
use kobo_hal::frontlight::Frontlight;
use kobo_policy::{Backends, Capability, Declared, DeviceServices, PowerPolicy};
use kobo_profile::{Warmth, MEDIATEK_BALANCE};
use kobo_protocol::{DenyReason, DeviceError, DeviceRequest, DeviceResult};
use std::fs;
use std::path::{Path, PathBuf};

/// A backlight directory holding the aggregate control as a mapped reader
/// publishes it: brightness 17 of 100, balance 3 of 10.
fn lamp(name: &str) -> (PathBuf, Frontlight) {
    let root = std::env::temp_dir().join(format!("kobod-light-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let control = root.join("lm3630a_led");
    fs::create_dir_all(&control).expect("create");
    for (file, value) in [
        ("brightness", "17"),
        ("max_brightness", "100"),
        ("color", "3"),
        ("max_color", "10"),
    ] {
        fs::write(control.join(file), value).expect("write");
    }
    let light = Frontlight::open_in(&root).expect("a light");
    (root, light)
}

fn services() -> DeviceServices {
    DeviceServices::new(
        Declared::all(),
        PowerPolicy::DEFAULT,
        Backends::with([Capability::FrontlightControl]),
    )
}

fn file(root: &Path, name: &str) -> String {
    fs::read_to_string(root.join("lm3630a_led").join(name))
        .expect("read")
        .trim()
        .to_owned()
}

/// Puts a directory where a control file was, which is a write no process
/// can make, whoever it runs as.
fn break_file(root: &Path, name: &str) {
    let path = root.join("lm3630a_led").join(name);
    fs::remove_file(&path).expect("remove");
    fs::create_dir(&path).expect("create");
}

#[test]
fn a_brightness_the_lamp_took_is_answered_with_what_it_took() {
    let (root, light) = lamp("set");
    let mut services = services();
    assert_eq!(
        light_request(
            Some(&light),
            MEDIATEK_BALANCE,
            &DeviceRequest::SetFrontlight { percent: 45 },
            &mut services
        ),
        DeviceResult::Frontlight { percent: 45 }
    );
    assert_eq!(file(&root, "brightness"), "45");
    assert_eq!(
        light_request(
            Some(&light),
            MEDIATEK_BALANCE,
            &DeviceRequest::ReadFrontlight,
            &mut services
        ),
        DeviceResult::Frontlight { percent: 45 }
    );
    fs::remove_dir_all(root).expect("remove");
}

/// This used to be traced and then answered with the level asked for, so
/// the page that asked drew a brightness the panel was not at.
#[test]
fn a_brightness_the_lamp_refused_is_answered_as_a_failure() {
    let (root, light) = lamp("set-refused");
    let mut services = services();
    services.observe_frontlight(17);
    break_file(&root, "brightness");
    assert_eq!(
        light_request(
            Some(&light),
            MEDIATEK_BALANCE,
            &DeviceRequest::SetFrontlight { percent: 45 },
            &mut services
        ),
        DeviceResult::Failed(DeviceError::Backend)
    );
    // Nothing that reads the level afterwards is told it moved.
    assert_eq!(services.state().frontlight_percent, 17);
    fs::remove_dir_all(root).expect("remove");
}

#[test]
fn a_warmth_is_answered_from_the_lamp_with_its_place_and_its_settings() {
    let (root, light) = lamp("warmth");
    let mut services = services();
    let ask = |request, services: &mut DeviceServices| {
        light_request(Some(&light), MEDIATEK_BALANCE, &request, services)
    };
    // Zero is the warmest on these readers, so 3 of 10 is 70% warm.
    assert_eq!(
        ask(DeviceRequest::ReadWarmth, &mut services),
        DeviceResult::Warmth {
            percent: 70,
            steps: 11
        }
    );
    assert_eq!(
        ask(DeviceRequest::SetWarmth { percent: 24 }, &mut services),
        DeviceResult::Warmth {
            percent: 20,
            steps: 11
        }
    );
    assert_eq!(file(&root, "color"), "8");
    assert_eq!(file(&root, "brightness"), "17");
    // Chosen now, so the top of the brightness range leaves it alone.
    assert_eq!(
        ask(DeviceRequest::SetFrontlight { percent: 100 }, &mut services),
        DeviceResult::Frontlight { percent: 100 }
    );
    assert_eq!(file(&root, "color"), "8");
    fs::remove_dir_all(root).expect("remove");
}

#[test]
fn a_warmth_the_lamp_refused_is_answered_as_a_failure() {
    let (root, light) = lamp("warmth-refused");
    let mut services = services();
    break_file(&root, "color");
    for request in [
        DeviceRequest::SetWarmth { percent: 100 },
        DeviceRequest::ReadWarmth,
    ] {
        assert_eq!(
            light_request(Some(&light), MEDIATEK_BALANCE, &request, &mut services),
            DeviceResult::Failed(DeviceError::Backend),
            "{request:?}"
        );
    }
    fs::remove_dir_all(root).expect("remove");
}

/// A reader with a light and no mapping keeps its brightness control and is
/// told, as a settled answer, that its warmth is not one this runtime sets.
#[test]
fn a_reader_with_no_warmth_mapping_is_refused_as_unsupported_and_left_alone() {
    let (root, light) = lamp("unmapped");
    let mut services = services();
    for request in [
        DeviceRequest::SetWarmth { percent: 100 },
        DeviceRequest::ReadWarmth,
    ] {
        assert_eq!(
            light_request(Some(&light), Warmth::Unmapped, &request, &mut services),
            DeviceResult::Denied(DenyReason::Unsupported),
            "{request:?}"
        );
        assert_eq!(
            light_request(None, MEDIATEK_BALANCE, &request, &mut services),
            DeviceResult::Denied(DenyReason::Unsupported),
            "{request:?}"
        );
    }
    assert_eq!(file(&root, "color"), "3");
    assert_eq!(
        light_request(
            Some(&light),
            Warmth::Unmapped,
            &DeviceRequest::SetFrontlight { percent: 60 },
            &mut services
        ),
        DeviceResult::Frontlight { percent: 60 }
    );
    fs::remove_dir_all(root).expect("remove");
}
