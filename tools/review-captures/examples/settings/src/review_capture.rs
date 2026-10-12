//! Actual app/renderer snapshots with synthetic status, not simulator captures.
use super::*;
use kobo_sdk::AppRunner;
use kobo_ui::{DisplayMetrics, TextScale, CLARA_BW_METRICS};
#[path = "../screenshots/ui-review/render.rs"]
mod render_capture;

#[test]
#[ignore]
fn capture_review_screens() {
    for text_scale in [TextScale::Default, TextScale::Largest] {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        kobo_text::install(metrics).unwrap();
        let runner = AppRunner::with_metrics(Settings::default(), metrics);
        let context = runner.context();
        let mut app = Settings::default();
        let size = text_scale.percent();
        render_capture::capture_at(
            "settings",
            &format!("{size}-home"),
            &app.home_for(&context),
            metrics,
            &(),
        );
        app.home_page = app.home_pages(&context).len() - 1;
        render_capture::capture_at(
            "settings",
            &format!("{size}-home-last"),
            &app.home_for(&context),
            metrics,
            &(),
        );
        app.wifi_state = RadioState::On;
        app.networks = (0..10)
            .map(|index| WifiNetwork {
                ssid: format!("Example network in the library {index}"),
                signal_dbm: -55,
                secured: true,
                connected: false,
            })
            .collect();
        render_capture::capture_at(
            "settings",
            &format!("{size}-wifi"),
            &app.wifi_for(&context),
            metrics,
            &(),
        );
        app.wifi_page = app.wifi_pages(&context).len() - 1;
        render_capture::capture_at(
            "settings",
            &format!("{size}-wifi-last"),
            &app.wifi_for(&context),
            metrics,
            &(),
        );
        app.bluetooth_state = RadioState::On;
        app.devices = (0..8)
            .map(|index| BluetoothDevice {
                address: format!("00:00:00:00:00:{index:02}"),
                name: format!("Headphones {index}"),
                kind: kobo_sdk::BluetoothDeviceKind::Audio,
                paired: true,
                connected: index == 0,
            })
            .collect();
        render_capture::capture_at(
            "settings",
            &format!("{size}-bluetooth"),
            &app.bluetooth_for(&context),
            metrics,
            &(),
        );
        app.bluetooth_page = app.bluetooth_pages(&context).len() - 1;
        render_capture::capture_at(
            "settings",
            &format!("{size}-bluetooth-last"),
            &app.bluetooth_for(&context),
            metrics,
            &(),
        );
        app.bluetooth_page = 0;
        app.restart_on_exit = true;
        render_capture::capture_at(
            "settings",
            &format!("{size}-bluetooth-restart"),
            &app.bluetooth_for(&context),
            metrics,
            &(),
        );
    }
    // Exercise the helper's default-metric entry point as well.
    render_capture::capture(
        "settings",
        "default-home",
        &Settings::default().home_for(&Context::default()),
    );
}
