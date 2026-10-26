use super::*;
use kobo_sdk::{AppRunner, Chrome, Command, DisplayMetrics, CLARA_BW_METRICS};

fn interface_sizes() -> Vec<DisplayMetrics> {
    let mut metrics = CLARA_BW_METRICS;
    while let Some(smaller) = metrics.text_scale.smaller() {
        metrics.text_scale = smaller;
    }
    let mut sizes = vec![metrics];
    while let Some(larger) = metrics.text_scale.larger() {
        metrics.text_scale = larger;
        sizes.push(metrics);
    }
    assert_eq!(sizes.len(), 9);
    sizes
}

fn fixture() -> Settings {
    Settings {
        wifi_state: RadioState::On,
        bluetooth_state: RadioState::On,
        networks: (0..13)
            .map(|index| WifiNetwork {
                ssid: format!("Example network in the library {index}"),
                signal_dbm: -55,
                secured: true,
                connected: false,
            })
            .collect(),
        devices: (0..13)
            .map(|index| BluetoothDevice {
                address: format!("00:00:00:00:00:{index:02}"),
                name: format!("Reader headphones with a long name {index}"),
                kind: kobo_sdk::BluetoothDeviceKind::Audio,
                paired: true,
                connected: false,
            })
            .collect(),
        ..Settings::default()
    }
}

fn touch(screen: &Screen, metrics: DisplayMetrics, action: ActionId) -> ActionId {
    let chrome = Chrome::measuring(true);
    let diagnostics = screen.diagnostics(&metrics, &chrome);
    assert!(
        !diagnostics.has_errors(),
        "{metrics:?}: {:?}",
        diagnostics.issues
    );
    let layout = screen.layout_with(&metrics, &chrome);
    let rect = layout
        .rect_of_action(action)
        .expect("action must be on screen");
    assert!(rect.height >= metrics.touch_target_minimum());
    let hit = layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert_eq!(hit, Some(action));
    hit.unwrap()
}

#[test]
fn every_home_destination_is_reachable_at_all_sizes_and_both_poses() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for size in interface_sizes() {
            let metrics = DisplayMetrics {
                width: pose.0,
                height: pose.1,
                ..size
            };
            let runner = AppRunner::with_metrics(Settings::default(), metrics);
            let mut context = runner.context();
            let mut app = fixture();
            let rows = app.home_rows();
            let pages = app.home_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..rows.len()).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.home_page = page;
                for &index in indices {
                    let action = touch(
                        &app.home_for(&context),
                        metrics,
                        action_id(&rows[index].action),
                    );
                    app.on_action(&mut context, action);
                    assert_ne!(app.view, View::Home);
                    app.on_action(&mut context, ActionId::BACK);
                    assert_eq!(app.view, View::Home);
                    assert_eq!(app.home_page, page);
                }
            }
        }
    }
}

#[test]
fn every_measured_radio_row_targets_its_own_identity_on_later_pages() {
    for pose in [(1072, 1448), (1448, 1072)] {
        for size in interface_sizes() {
            let metrics = DisplayMetrics {
                width: pose.0,
                height: pose.1,
                ..size
            };
            let runner = AppRunner::with_metrics(Settings::default(), metrics);
            let mut context = runner.context();
            let mut app = fixture();
            app.view = View::Wifi;
            let pages = app.wifi_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..13).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.wifi_page = page;
                for &index in indices {
                    let expected = app.networks[index].ssid.clone();
                    let action = touch(
                        &app.wifi_for(&context),
                        metrics,
                        action_id(&network_action(&app.networks[index].ssid)),
                    );
                    app.on_action(&mut context, action);
                    assert_eq!(app.view, View::WifiPassword);
                    assert_eq!(app.selected_ssid.as_deref(), Some(expected.as_str()));
                    assert!(!context.take_commands().iter().any(|command| matches!(
                        command,
                        Command::Device(DeviceRequest::JoinWifi { .. })
                    )));
                    app.on_action(&mut context, ActionId::BACK);
                    assert_eq!(app.view, View::Wifi);
                }
            }
            app.view = View::Bluetooth;
            let pages = app.bluetooth_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..13).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.bluetooth_page = page;
                for &index in indices {
                    let expected = app.devices[index].address.clone();
                    let screen = app.bluetooth_for(&context);
                    let action = touch(
                        &screen,
                        metrics,
                        action_id(&bluetooth_action(&app.devices[index].address)),
                    );
                    touch(&screen, metrics, action_id(RESCAN));
                    let _ = context.take_commands();
                    app.on_action(&mut context, action);
                    assert!(context.take_commands().iter().any(|command| matches!(command,Command::Device(DeviceRequest::ConnectBluetooth {address}) if address == &expected)));
                }
            }
        }
    }
}

#[test]
fn radio_notices_and_connected_status_are_included_in_the_page_budget() {
    let metrics = *interface_sizes().last().unwrap();
    let runner = AppRunner::with_metrics(Settings::default(), metrics);
    let context = runner.context();
    for trouble in [false, true] {
        let mut app = fixture();
        app.connected_ssid = Some("Library Wi-Fi".into());
        if trouble {
            app.trouble = Some((Topic::Wifi, "The scan did not finish. Try again.".into()));
        }
        for page in 0..app.wifi_pages(&context).len() {
            app.wifi_page = page;
            let screen = app.wifi_for(&context);
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                !diagnostics.has_errors(),
                "{trouble} page {page}: {:?}",
                diagnostics.issues
            );
        }
        app.trouble = trouble.then(|| {
            (
                Topic::Bluetooth,
                "The device did not connect. Try again.".into(),
            )
        });
        app.restart_on_exit = !trouble;
        for page in 0..app.bluetooth_pages(&context).len() {
            app.bluetooth_page = page;
            let screen = app.bluetooth_for(&context);
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                !diagnostics.has_errors(),
                "{trouble} page {page}: {:?}",
                diagnostics.issues
            );
            touch(&screen, metrics, action_id(RESCAN));
        }
    }
}

#[test]
fn refreshed_lists_and_repeated_page_turns_clamp_without_wrapping() {
    let metrics = *interface_sizes().last().unwrap();
    let runner = AppRunner::with_metrics(Settings::default(), metrics);
    let mut context = runner.context();
    let mut app = fixture();
    app.view = View::Wifi;
    for _ in 0..20 {
        app.on_page_turn(&mut context, true);
    }
    assert_eq!(app.wifi_page, app.wifi_pages(&context).len() - 1);
    app.on_device_result(
        &mut context,
        DeviceRequest::ScanWifi,
        DeviceResult::Wifi {
            available: true,
            enabled: true,
            connected_ssid: None,
            networks: vec![WifiNetwork {
                ssid: "Only network".into(),
                signal_dbm: -50,
                secured: true,
                connected: false,
            }],
        },
    );
    assert_eq!(app.wifi_page, 0);
    for _ in 0..20 {
        app.on_page_turn(&mut context, false);
    }
    assert_eq!(app.wifi_page, 0);
    let screen = app.wifi_for(&context);
    touch(&screen, metrics, action_id(&network_action("Only network")));
    app.on_action(&mut context, action_id(&network_action("Only network")));
    assert_eq!(app.selected_ssid.as_deref(), Some("Only network"));
}

#[test]
fn a_rescan_reordering_rows_does_not_retarget_an_already_drawn_tap() {
    let _runner = AppRunner::new(Settings::default());
    let mut context = Context::default();
    let mut app = fixture();
    app.view = View::Wifi;
    let ssid = app.networks[1].ssid.clone();
    let selected = action_id(&network_action(&ssid));
    app.networks.reverse();
    app.on_action(&mut context, selected);
    assert_eq!(app.selected_ssid.as_deref(), Some(ssid.as_str()));
    app.on_action(&mut context, ActionId::BACK);
    app.networks.retain(|network| network.ssid != ssid);
    let _ = context.take_commands();
    app.on_action(&mut context, selected);
    assert_eq!(app.view, View::Wifi);
    assert!(!context
        .take_commands()
        .iter()
        .any(|command| matches!(command, Command::Device(DeviceRequest::JoinWifi { .. }))));
    app.view = View::Bluetooth;
    let address = app.devices[1].address.clone();
    let selected = action_id(&bluetooth_action(&address));
    app.devices.reverse();
    let _ = context.take_commands();
    app.on_action(&mut context, selected);
    assert!(context.take_commands().iter().any(|command| matches!(command, Command::Device(DeviceRequest::ConnectBluetooth { address: found }) if found == &address)));
    app.devices.retain(|device| device.address != address);
    app.on_action(&mut context, selected);
    assert!(context.take_commands().is_empty());
}
