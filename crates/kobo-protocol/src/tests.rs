use super::*;
use std::io::Cursor;

#[test]
fn a_shared_failure_code_never_blames_a_part_of_the_system_it_cannot_know_about() {
    // These seven codes answer for the radio, the Store, the app link and
    // the platform updater alike, so each sentence has to be true of all
    // of them. InvalidInput used to read "the address or credentials are
    // invalid", which is a sentence about a network: it was what a reader
    // showed when a platform update would not install, sending its owner
    // to their Wi-Fi settings for a download that had already succeeded,
    // and telling whoever they reported it to nothing at all.
    for failure in [
        DeviceError::NotFound,
        DeviceError::Authentication,
        DeviceError::TimedOut,
        DeviceError::Unreachable,
        DeviceError::InvalidInput,
        DeviceError::Backend,
        DeviceError::Integrity,
    ] {
        let described = failure.describe();
        assert!(
            !described.is_empty(),
            "{failure:?} has nothing to show an owner"
        );
        if !matches!(
            failure,
            DeviceError::Authentication | DeviceError::Unreachable | DeviceError::NotFound
        ) {
            assert!(
                !described.contains("credential")
                    && !described.contains("address")
                    && !described.contains("network"),
                "{failure:?} explains itself with a network that may not be involved: {described}"
            );
        }
    }
}

#[test]
fn every_glyph_has_a_wire_tag_and_gets_it_back() {
    // A glyph added to the enum without a tag here encodes as whatever the
    // match arm above it chose, so a book would arrive as a battery. The
    // encoder is exhaustive so that half is caught by the compiler; the
    // decoder is a numeric table and is not, which is the half this covers.
    for glyph in Glyph::ALL {
        let tag = encode_glyph(glyph);
        assert_eq!(
            decode_glyph(tag),
            Some(glyph),
            "{glyph:?} encoded as {tag} and came back as something else"
        );
    }
    let tags: Vec<u8> = Glyph::ALL.iter().copied().map(encode_glyph).collect();
    let mut unique = tags.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), tags.len(), "two glyphs share one wire tag");
    let highest = tags.iter().copied().max().expect("glyphs have tags");
    assert_eq!(
        decode_glyph(highest.checked_add(1).expect("glyph tag space remains")),
        None,
        "a tag one past the end decoded as a glyph"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one entry per wire variant; a shorter list is a variant that stopped being covered"
)]
fn every_device_request_round_trips() {
    let requests = vec![
        DeviceRequest::ReadBattery,
        DeviceRequest::HoldWifi { seconds: 600 },
        DeviceRequest::ReleaseWifi,
        DeviceRequest::KeepAwake { seconds: u32::MAX },
        DeviceRequest::AllowSleep,
        DeviceRequest::ScheduleWake { seconds: 900 },
        DeviceRequest::CancelWake,
        DeviceRequest::SetFrontlight { percent: 100 },
        DeviceRequest::SetFrontlight { percent: 0 },
        DeviceRequest::ReadFrontlight,
        DeviceRequest::ReadBluetooth,
        DeviceRequest::SetBluetooth { enabled: true },
        DeviceRequest::ScanBluetooth,
        DeviceRequest::PairBluetooth {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
        },
        DeviceRequest::ConnectBluetooth {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
        },
        DeviceRequest::DisconnectBluetooth {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
        },
        DeviceRequest::ForgetBluetooth {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
        },
        DeviceRequest::ReadWifi,
        DeviceRequest::SetWifi { enabled: false },
        DeviceRequest::ScanWifi,
        DeviceRequest::JoinWifi {
            ssid: "Library".to_owned(),
            password: "readmore".to_owned(),
        },
        DeviceRequest::DisconnectWifi,
        DeviceRequest::ReadAudio,
        DeviceRequest::LoadAudio {
            source: AudioSource::Shelf("audiobook.mp3z".to_owned()),
        },
        DeviceRequest::LoadAudio {
            source: AudioSource::Stream("https://example.com/book.mp3".to_owned()),
        },
        DeviceRequest::PlayAudio,
        DeviceRequest::PauseAudio,
        DeviceRequest::SeekAudio {
            position_ms: 120_000,
        },
        DeviceRequest::StopAudio,
        DeviceRequest::SetAudioVolume { percent: 65 },
        DeviceRequest::ReadBatteryDetail,
        DeviceRequest::ReadCover,
        DeviceRequest::Update {
            url: "https://github.com/o/r/releases/download/v0.1.1/KoboRoot.tgz".to_owned(),
            sha256: "a".repeat(64),
        },
        DeviceRequest::ListInstalledApps,
        DeviceRequest::ReadAppCatalog,
        DeviceRequest::RefreshAppCatalog,
        DeviceRequest::InstallApp {
            id: "word-count".to_owned(),
        },
        DeviceRequest::UninstallApp {
            id: "word-count".to_owned(),
        },
        DeviceRequest::ReadAppLink,
        DeviceRequest::BeginAppLink,
        DeviceRequest::PollAppLink,
        DeviceRequest::DisconnectAppLink,
        DeviceRequest::ReadIdentity,
        DeviceRequest::ReadAutoUpdate,
        DeviceRequest::SetAutoUpdate {
            cobalt: true,
            apps: false,
        },
        DeviceRequest::SetAutoUpdate {
            cobalt: false,
            apps: true,
        },
        DeviceRequest::ReadUpdateChannel,
        DeviceRequest::SetUpdateChannel {
            channel: UpdateChannel::Stable,
        },
        DeviceRequest::SetUpdateChannel {
            channel: UpdateChannel::Beta,
        },
        DeviceRequest::SetSecret {
            name: "lichess".to_owned(),
            value: SecretValue::new("token-value"),
        },
        DeviceRequest::ListLibrary,
        DeviceRequest::ReadLibrary {
            id: "0/Bleak-House.epub".to_owned(),
        },
    ];
    for request in requests {
        let frame = Frame {
            version: VERSION,
            request_id: 9,
            message: Message::DeviceRequest(request),
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }
}

#[test]
fn an_update_that_could_not_be_verified_is_refused_at_the_encoder() {
    let cases = [
        // Plaintext could be rewritten in flight, digest or no digest.
        ("http://github.com/a.tgz".to_owned(), "a".repeat(64)),
        // A digest of the wrong length matches nothing.
        ("https://github.com/a.tgz".to_owned(), "a".repeat(63)),
        // Uppercase is somebody's formatting, not this wire's.
        (
            "https://github.com/a.tgz".to_owned(),
            format!("{}F", "a".repeat(63)),
        ),
    ];
    for (url, sha256) in cases {
        let frame = Frame {
            version: VERSION,
            request_id: 9,
            message: Message::DeviceRequest(DeviceRequest::Update {
                url: url.clone(),
                sha256,
            }),
        };
        assert!(encode(&frame).is_err(), "{url} was encoded");
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one entry per wire variant; a shorter list is a variant that stopped being covered"
)]
fn every_device_result_round_trips() {
    let results = vec![
        DeviceResult::Done,
        DeviceResult::Granted { seconds: 300 },
        DeviceResult::Battery {
            percent: 100,
            charging: true,
        },
        DeviceResult::Battery {
            percent: 0,
            charging: false,
        },
        DeviceResult::Frontlight { percent: 42 },
        DeviceResult::Denied(DenyReason::NotDeclared),
        DeviceResult::Denied(DenyReason::WithheldForBattery),
        DeviceResult::Denied(DenyReason::Unsupported),
        DeviceResult::Denied(DenyReason::PolicyRejected),
        DeviceResult::Denied(DenyReason::Busy),
        DeviceResult::Bluetooth {
            available: true,
            enabled: true,
            devices: vec![BluetoothDevice {
                address: "AA:BB:CC:DD:EE:FF".to_owned(),
                name: "Headphones".to_owned(),
                kind: BluetoothDeviceKind::Audio,
                paired: true,
                connected: true,
            }],
            restart_on_exit: false,
        },
        DeviceResult::Bluetooth {
            available: true,
            enabled: true,
            devices: Vec::new(),
            restart_on_exit: true,
        },
        DeviceResult::Wifi {
            available: true,
            enabled: true,
            connected_ssid: Some("Library".to_owned()),
            networks: vec![WifiNetwork {
                ssid: "Library".to_owned(),
                signal_dbm: -48,
                secured: true,
                connected: true,
            }],
        },
        DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Playing,
            position_ms: 75_000,
            duration_ms: 600_000,
            volume: 70,
        },
        DeviceResult::Failed(DeviceError::Authentication),
        DeviceResult::Failed(DeviceError::Integrity),
        DeviceResult::BatteryDetail(BatteryDetail {
            percent: Some(28),
            status: Some("Discharging".to_owned()),
            health: Some("Good".to_owned()),
            technology: Some("Li-ion".to_owned()),
            decidegrees: Some(290),
            microvolts: Some(3_720_000),
            microamps: Some(-180_000),
            charge_now: Some(420_000),
            charge_full: Some(1_480_000),
            charge_full_design: Some(1_500_000),
        }),
        DeviceResult::BatteryDetail(BatteryDetail::default()),
        DeviceResult::Cover {
            available: true,
            magnet_present: true,
        },
        DeviceResult::Cover {
            available: false,
            magnet_present: false,
        },
        DeviceResult::Apps {
            entries: vec![AppInfo {
                id: "word-count".to_owned(),
                title: "Word Count".to_owned(),
                label: "Words".to_owned(),
                summary: "Counts words in a note.".to_owned(),
                version: "1.2.0".to_owned(),
                minimum_cobalt_version: "0.3.0".to_owned(),
                glyph: Glyph::Note,
                capabilities: vec!["shared-files".to_owned()],
                installed_version: Some("1.1.0".to_owned()),
            }],
        },
        DeviceResult::Library {
            entries: vec![LibraryEntry {
                id: "0/Bleak-House.epub".to_owned(),
                title: "Bleak House".to_owned(),
                kind: 1,
                bytes: 12_000,
                on_card: true,
            }],
            truncated: false,
        },
        DeviceResult::LibraryDocument {
            id: "0/notes.md".to_owned(),
            bytes: b"hello".to_vec(),
        },
    ];
    for result in results {
        let frame = Frame {
            version: VERSION,
            request_id: 11,
            message: Message::DeviceResult(result),
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }
}

/// A Kepub is the kind most Kobo owners have most of, and the encoder
/// used to refuse it. A refused entry is not a missing tile: encoding is
/// the whole listing, so one purchased book took the shelf, the launcher
/// and the session down with it.
#[test]
fn a_kepub_listing_survives_the_wire() {
    let frame = Frame {
        version: VERSION,
        request_id: 11,
        message: Message::DeviceResult(DeviceResult::Library {
            entries: vec![LibraryEntry {
                id: "n//mnt/onboard/Piranesi.kepub.epub".to_owned(),
                title: "Piranesi".to_owned(),
                kind: 6,
                bytes: 900_000,
                on_card: true,
            }],
            truncated: false,
        }),
    };
    let bytes = encode(&frame).expect("a Kepub entry the wire refuses takes the session down");
    assert_eq!(decode(&bytes).expect("decode"), frame);
}

#[test]
fn a_kepub_is_listed_and_not_offered_as_readable() {
    let kepub = LibraryEntry {
        id: "n//mnt/onboard/Piranesi.kepub.epub".to_owned(),
        title: "Piranesi".to_owned(),
        kind: 6,
        bytes: 900_000,
        on_card: true,
    };
    assert!(
        !kepub.is_readable(),
        "a Kepub on the card was offered to a reader that shows its markup"
    );
}

#[test]
fn every_auto_update_result_round_trips() {
    for cobalt in [false, true] {
        for apps in [false, true] {
            let frame = Frame {
                version: VERSION,
                request_id: 11,
                message: Message::DeviceResult(DeviceResult::AutoUpdate { cobalt, apps }),
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }
}

#[test]
fn protocol_12_auto_update_frames_keep_their_payload_shape() {
    let read = Frame {
        version: VERSION,
        request_id: 8,
        message: Message::DeviceRequest(DeviceRequest::ReadAutoUpdate),
    };
    let read_bytes = encode(&read).expect("encode read");
    assert_eq!(
        read_bytes,
        [b'K', b'O', b'B', b'O', VERSION, 7, 0, 0, 0, 1, 0, 0, 0, 8, 43,]
    );
    assert_eq!(decode(&read_bytes).expect("decode read"), read);

    let request = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceRequest(DeviceRequest::SetAutoUpdate {
            cobalt: true,
            apps: false,
        }),
    };
    let request_bytes = encode(&request).expect("encode request");
    assert_eq!(
        request_bytes,
        [b'K', b'O', b'B', b'O', VERSION, 7, 0, 0, 0, 2, 0, 0, 0, 9, 44, 1,]
    );
    assert_eq!(decode(&request_bytes).expect("decode request"), request);

    let result = Frame {
        version: VERSION,
        request_id: 11,
        message: Message::DeviceResult(DeviceResult::AutoUpdate {
            cobalt: false,
            apps: true,
        }),
    };
    let result_bytes = encode(&result).expect("encode result");
    assert_eq!(
        result_bytes,
        [b'K', b'O', b'B', b'O', VERSION, 8, 0, 0, 0, 2, 0, 0, 0, 11, 17, 2,]
    );
    assert_eq!(decode(&result_bytes).expect("decode result"), result);
}

#[test]
fn secret_presence_round_trips_and_stays_bounded() {
    let request = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceRequest(DeviceRequest::CheckSecrets {
            names: vec!["exa".to_owned(), "openai".to_owned()],
        }),
    };
    let bytes = encode(&request).expect("encode request");
    assert_eq!(decode(&bytes).expect("decode request"), request);
    let result = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceResult(DeviceResult::Secrets {
            present: vec!["openai".to_owned()],
        }),
    };
    let bytes = encode(&result).expect("encode result");
    assert_eq!(decode(&bytes).expect("decode result"), result);
    // An empty answer is meaningful: nothing asked about is installed.
    let none = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceResult(DeviceResult::Secrets { present: vec![] }),
    };
    let bytes = encode(&none).expect("encode empty result");
    assert_eq!(decode(&bytes).expect("decode empty result"), none);
    // An empty ask is not, and neither is one past the bound or a name
    // that could not be a reviewed secret.
    for message in [
        Message::DeviceRequest(DeviceRequest::CheckSecrets { names: vec![] }),
        Message::DeviceRequest(DeviceRequest::CheckSecrets {
            names: (0..=MAX_CHECKED_SECRETS)
                .map(|index| format!("key{index}"))
                .collect(),
        }),
        Message::DeviceRequest(DeviceRequest::CheckSecrets {
            names: vec!["not a name".to_owned()],
        }),
        Message::DeviceResult(DeviceResult::Secrets {
            present: vec!["../escape".to_owned()],
        }),
    ] {
        let frame = Frame {
            version: VERSION,
            request_id: 9,
            message,
        };
        assert!(encode(&frame).is_err());
    }
}

#[test]
fn update_channel_variants_round_trip() {
    for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
        for message in [
            Message::DeviceRequest(DeviceRequest::SetUpdateChannel { channel }),
            Message::DeviceResult(DeviceResult::UpdateChannel(channel)),
        ] {
            let frame = Frame {
                version: VERSION,
                request_id: 9,
                message,
            };
            let bytes = encode(&frame).expect("encode");
            assert_eq!(decode(&bytes).expect("decode"), frame);
        }
    }
    let read = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceRequest(DeviceRequest::ReadUpdateChannel),
    };
    let bytes = encode(&read).expect("encode");
    assert_eq!(decode(&bytes).expect("decode"), read);
}

#[test]
fn server_accounts_roundtrip_only_with_scope_aware_protocol() {
    let frame = Frame {
        version: SERVER_ACCOUNT_VERSION,
        request_id: 19,
        message: Message::DeviceRequest(DeviceRequest::SetServerSecret {
            name: "komga".into(),
            server: "https://books.example/library".into(),
            value: SecretValue::new("reader:private-value"),
        }),
    };
    let bytes = encode(&frame).unwrap();
    assert_eq!(decode(&bytes).unwrap(), frame);
    assert!(!format!("{:?}", frame.message).contains("private-value"));
    for version in 11..SERVER_ACCOUNT_VERSION {
        assert!(encode(&Frame {
            version,
            ..frame.clone()
        })
        .is_err());
        let mut legacy = bytes.clone();
        legacy[4] = version;
        assert!(decode(&legacy).is_err());
    }
    for server in [
        "http://books.example",
        "https://reader@books.example",
        "https://books.example?key=x",
        "https://books.example/#a",
    ] {
        let invalid = Frame {
            message: Message::DeviceRequest(DeviceRequest::SetServerSecret {
                name: "komga".into(),
                server: server.into(),
                value: SecretValue::new("reader:private-value"),
            }),
            ..frame.clone()
        };
        assert!(encode(&invalid).is_err(), "{server}");
    }
}

#[test]
fn application_secret_debug_output_is_redacted() {
    let request = DeviceRequest::SetSecret {
        name: "zotero".to_owned(),
        value: SecretValue::new("must-not-appear"),
    };
    let debug = format!("{request:?}");
    assert!(debug.contains("zotero"));
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("must-not-appear"));
}

#[test]
fn pre_folio_protocols_cannot_encode_or_decode_application_secret_requests() {
    // Protocol 10 is read and answered as protocol 11 is, through the
    // same pre-Folio gates, so every gate is held for both.
    for version in crate::PRE_FOLIO_VERSIONS {
        let request = Frame {
            version,
            request_id: 9,
            message: Message::DeviceRequest(DeviceRequest::SetSecret {
                name: "zotero".to_owned(),
                value: SecretValue::new("owner-value"),
            }),
        };
        assert_eq!(
            encode(&request),
            Err(ProtocolError::InvalidValue("application secret"))
        );

        let mut bytes = encode(&Frame {
            version: VERSION,
            ..request
        })
        .expect("protocol 12 secret request");
        bytes[4] = version;
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::InvalidValue("device request"))
        );
    }
}

#[test]
fn unknown_update_channels_are_refused() {
    let request = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceRequest(DeviceRequest::SetUpdateChannel {
            channel: UpdateChannel::Stable,
        }),
    };
    let mut bytes = encode(&request).expect("encode request");
    *bytes.last_mut().expect("channel byte") = 2;
    assert_eq!(
        decode(&bytes),
        Err(ProtocolError::InvalidValue("update channel"))
    );

    let result = Frame {
        version: VERSION,
        request_id: 9,
        message: Message::DeviceResult(DeviceResult::UpdateChannel(UpdateChannel::Stable)),
    };
    let mut bytes = encode(&result).expect("encode result");
    *bytes.last_mut().expect("channel byte") = 2;
    assert_eq!(
        decode(&bytes),
        Err(ProtocolError::InvalidValue("update channel"))
    );
}

#[test]
fn identity_results_round_trip() {
    let results = [
        DeviceResult::Identity(DeviceIdentity {
            profile_id: "CLARA_BW_391".to_owned(),
            model: "Kobo Clara BW".to_owned(),
            device_code: 391,
            firmware: "4.41.23145".to_owned(),
            kernel: "5.10.117".to_owned(),
            runtime_version: "0.1.0".to_owned(),
            panel_width: 1072,
            panel_height: 1448,
        }),
        DeviceResult::Identity(DeviceIdentity::default()),
    ];
    for result in results {
        let frame = Frame {
            version: VERSION,
            request_id: 11,
            message: Message::DeviceResult(result),
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }
}

#[test]
fn oversized_identity_field_is_refused() {
    let identity = DeviceIdentity {
        model: "x".repeat(MAX_IDENTITY_FIELD_LEN + 1),
        ..DeviceIdentity::default()
    };
    let frame = Frame {
        version: VERSION,
        request_id: 11,
        message: Message::DeviceResult(DeviceResult::Identity(identity)),
    };
    assert!(encode(&frame).is_err());
}

#[test]
fn every_app_link_result_round_trips() {
    let results = [
        DeviceResult::AppLink(AppLinkState::Unpaired),
        DeviceResult::AppLink(AppLinkState::Pairing {
            code: "2345ABCD".to_owned(),
            url: "https://store.example/link".to_owned(),
            expires_in: 600,
        }),
        DeviceResult::AppLink(AppLinkState::Paired { browsers: 8 }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::None),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::Installed {
            id: "word-count".to_owned(),
        }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::Updated {
            id: "word-count".to_owned(),
        }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::AlreadyInstalled {
            id: "word-count".to_owned(),
        }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::Included {
            id: "word-count".to_owned(),
        }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::Unavailable {
            id: "word-count".to_owned(),
        }),
        DeviceResult::RemoteInstall(RemoteInstallOutcome::RequiresCobalt {
            id: "word-count".to_owned(),
            minimum_cobalt_version: "0.4.0".to_owned(),
        }),
    ];
    for result in results {
        let frame = Frame {
            version: VERSION,
            request_id: 11,
            message: Message::DeviceResult(result),
        };
        let bytes = encode(&frame).expect("encode");
        assert_eq!(decode(&bytes).expect("decode"), frame);
    }
}

#[test]
fn app_requests_and_results_are_bounded_and_validated() {
    let ids = vec![
        String::new(),
        "../todo".to_owned(),
        "Todo".to_owned(),
        "a/b".to_owned(),
        "1todo".to_owned(),
        "-todo".to_owned(),
        "todo-".to_owned(),
        "todo--list".to_owned(),
        "a".repeat(MAX_APP_ID_LEN + 1),
    ];
    for id in ids {
        let frame = Frame {
            version: VERSION,
            request_id: 1,
            message: Message::DeviceRequest(DeviceRequest::InstallApp { id: id.clone() }),
        };
        assert!(encode(&frame).is_err(), "{id:?} was accepted as an app id");
    }
    for id in ["a", "todo", "todo-2", "a1-b2"] {
        assert!(valid_app_id(id), "{id:?} was rejected as an app id");
    }

    let too_many = DeviceResult::Apps {
        entries: (0..=MAX_APP_CATALOG_ENTRIES)
            .map(|index| AppInfo {
                id: format!("app-{index}"),
                title: "App".to_owned(),
                label: "App".to_owned(),
                summary: "One application.".to_owned(),
                version: "1.0.0".to_owned(),
                minimum_cobalt_version: "0.3.0".to_owned(),
                glyph: Glyph::App,
                capabilities: Vec::new(),
                installed_version: None,
            })
            .collect(),
    };
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(too_many),
    })
    .is_err());

    let app = |version: String| AppInfo {
        id: "version-test".to_owned(),
        title: "Version Test".to_owned(),
        label: "Version".to_owned(),
        summary: "Checks the application version wire bound.".to_owned(),
        version,
        minimum_cobalt_version: "0.3.0".to_owned(),
        glyph: Glyph::App,
        capabilities: Vec::new(),
        installed_version: None,
    };
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::Apps {
            entries: vec![app("a".repeat(MAX_APP_VERSION_LEN))],
        }),
    })
    .is_ok());
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::Apps {
            entries: vec![app("a".repeat(MAX_APP_VERSION_LEN + 1))],
        }),
    })
    .is_err());
}

#[test]
fn app_compatibility_uses_numeric_cobalt_versions() {
    let app = AppInfo {
        id: "version-test".to_owned(),
        title: "Version Test".to_owned(),
        label: "Version".to_owned(),
        summary: "Checks minimum Cobalt versions.".to_owned(),
        version: "1.0.0".to_owned(),
        minimum_cobalt_version: "0.2.4".to_owned(),
        glyph: Glyph::App,
        capabilities: Vec::new(),
        installed_version: None,
    };
    assert!(app.is_compatible_with("0.3.0"));
    assert!(app.is_compatible_with("0.3"));
    assert!(!app.is_compatible_with("0.2.3"));
    assert!(!app.is_compatible_with("nightly"));

    let newer = AppInfo {
        minimum_cobalt_version: "0.4.0".to_owned(),
        ..app
    };
    assert!(!newer.is_compatible_with("0.3.0"));
}

#[test]
fn app_updates_are_only_strictly_newer_numeric_versions() {
    let app = |published: &str, installed: &str| AppInfo {
        id: "version-test".to_owned(),
        title: "Version Test".to_owned(),
        label: "Version".to_owned(),
        summary: "Checks Store update ordering.".to_owned(),
        version: published.to_owned(),
        minimum_cobalt_version: "0.3.2".to_owned(),
        glyph: Glyph::App,
        capabilities: Vec::new(),
        installed_version: Some(installed.to_owned()),
    };
    assert!(app("1.2.0", "1.1.9").has_update());
    assert!(!app("1.2.0", "1.2.0").has_update());
    assert!(!app("1.1.9", "1.2.0").has_update());
    assert!(!app("stable", "1.2.0").has_update());
    assert!(!app("1.2.0", "beta").has_update());
}

#[test]
fn malformed_device_payloads_are_rejected_without_panic() {
    let template = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceRequest(DeviceRequest::ReadBattery),
    })
    .expect("encode");

    // An unknown request tag.
    let mut unknown = template.clone();
    unknown[HEADER_LEN] = 200;
    assert_eq!(
        decode(&unknown),
        Err(ProtocolError::InvalidValue("device request"))
    );

    // A percentage that cannot exist.
    let mut absurd = template.clone();
    absurd[HEADER_LEN] = 8;
    absurd[HEADER_LEN + 4] = 250;
    assert_eq!(
        decode(&absurd),
        Err(ProtocolError::InvalidValue("frontlight percent"))
    );

    // A truncated payload must not be read past its end.
    let mut truncated = template.clone();
    truncated.truncate(HEADER_LEN + 3);
    assert_eq!(decode(&truncated), Err(ProtocolError::LengthMismatch));

    let result = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::Denied(DenyReason::Busy)),
    })
    .expect("encode");
    let mut bad_reason = result;
    let last = bad_reason.len() - 1;
    bad_reason[last] = 99;
    assert_eq!(
        decode(&bad_reason),
        Err(ProtocolError::InvalidValue("deny reason"))
    );
}

#[test]
fn app_link_and_remote_install_payloads_are_bounded_and_validated() {
    let pairing = |code: String, url: String, expires_in| Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Pairing {
            code,
            url,
            expires_in,
        })),
    };
    for code in [
        "2345ABC".to_owned(),
        "2345ABCDE".to_owned(),
        "2345ABCI".to_owned(),
        "2345abcD".to_owned(),
    ] {
        assert!(encode(&pairing(code, "https://store.example/link".to_owned(), 60)).is_err());
    }
    assert!(encode(&pairing(
        "2345ABCD".to_owned(),
        "http://store.example/link".to_owned(),
        60
    ))
    .is_err());
    assert!(encode(&pairing(
        "2345ABCD".to_owned(),
        format!("https://{}", "a".repeat(MAX_URL_LEN)),
        60
    ))
    .is_err());
    assert!(encode(&pairing(
        "2345ABCD".to_owned(),
        "https://store.example/link".to_owned(),
        601
    ))
    .is_err());
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Paired { browsers: 9 })),
    })
    .is_err());
    assert!(encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::RemoteInstall(
            RemoteInstallOutcome::Installed {
                id: "../store".to_owned(),
            },
        )),
    })
    .is_err());
}

#[test]
fn malformed_app_link_payloads_are_rejected() {
    let pairing = |code: String, url: String, expires_in| Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Pairing {
            code,
            url,
            expires_in,
        })),
    };
    let valid_pairing = encode(&pairing(
        "2345ABCD".to_owned(),
        "https://store.example/link".to_owned(),
        600,
    ))
    .expect("valid pairing");
    let mut bad_code = valid_pairing.clone();
    bad_code[HEADER_LEN + 6] = b'I';
    assert_eq!(
        decode(&bad_code),
        Err(ProtocolError::InvalidValue("application link"))
    );
    let mut bad_expiry = valid_pairing;
    let last = bad_expiry.len() - 1;
    bad_expiry[last] = 0x59;
    assert_eq!(
        decode(&bad_expiry),
        Err(ProtocolError::InvalidValue("application link"))
    );

    let mut bad_url = encode(&pairing(
        "2345ABCD".to_owned(),
        "https://store.example/link".to_owned(),
        600,
    ))
    .expect("valid pairing");
    bad_url[HEADER_LEN + 18] = b'x';
    assert_eq!(
        decode(&bad_url),
        Err(ProtocolError::InvalidValue("application link"))
    );

    let mut bad_browsers = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::AppLink(AppLinkState::Paired { browsers: 8 })),
    })
    .expect("valid paired state");
    let last = bad_browsers.len() - 1;
    bad_browsers[last] = 9;
    assert_eq!(
        decode(&bad_browsers),
        Err(ProtocolError::InvalidValue("application link"))
    );

    let mut bad_id = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::RemoteInstall(
            RemoteInstallOutcome::Installed {
                id: "word-count".to_owned(),
            },
        )),
    })
    .expect("valid outcome");
    bad_id[HEADER_LEN + 6] = b'W';
    assert_eq!(
        decode(&bad_id),
        Err(ProtocolError::InvalidValue("application id"))
    );

    let mut bad_outcome = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::DeviceResult(DeviceResult::RemoteInstall(RemoteInstallOutcome::None)),
    })
    .expect("valid empty outcome");
    let last = bad_outcome.len() - 1;
    bad_outcome[last] = 8;
    assert_eq!(
        decode(&bad_outcome),
        Err(ProtocolError::InvalidValue("remote install outcome"))
    );
}

#[test]
fn screen_round_trip_is_deterministic() {
    let frame = Frame {
        version: VERSION,
        request_id: 12,
        message: Message::SetScreen(Screen::new(
            7,
            vec![Node::Card {
                id: NodeId(1),
                children: vec![Node::Button {
                    id: NodeId(2),
                    action: ActionId(3),
                    label: "Go".into(),
                    state: ControlState::Enabled,
                    emphasis: kobo_ui::Emphasis::Normal,
                }],
            }],
        )),
    };
    let encoded = encode(&frame).expect("valid screen");
    assert_eq!(encoded, encode(&frame).expect("stable encoding"));
    assert_eq!(decode(&encoded), Ok(frame));
}

#[test]
fn a_reading_screens_middle_column_survives_the_wire() {
    // The length calculation is separate from the encoder, so a field
    // added to one and not the other produces a frame that encodes and
    // then refuses to decode. Both shapes are round-tripped for that
    // reason rather than only the new one.
    let plain = Screen::new(1, Vec::new()).with_page_turns(ActionId(11), ActionId(12));
    let with_menu = {
        let mut screen = plain.clone();
        screen.page_turns = screen
            .page_turns
            .map(|turns| turns.with_menu(ActionId(13)).with_position(4, 12));
        screen
    };
    for screen in [plain, with_menu] {
        let expected = screen.page_turns;
        let frame = Frame {
            version: VERSION,
            request_id: 4,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encodes");
        let back = decode(&bytes).expect("decodes");
        let Message::SetScreen(out) = back.message else {
            panic!("wrong message");
        };
        assert_eq!(out.page_turns, expected);
    }
}

#[test]
fn a_held_finger_survives_the_wire() {
    // Same trap as the middle column: encoded_len is computed apart from
    // the encoder, so both shapes are round-tripped rather than only the
    // one carrying the new field.
    let plain = Screen::new(1, Vec::new());
    let holding = Screen::new(1, Vec::new()).with_hold(ActionId(21));
    for screen in [plain, holding] {
        let expected = screen.hold;
        let frame = Frame {
            version: VERSION,
            request_id: 4,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("encodes");
        let back = decode(&bytes).expect("decodes");
        let Message::SetScreen(out) = back.message else {
            panic!("wrong message");
        };
        assert_eq!(out.hold, expected);
    }
}

#[test]
fn a_screen_can_ask_for_a_text_size_and_most_do_not() {
    let screen = Screen::new(1, Vec::new());
    assert!(
        screen.text_scale.is_none(),
        "inheriting the reader's own setting is the default"
    );
    for scale in [None, Some(TextScale::Large), Some(TextScale::ExtraLarge)] {
        let frame = Frame {
            version: VERSION,
            request_id: 9,
            message: Message::SetScreen(screen.clone().with_text_scale(scale)),
        };
        let bytes = encode(&frame).expect("encodes");
        let back = decode(&bytes).expect("decodes");
        let Message::SetScreen(out) = back.message else {
            panic!("wrong message");
        };
        assert_eq!(out.text_scale, scale);
    }
}

#[test]
fn a_request_for_first_refusal_on_back_survives_the_wire() {
    // A screen that asked to answer Back itself has to arrive that way,
    // because the runtime decides where the reader's tap goes from this
    // flag alone. Lost in transit it would silently mean the opposite.
    let screen = Screen::new(
        7,
        vec![Node::Text {
            id: NodeId(1),
            text: "Chapter one".into(),
            links: Vec::new(),
        }],
    );
    assert!(!screen.owns_back, "not asking for it is the default");
    for owns_back in [false, true] {
        let frame = Frame {
            version: VERSION,
            request_id: 12,
            message: Message::SetScreen(screen.clone().with_own_back(owns_back)),
        };
        let encoded = encode(&frame).expect("valid screen");
        assert_eq!(decode(&encoded), Ok(frame));
    }
}

#[test]
fn malformed_frames_are_rejected_before_allocation() {
    assert_eq!(decode(b"short"), Err(ProtocolError::Truncated));
    let mut header = [0_u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    header[4] = VERSION;
    header[5] = 6;
    header[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(decode(&header), Err(ProtocolError::FrameTooLarge));
}

#[test]
fn pre_folio_screen_frames_keep_the_legacy_renderer() {
    // Protocol 10 is read and answered as protocol 11 is, through the
    // same pre-Folio gates, so every gate is held for both.
    for version in crate::PRE_FOLIO_VERSIONS {
        // Heading and body are unchanged v11 payloads, representative of an
        // already-installed app. The peer version must select the legacy
        // renderer rather than refuse an OTA user's application.
        let nodes = vec![
            Node::Heading {
                id: NodeId(1),
                text: "Existing app".into(),
                level: 1,
            },
            Node::Text {
                id: NodeId(2),
                text: "Its local Atkinson measurements still apply.".into(),
                links: Vec::new(),
            },
        ];
        let screen = Screen::new(9, nodes.clone());
        let frame = Frame {
            version,
            request_id: 3,
            message: Message::SetScreen(screen),
        };
        let bytes = encode(&frame).expect("v11 encoding");
        let back = decode(&bytes).expect("v11 accepted");
        assert_eq!(back.version, version);
        let Message::SetScreen(decoded) = back.message else {
            panic!("expected screen");
        };
        assert_eq!(decoded.typography, kobo_ui::Typography::Paperback);
        assert_eq!(decoded.nodes, nodes);
        assert!(decoded
            .layout()
            .nodes
            .iter()
            .any(|node| matches!(node.kind, kobo_ui::LayoutKind::Heading(1))));
    }
}

#[test]
fn pre_folio_sections_and_tiles_use_the_legacy_wire_layout() {
    // Protocol 10 is read and answered as protocol 11 is, through the
    // same pre-Folio gates, so every gate is held for both.
    for version in crate::PRE_FOLIO_VERSIONS {
        let nodes = vec![
            Node::Section {
                id: NodeId(1),
                title: "Library".into(),
                value: Some("12".into()),
                link: None,
            },
            Node::TileGrid {
                id: NodeId(2),
                shape: TileShape::Portrait,
                tiles: vec![Tile::new(ActionId(5), "Book", Glyph::Book)],
            },
        ];
        let frame = Frame {
            version,
            request_id: 7,
            message: Message::SetScreen(Screen::new(10, nodes.clone())),
        };
        let Message::SetScreen(decoded) = decode(&encode(&frame).expect("v11 encoding"))
            .expect("v11 decoding")
            .message
        else {
            panic!("expected screen");
        };
        assert_eq!(decoded.nodes, nodes);
    }
}

#[test]
fn pre_folio_encoders_reject_folio_only_fields() {
    // Protocol 10 is read and answered as protocol 11 is, through the
    // same pre-Folio gates, so every gate is held for both.
    for version in crate::PRE_FOLIO_VERSIONS {
        let screens = [
            Screen::new(
                1,
                vec![Node::Section {
                    id: NodeId(1),
                    title: "Library".into(),
                    value: None,
                    link: Some(BarAction::new(ActionId(2), "View all")),
                }],
            ),
            Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    shape: TileShape::Square,
                    tiles: vec![Tile::new(ActionId(2), "Updates", Glyph::Download)
                        .with_value("3")
                        .with_menu(ActionId(3))],
                }],
            ),
            Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    shape: TileShape::Card,
                    tiles: Vec::new(),
                }],
            ),
            Screen::new(
                1,
                vec![Node::PageRail {
                    id: NodeId(1),
                    page: 0,
                    of: 2,
                }],
            ),
        ];
        for screen in screens {
            assert!(matches!(
                encode(&Frame {
                    version,
                    request_id: 8,
                    message: Message::SetScreen(screen),
                }),
                Err(ProtocolError::InvalidValue(_))
            ));
        }
    }
}

#[test]
fn version_12_folio_fields_round_trip() {
    let screen = Screen::new(
        10,
        vec![
            Node::Section {
                id: NodeId(1),
                title: "Featured".into(),
                value: None,
                link: Some(BarAction::new(ActionId(4), "View all ↗")),
            },
            Node::TileGrid {
                id: NodeId(2),
                shape: TileShape::Card,
                tiles: vec![Tile::new(ActionId(5), "App Store", Glyph::Download)
                    .with_caption("Updates")
                    .with_value("3")
                    .with_menu(ActionId(6))],
            },
            Node::PageRail {
                id: NodeId(3),
                page: 1,
                of: 3,
            },
        ],
    );
    let frame = Frame {
        version: VERSION,
        request_id: 4,
        message: Message::SetScreen(screen.clone()),
    };
    let Message::SetScreen(decoded) = decode(&encode(&frame).expect("v12 encoding"))
        .expect("v12 decoding")
        .message
    else {
        panic!("expected screen");
    };
    assert_eq!(decoded, screen);
    assert_eq!(decoded.typography, kobo_ui::Typography::Quiet);
}

#[test]
fn every_protocol_a_published_app_was_built_for_is_still_accepted() {
    // The release registry lists every protocol the Store has published
    // applications at. An application keeps its protocol for as long as
    // it is installed, so a platform update that refused one of these
    // would leave that application unable to open. Removing a version
    // from the runtime now means removing it from the registry first.
    let registry = include_str!("../../../tools/protocol-minimums.json");
    let start = registry.find("\"protocols\"").expect("a protocols map");
    let body = &registry[start..];
    let end = body.find('}').expect("the map closes");
    let published: Vec<u8> = body[..end]
        .split('"')
        .filter_map(|part| part.parse::<u8>().ok())
        .collect();
    assert!(
        !published.is_empty(),
        "no protocols were read from the registry"
    );
    for version in published {
        assert!(
            accepted_version(version),
            "protocol {version} is in the release registry but the runtime refuses it"
        );
    }
}

#[test]
fn versions_outside_the_window_are_refused() {
    let frame = Frame {
        version: VERSION,
        request_id: 3,
        message: Message::Exit,
    };
    let mut bytes = encode(&frame).expect("encoding");
    bytes[4] = DICTIONARY_VERSION - 1;
    assert_eq!(
        decode(&bytes),
        Err(ProtocolError::UnsupportedVersion(DICTIONARY_VERSION - 1))
    );
    bytes[4] = VERSION + 1;
    assert_eq!(
        decode(&bytes),
        Err(ProtocolError::UnsupportedVersion(VERSION + 1))
    );
}

#[test]
fn invalid_utf8_is_rejected() {
    let mut bytes = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::Hello { name: "x".into() },
    })
    .expect("valid hello");
    *bytes.last_mut().expect("payload") = 0xff;
    assert_eq!(decode(&bytes), Err(ProtocolError::InvalidUtf8));
}

#[test]
fn stream_round_trip_reads_exactly_one_frame() {
    let frame = Frame {
        version: VERSION,
        request_id: 42,
        message: Message::Hello {
            name: "counter".into(),
        },
    };
    let mut bytes = Vec::new();
    write_to(&mut bytes, &frame).expect("write frame");
    bytes.extend_from_slice(b"remaining");
    let mut cursor = Cursor::new(bytes);
    assert_eq!(read_from(&mut cursor).expect("read frame"), frame);
    assert_eq!(
        usize::try_from(cursor.position()).expect("fixture position fits"),
        encode(&frame).unwrap().len()
    );
}

#[test]
fn stream_rejects_oversized_length_before_allocation() {
    let mut header = [0_u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAGIC);
    header[4] = VERSION;
    header[5] = 1;
    header[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(
        read_from(&mut Cursor::new(header)),
        Err(StreamError::Protocol(ProtocolError::FrameTooLarge))
    );
}

#[test]
fn encoder_rejects_node_and_list_counts_decoder_would_reject() {
    let nodes = (0..=MAX_NODES)
        .map(|id| Node::Divider {
            id: NodeId(u32::try_from(id).expect("fixture ID")),
        })
        .collect();
    assert_eq!(
        encode(&Frame {
            version: VERSION,
            request_id: 1,
            message: Message::SetScreen(Screen::new(1, nodes)),
        }),
        Err(ProtocolError::TooManyNodes)
    );

    assert_eq!(
        encode(&Frame {
            version: VERSION,
            request_id: 2,
            message: Message::SetScreen(Screen::new(
                2,
                vec![Node::PagedList {
                    id: NodeId(1),
                    page: 0,
                    items: vec![String::new(); MAX_NODES + 1],
                }],
            )),
        }),
        Err(ProtocolError::TooManyNodes)
    );
}

#[test]
fn encoder_preflights_payload_limit() {
    // The full node budget with the longest string each node may carry:
    // 512 × 16 KiB is the frame limit exactly, and the envelope around
    // the strings takes it over.
    let nodes = (0..512)
        .map(|id| Node::Text {
            id: NodeId(id),
            text: "x".repeat(MAX_STRING_LEN),
            links: Vec::new(),
        })
        .collect();
    assert_eq!(
        encode(&Frame {
            version: VERSION,
            request_id: 3,
            message: Message::SetScreen(Screen::new(3, nodes)),
        }),
        Err(ProtocolError::FrameTooLarge)
    );
}

fn quoted_command() -> Screen {
    Screen::new(
        5,
        vec![Node::Quote {
            id: kobo_ui::NodeId(1),
            depth: 0,
            role: kobo_ui::QuoteRole::Command,
            text: "kobo connect hn".to_owned(),
            fold: None,
        }],
    )
}

fn decoded_at(version: u8, screen: Screen) -> Screen {
    let frame = Frame {
        version,
        request_id: 1,
        message: Message::SetScreen(screen),
    };
    let Message::SetScreen(decoded) = decode(&encode(&frame).expect("encodes"))
        .expect("decodes")
        .message
    else {
        panic!("expected screen");
    };
    decoded
}

#[test]
fn a_command_travels_as_a_command_at_protocol_seventeen() {
    let decoded = decoded_at(QUIET_TYPE_VERSION, quoted_command());
    assert_eq!(decoded, quoted_command());
}

/// A runtime without the command role reads an unknown role as prose, so a
/// command written at an earlier protocol is the quotation that runtime always
/// drew.
#[test]
fn a_command_written_at_protocol_sixteen_is_read_as_a_quotation() {
    let decoded = decoded_at(QUIET_TYPE_VERSION - 1, quoted_command());
    let [Node::Quote { role, text, .. }] = decoded.nodes.as_slice() else {
        panic!("expected one quote");
    };
    assert_eq!(*role, kobo_ui::QuoteRole::Body);
    assert_eq!(text, "kobo connect hn");
}

/// An application at protocol 16 fitted its text to Folio's sizes and gaps,
/// so its screens are laid out in them; one at 17 fitted them to the quiet
/// ones.
#[test]
fn each_protocol_is_laid_out_in_the_type_it_measured_with() {
    let screen = Screen::new(5, Vec::new());
    for (version, typography) in [
        (LEGACY_VERSION, kobo_ui::Typography::Paperback),
        (FOLIO_VERSION, kobo_ui::Typography::Folio),
        (QUIET_TYPE_VERSION - 1, kobo_ui::Typography::Folio),
        (QUIET_TYPE_VERSION, kobo_ui::Typography::Quiet),
    ] {
        assert_eq!(
            decoded_at(version, screen.clone()).typography,
            typography,
            "protocol {version}"
        );
    }
}
