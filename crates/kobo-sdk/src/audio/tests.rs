use super::{clock, span, speed_label, AudioMetadata, AudioPlayer, Sleep};
use crate::{
    action_id, AudioChapter, AudioDetail, AudioPlaybackState, AudioSource, BluetoothDevice,
    BluetoothDeviceKind, Command, DeviceRequest, DeviceResult, PictureHandle, TaskOutcome,
    TilePicture, CLARA_BW_METRICS,
};
use std::time::{Duration, Instant};

fn headphones(connected: bool) -> BluetoothDevice {
    BluetoothDevice {
        address: "AA:BB:CC:DD:EE:FF".to_owned(),
        name: "Headphones".to_owned(),
        kind: BluetoothDeviceKind::Audio,
        paired: connected,
        connected,
    }
}

fn chapters(count: usize, each_ms: u32) -> Vec<AudioChapter> {
    (0..count)
        .map(|index| AudioChapter {
            start_ms: u32::try_from(index).unwrap() * each_ms,
            title: format!("Chapter {} of the night", index + 1),
        })
        .collect()
}

/// A player whose book has loaded, with chapters, at a known place.
fn loaded(chapter_count: usize) -> AudioPlayer {
    let mut player = AudioPlayer::library("0/Audiobooks/night.m4b", "A History of the Moon")
        .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"));
    player.loaded = super::LoadState::Loaded;
    player.playback = AudioPlaybackState::Paused;
    player.duration_ms = 7_200_000;
    player.position_ms = 3_675_000;
    player.detail = Some(AudioDetail {
        title: "A History of the Moon".to_owned(),
        author: "Ada Example".to_owned(),
        narrator: "Ada Example".to_owned(),
        album: String::new(),
        format: "AAC 64 kbps, 22 kHz mono".to_owned(),
        duration_ms: 7_200_000,
        chapters: chapters(chapter_count, 600_000),
        cover: Vec::new(),
    });
    player.bluetooth_enabled = super::Switch::On;
    player.bluetooth_known = true;
    player.devices = vec![headphones(true)];
    player
}

fn sent(context: &mut crate::Context) -> Vec<DeviceRequest> {
    context
        .take_commands()
        .into_iter()
        .filter_map(|command| match command {
            Command::Device(request) => Some(request),
            _ => None,
        })
        .collect()
}

#[test]
fn an_author_is_stated_once_not_twice() {
    // It is the hero's byline. A facts row repeating it word for word two
    // lines below reads as a rendering bug, and did on the reader.
    let player = AudioPlayer::shelf("book.mp3z", "A History of the Moon")
        .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"));
    let drawn = format!("{:?}", player.screen());
    assert_eq!(drawn.matches("Cobalt Audio").count(), 1, "{drawn}");
    assert!(!drawn.contains("Author"), "{drawn}");
}

#[test]
fn the_books_own_byline_names_the_narrator_only_when_it_is_somebody_else() {
    let mut player = loaded(3);
    player.metadata.author = None;
    assert_eq!(player.byline().as_deref(), Some("Ada Example"));
    if let Some(detail) = player.detail.as_mut() {
        detail.narrator = "Ben Reader".to_owned();
    }
    assert_eq!(
        player.byline().as_deref(),
        Some("Ada Example, read by Ben Reader")
    );
    if let Some(detail) = player.detail.as_mut() {
        detail.author.clear();
    }
    assert_eq!(player.byline().as_deref(), Some("Read by Ben Reader"));
}

#[test]
fn a_player_reached_from_a_shelf_keeps_the_way_back() {
    let alone = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
    assert!(!alone.screen().owns_back);
    let from_a_shelf = AudioPlayer::shelf("book.mp3z", "A History of the Moon").owns_back(true);
    assert!(from_a_shelf.screen().owns_back);
}

#[test]
fn a_volume_is_stated_once_not_on_both_buttons() {
    // Both volume buttons used to print the current level, so a reader saw
    // "Volume -  40%" beside "Volume +  40%" and had to work out that the
    // two numbers were one number. Same fault as the doubled byline above.
    let mut player = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
    player.volume = 40;
    let drawn = format!("{:?}", player.screen());
    assert_eq!(drawn.matches("40%").count(), 1, "{drawn}");
}

#[test]
fn the_play_control_shows_the_picture_of_what_it_will_do() {
    // Stopped, the button plays, so it draws a triangle. Playing, it
    // pauses, so it draws two bars. A control whose icon describes the
    // current state rather than the pending action is the classic way to
    // get this backwards.
    let stopped = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
    let drawn = format!("{:?}", stopped.screen());
    assert!(drawn.contains("Play"), "{drawn}");
    assert!(!drawn.contains("Pause"), "{drawn}");

    let mut playing = AudioPlayer::shelf("book.mp3z", "A History of the Moon");
    playing.playback = AudioPlaybackState::Playing;
    let drawn = format!("{:?}", playing.screen());
    assert!(drawn.contains("Pause"), "{drawn}");
}

#[test]
fn clocks_spans_and_speeds_read_as_people_say_them() {
    assert_eq!(clock(75_000), "1:15");
    assert_eq!(clock(3_675_000), "1:01:15");
    assert_eq!(span(45_000), "45 s");
    assert_eq!(span(38 * 60_000), "38 min");
    assert_eq!(span(7 * 3_600_000 + 38 * 60_000), "7 h 38 min");
    assert_eq!(span(2 * 3_600_000 + 5 * 60_000), "2 h 05 min");
    assert_eq!(speed_label(100), "1.0×");
    assert_eq!(speed_label(125), "1.25×");
    assert_eq!(speed_label(75), "0.75×");
    assert_eq!(speed_label(150), "1.5×");
    assert_eq!(speed_label(300), "3.0×");
}

#[test]
fn the_chapter_fact_says_where_in_the_book_this_is() {
    let player = loaded(12);
    // 3,675 s into chapters of 600 s is the seventh chapter.
    assert_eq!(
        player.chapter_fact().as_deref(),
        Some("7 of 12 · Chapter 7 of the night")
    );
    let drawn = format!("{:?}", player.screen());
    assert!(drawn.contains("7 of 12"), "{drawn}");
}

#[test]
fn player_and_pairing_screens_fit_a_clara() {
    let mut player = AudioPlayer::shelf("book.mp3z", "A History of the Moon")
        .metadata(AudioMetadata::new("A History of the Moon").author("Cobalt Audio"))
        .cover(TilePicture::new(PictureHandle(1), 240, 320))
        .secondary_action("another", "Create another", crate::Glyph::Plus);
    player.playback = AudioPlaybackState::Playing;
    player.position_ms = 75_000;
    player.duration_ms = 600_000;
    player.bluetooth_enabled = super::Switch::On;
    player.devices = vec![headphones(true)];
    assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());

    player.view = super::View::Bluetooth;
    assert!(player.screen().validate(&CLARA_BW_METRICS).is_empty());
}

/// An application that shows the player, so the player is measured
/// through a runner's context, at the size and against the type the
/// runtime will draw it with.
struct Shows;

impl crate::KoboApp for Shows {
    fn on_start(&mut self, _context: &mut crate::Context) {}
    fn on_action(&mut self, _context: &mut crate::Context, _action: crate::ActionId) {}
}

/// Every state the player is drawn in, at its longest: a title of two
/// lines, a byline, a chapter, a sleep timer, a connected output, a trouble
/// banner, and the secondary action beside the output picker.
fn every_player() -> Vec<(&'static str, AudioPlayer)> {
    let full = || {
        let mut player = loaded(12)
            .metadata(
                AudioMetadata::new("A History of the Moon and the People Who Learned to Read It")
                    .author("Made on this reader"),
            )
            .cover(TilePicture::new(PictureHandle(1), 240, 320))
            .secondary_action("another", "Create another", crate::Glyph::Plus);
        player.sleep = Sleep::Minutes(30);
        player.sleep_until = Some(Instant::now() + Duration::from_secs(23 * 60));
        player.speed_percent = 125;
        player
    };
    let playing = {
        let mut player = full();
        player.playback = AudioPlaybackState::Playing;
        player
    };
    let unavailable = {
        let mut player = full();
        player.audio_available = super::Availability::Unavailable;
        player.devices.clear();
        player
    };
    let trouble = {
        let mut player = full();
        player.trouble = Some("The Bluetooth audio service stopped responding.".to_owned());
        player
    };
    let finished = {
        let mut player = full();
        player.playback = AudioPlaybackState::Finished;
        player.position_ms = player.duration_ms;
        player
    };
    let alone = AudioPlayer::shelf("book.mp3z", "The Quiet Shelf").cover(TilePicture::new(
        PictureHandle(1),
        240,
        320,
    ));
    vec![
        ("playing", playing),
        ("unavailable", unavailable),
        ("trouble", trouble),
        ("finished", finished),
        ("without a secondary action", alone),
    ]
}

#[test]
fn the_player_keeps_every_control_on_a_clara_at_every_text_size_held_either_way() {
    use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, Orientation, TextScale};
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let context = crate::AppRunner::with_metrics(Shows, metrics).context();
            for (state, player) in every_player() {
                let at = format!("{state}, {text_scale:?}, {orientation:?}");
                let screen = player.screen_for(&context);
                let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                let errors = diagnostics
                    .issues
                    .iter()
                    .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                    .collect::<Vec<_>>();
                assert!(errors.is_empty(), "{at}: {errors:?}");
                let mut wanted = vec![super::OUTPUT];
                if player.audio_available == super::Availability::Available {
                    wanted.extend([
                        super::PLAY,
                        super::BACK_THIRTY,
                        super::FORWARD_THIRTY,
                        super::VOLUME_DOWN,
                        super::VOLUME_UP,
                        super::CHAPTERS,
                        super::SPEED,
                        super::SLEEP,
                    ]);
                }
                for name in wanted {
                    assert!(
                        diagnostics.layout.rect_of_action(action_id(name)).is_some(),
                        "{at}: {name} is not on the screen"
                    );
                }
                let drawn = format!("{screen:?}");
                if player.audio_available == super::Availability::Available {
                    assert!(drawn.contains(&format!("{}%", player.volume)), "{at}");
                    assert!(
                        drawn.contains(&speed_label(player.speed_percent)),
                        "{at}: the speed is not on the screen"
                    );
                }
                if text_scale == TextScale::Default && orientation == Orientation::Portrait {
                    // Where the panel has the room, nothing gives way.
                    assert_eq!(screen, player.screen(), "{at}");
                }
            }
        }
    }
}

#[test]
fn the_chapters_speed_and_sleep_screens_fit_a_clara_at_every_text_size() {
    use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, Orientation, TextScale};
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = crate::AppRunner::with_metrics(Shows, metrics);
            for view in [
                super::View::Speed,
                super::View::Sleep,
                super::View::Chapters,
            ] {
                let at = format!("{view:?}, {text_scale:?}, {orientation:?}");
                let mut player = loaded(40);
                player.view = view;
                player.speed_percent = 125;
                player.sleep = Sleep::EndOfChapter;
                let mut seen = Vec::new();
                loop {
                    let context = runner.context();
                    let screen = player.screen_for(&context);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    let errors = diagnostics
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(errors.is_empty(), "{at}: {errors:?}");
                    let drawn = format!("{screen:?}");
                    let next = match view {
                        super::View::Chapters => super::CHAPTERS_NEXT,
                        super::View::Sleep => super::SLEEP_NEXT,
                        _ => break,
                    };
                    seen.extend((0..40).filter(|index| {
                        drawn.contains(&format!("\"Chapter {} of the night\"", index + 1))
                    }));
                    seen.extend(
                        super::SLEEP_OPTIONS
                            .iter()
                            .enumerate()
                            .filter(|(_, (name, _))| {
                                diagnostics.layout.rect_of_action(action_id(name)).is_some()
                            })
                            .map(|(index, _)| 100 + index),
                    );
                    if diagnostics.layout.rect_of_action(action_id(next)).is_none() {
                        break;
                    }
                    assert!(player.press(&mut runner.context(), action_id(next)));
                }
                seen.sort_unstable();
                seen.dedup();
                match view {
                    super::View::Chapters => {
                        assert_eq!(seen, (0..40).collect::<Vec<_>>(), "{at}");
                    }
                    super::View::Sleep => {
                        assert_eq!(seen, (100..106).collect::<Vec<_>>(), "{at}");
                    }
                    _ => {}
                }
            }
        }
    }
}

#[test]
fn the_output_screen_pages_its_devices_to_fit_a_clara_at_every_text_size_held_either_way() {
    use kobo_ui::{Chrome, DiagnosticSeverity, DisplayMetrics, Orientation, TextScale};
    let device = |index: usize| BluetoothDevice {
        address: format!("AA:BB:CC:DD:EE:{index:02}"),
        name: format!("Travel headphones {index}"),
        kind: BluetoothDeviceKind::Audio,
        paired: index < 2,
        connected: index == 0,
    };
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner = crate::AppRunner::with_metrics(Shows, metrics);
            for (state, restart, trouble, count) in [
                ("restart owed", true, false, 9),
                ("trouble", false, true, 9),
                ("one device", true, false, 1),
                ("nothing found", true, false, 0),
            ] {
                let at = format!("{state}, {text_scale:?}, {orientation:?}");
                let mut player = AudioPlayer::shelf("book.mp3z", "Book");
                player.view = super::View::Bluetooth;
                player.bluetooth_enabled = super::Switch::On;
                player.restart_on_exit = restart;
                if trouble {
                    player.trouble =
                        Some("The Bluetooth audio service stopped responding.".to_owned());
                }
                player.devices = (0..count).map(device).collect();
                let mut seen = Vec::new();
                loop {
                    let context = runner.context();
                    let screen = player.screen_for(&context);
                    let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
                    let errors = diagnostics
                        .issues
                        .iter()
                        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(errors.is_empty(), "{at}: {errors:?}");
                    for name in [super::BLUETOOTH_TOGGLE, super::BLUETOOTH_SCAN] {
                        assert!(
                            diagnostics.layout.rect_of_action(action_id(name)).is_some(),
                            "{at}: {name}"
                        );
                    }
                    let drawn = format!("{screen:?}");
                    seen.extend((0..count).filter(|&index| {
                        drawn.contains(&format!("\"Travel headphones {index}\""))
                    }));
                    if diagnostics
                        .layout
                        .rect_of_action(action_id(super::DEVICES_NEXT))
                        .is_none()
                    {
                        break;
                    }
                    assert!(player.press(&mut runner.context(), action_id(super::DEVICES_NEXT)));
                }
                seen.sort_unstable();
                seen.dedup();
                assert_eq!(seen, (0..count).collect::<Vec<_>>(), "{at}");
            }
        }
    }
}

#[test]
fn play_without_an_output_opens_bluetooth_and_turns_it_on() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    player.bluetooth_known = true;
    let mut context = crate::Context::default();
    assert!(player.press(&mut context, action_id(super::PLAY)));
    assert_eq!(player.view, super::View::Bluetooth);
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::SetBluetooth { enabled: true })));
}

/// The one Bluetooth failure a listener can do something about is the radio
/// not having been started since the reader was switched on, which the
/// reader's own home screen does. The complaint says so; any other failure
/// is reported as what it is.
#[test]
fn a_radio_that_is_not_ready_says_what_to_do_about_it() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::SetBluetooth { enabled: true },
        &DeviceResult::Failed(crate::DeviceError::Unreachable),
    );
    let trouble = player.trouble.clone().expect("a complaint");
    assert!(trouble.contains("Kobo home screen"), "{trouble}");

    player.on_device_result(
        &mut context,
        &DeviceRequest::SetBluetooth { enabled: true },
        &DeviceResult::Failed(crate::DeviceError::Backend),
    );
    let trouble = player.trouble.clone().expect("a complaint");
    assert!(trouble.starts_with("Bluetooth failed:"), "{trouble}");
}

#[test]
fn connecting_after_play_loads_the_source_automatically() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    player.autoplay = super::PlayIntent::Autoplay;
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::ConnectBluetooth {
            address: "AA:BB:CC:DD:EE:FF".to_owned(),
        },
        &DeviceResult::Bluetooth {
            available: true,
            enabled: true,
            devices: vec![headphones(true)],
            restart_on_exit: false,
        },
    );
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::LoadAudio { .. })));
}

/// The reboot this warns about used to happen with no warning at all, and
/// was reported as the operating system crashing. The warning is owed only
/// when the runtime says the radio has actually been touched, so the
/// quieter case must stay quiet or the warning stops meaning anything.
#[test]
fn the_coming_restart_is_announced_only_when_it_is_really_coming() {
    fn reading(restart_on_exit: bool) -> String {
        let mut player = AudioPlayer::shelf("book.mp3z", "Book");
        player.view = super::View::Bluetooth;
        player.on_device_result(
            &mut crate::Context::default(),
            &DeviceRequest::ReadBluetooth,
            &DeviceResult::Bluetooth {
                available: true,
                enabled: true,
                devices: vec![headphones(false)],
                restart_on_exit,
            },
        );
        format!("{:?}", player.screen())
    }

    assert!(reading(true).contains("restart itself"));
    assert!(!reading(false).contains("restart itself"));
}

/// The debt is owed for the rest of the session once the radio is touched.
/// A later reading that answers false must not withdraw a promise the
/// reader has already been shown.
#[test]
fn the_restart_warning_is_never_withdrawn() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    player.view = super::View::Bluetooth;
    let mut context = crate::Context::default();
    for restart_on_exit in [true, false] {
        player.on_device_result(
            &mut context,
            &DeviceRequest::ReadBluetooth,
            &DeviceResult::Bluetooth {
                available: true,
                enabled: true,
                devices: vec![headphones(false)],
                restart_on_exit,
            },
        );
    }
    assert!(format!("{:?}", player.screen()).contains("restart itself"));
}

/// Reported from the reader as "clicking play a second time started it".
/// The player asked to be woken while the file was decoding and then threw
/// its own wake-up away, so nothing ever asked the backend again and the
/// screen sat on "Loading…" until a second press took the loaded path.
#[test]
fn a_book_that_is_still_decoding_is_asked_about_again() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    let mut context = crate::Context::default();

    // The load is answered while decoding is still running.
    player.on_device_result(
        &mut context,
        &DeviceRequest::LoadAudio {
            source: AudioSource::Shelf("book.mp3z".to_owned()),
        },
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Loading,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
        },
    );
    let poll = player.poll.expect("decoding should schedule a poll");
    drop(context.take_commands());

    // When that poll comes due the player must ask again, not go quiet.
    assert!(player.on_task(&mut context, poll, &TaskOutcome::Completed(Vec::new())));
    assert!(
        sent(&mut context)
            .iter()
            .any(|request| matches!(request, DeviceRequest::ReadAudio)),
        "a decoding player must keep asking or it never learns it is ready"
    );
}

/// The same press must also finish the job: once decoding lands, the play
/// the reader already asked for has to happen without a second press.
#[test]
fn the_press_that_started_a_load_still_plays_when_decoding_lands() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    player.autoplay = super::PlayIntent::Autoplay;
    player.loaded = super::LoadState::Loaded;
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudio,
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Ready,
            position_ms: 0,
            duration_ms: 30_000,
            volume: 70,
        },
    );
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::PlayAudio)));
}

/// Opening a thirty second book straight after a ten minute one showed ten
/// minutes, because the backend still held the previous book's numbers and
/// the fresh player adopted them on its opening read.
#[test]
fn a_new_player_does_not_wear_the_last_books_duration() {
    let mut player = AudioPlayer::shelf("short.mp3z", "Book");
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudio,
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Ready,
            position_ms: 44_000,
            duration_ms: 658_000,
            volume: 55,
        },
    );

    assert_eq!(
        player.duration_ms, 0,
        "that duration belongs to another book"
    );
    assert_eq!(player.position_ms, 0);
    assert_eq!(player.playback, AudioPlaybackState::Idle);
    // Volume and availability describe the device, not the book.
    assert_eq!(player.volume, 55);
    let drawn = format!("{:?}", player.screen());
    assert!(!drawn.contains("10:58"), "{drawn}");
}

#[test]
fn only_the_players_own_sleep_is_consumed() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    let mut context = crate::Context::default();
    assert!(!player.on_task(
        &mut context,
        crate::TaskId(99),
        &TaskOutcome::Completed(Vec::new())
    ));
}

/// Once a book is ready the player asks what it says about itself, sets
/// the speed it was given, and goes to where the listener left off, in that
/// order and before anything plays.
#[test]
fn a_ready_book_is_described_sped_up_and_resumed_before_it_plays() {
    let mut player = AudioPlayer::library("0/night.m4b", "Night")
        .speed(150)
        .resuming_at(Duration::from_secs(3_600));
    player.autoplay = super::PlayIntent::Autoplay;
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::LoadAudio {
            source: AudioSource::Library("0/night.m4b".to_owned()),
        },
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Loading,
            position_ms: 0,
            duration_ms: 0,
            volume: 70,
        },
    );
    drop(context.take_commands());
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudio,
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Ready,
            position_ms: 0,
            duration_ms: 7_200_000,
            volume: 70,
        },
    );
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::ReadAudioDetail,
                DeviceRequest::SetAudioSpeed { percent: 150 },
                DeviceRequest::SeekAudio {
                    position_ms: 3_600_000
                }
            ]
        ),
        "{requests:?}"
    );
    // The seek's own answer is what starts playback.
    player.on_device_result(
        &mut context,
        &DeviceRequest::SeekAudio {
            position_ms: 3_600_000,
        },
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Ready,
            position_ms: 3_600_000,
            duration_ms: 7_200_000,
            volume: 70,
        },
    );
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::PlayAudio)));
    assert!(player.detail_asked);

    // A runtime that answers a load with Ready at once, as the simulator
    // does, gets the same three requests from that one answer.
    let mut at_once = AudioPlayer::library("0/night.m4b", "Night")
        .speed(150)
        .resuming_at(Duration::from_secs(3_600));
    at_once.on_device_result(
        &mut context,
        &DeviceRequest::LoadAudio {
            source: AudioSource::Library("0/night.m4b".to_owned()),
        },
        &DeviceResult::Audio {
            available: true,
            state: AudioPlaybackState::Ready,
            position_ms: 0,
            duration_ms: 7_200_000,
            volume: 70,
        },
    );
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::ReadAudioDetail,
                DeviceRequest::SetAudioSpeed { percent: 150 },
                DeviceRequest::SeekAudio {
                    position_ms: 3_600_000
                }
            ]
        ),
        "{requests:?}"
    );
}

#[test]
fn the_description_brings_chapters_and_hands_the_cover_over_once() {
    let mut player = loaded(0);
    player.detail = None;
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudioDetail,
        &DeviceResult::AudioDetail(AudioDetail {
            title: "Night".to_owned(),
            chapters: chapters(4, 600_000),
            cover: vec![0xff, 0xd8],
            ..AudioDetail::default()
        }),
    );
    assert_eq!(player.chapters().len(), 4);
    assert_eq!(player.title(), "Night");
    // An application that knows the name keeps it over the file's.
    let mut named = loaded(0).keep_title();
    named.detail = player.detail.clone();
    assert_eq!(named.title(), "A History of the Moon");
    assert_eq!(player.take_cover_bytes(), Some(vec![0xff, 0xd8]));
    assert_eq!(player.take_cover_bytes(), None);
    // An older runtime that cannot describe the book is not trouble.
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudioDetail,
        &DeviceResult::Failed(crate::DeviceError::NotFound),
    );
    assert!(player.trouble.is_none());
}

#[test]
fn chapter_keys_step_between_chapter_starts() {
    let mut player = loaded(12);
    let mut context = crate::Context::default();
    // 3,675 s is 75 s into chapter 7, so back goes to its start.
    player.press(&mut context, action_id(super::CHAPTER_BACK));
    assert!(matches!(
        sent(&mut context).as_slice(),
        [DeviceRequest::SeekAudio {
            position_ms: 3_600_000
        }]
    ));
    // Within three seconds of the start, back goes to the chapter before.
    player.position_ms = 3_601_000;
    player.press(&mut context, action_id(super::CHAPTER_BACK));
    assert!(matches!(
        sent(&mut context).as_slice(),
        [DeviceRequest::SeekAudio {
            position_ms: 3_000_000
        }]
    ));
    player.press(&mut context, action_id(super::CHAPTER_FORWARD));
    assert!(matches!(
        sent(&mut context).as_slice(),
        [DeviceRequest::SeekAudio {
            position_ms: 3_600_000
        }]
    ));
    // The last chapter's "next" is the end of the book.
    player.position_ms = 7_000_000;
    player.press(&mut context, action_id(super::CHAPTER_FORWARD));
    assert!(matches!(
        sent(&mut context).as_slice(),
        [DeviceRequest::SeekAudio {
            position_ms: 7_200_000
        }]
    ));
}

#[test]
fn a_tap_on_a_chapter_seeks_to_it_and_plays() {
    let runner = crate::AppRunner::new(Shows);
    let mut player = loaded(12);
    let mut context = runner.context();
    assert!(player.press(&mut context, action_id(super::CHAPTERS)));
    assert_eq!(player.view, super::View::Chapters);
    let drawn = format!("{:?}", player.screen_for(&context));
    assert!(drawn.contains("Now · 10 min"), "{drawn}");
    // The second row on the first page is the second chapter.
    assert!(player.press(&mut context, action_id(super::CHAPTER_ACTIONS[1])));
    assert_eq!(player.view, super::View::Player);
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::SeekAudio {
                    position_ms: 600_000
                },
                DeviceRequest::PlayAudio
            ]
        ),
        "{requests:?}"
    );
}

#[test]
fn the_speed_screen_steps_and_presets_send_the_speed_and_keep_it_in_range() {
    let mut player = loaded(3);
    let mut context = crate::Context::default();
    player.press(&mut context, action_id(super::SPEED));
    assert!(player.press(&mut context, action_id(super::SPEED_MORE)));
    assert_eq!(player.speed_percent(), 105);
    assert!(player.press(&mut context, action_id("audio-speed-200")));
    assert_eq!(player.speed_percent(), 200);
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::SetAudioSpeed { percent: 105 },
                DeviceRequest::SetAudioSpeed { percent: 200 }
            ]
        ),
        "{requests:?}"
    );
    player.speed_percent = super::MAX_SPEED_PERCENT;
    player.press(&mut context, action_id(super::SPEED_MORE));
    assert_eq!(player.speed_percent(), super::MAX_SPEED_PERCENT);
    let drawn = format!("{:?}", player.screen());
    assert!(drawn.contains("3.0×"), "{drawn}");
}

#[test]
fn the_sleep_timer_pauses_when_it_fires_and_says_so() {
    let mut player = loaded(12);
    player.playback = AudioPlaybackState::Playing;
    let mut context = crate::Context::default();
    player.press(&mut context, action_id(super::SLEEP));
    assert!(player.press(&mut context, action_id("audio-sleep-30")));
    assert_eq!(player.sleep, Sleep::Minutes(30));
    assert_eq!(player.view, super::View::Player);
    let task = player.sleep_task.expect("a timer is a task");
    assert!(format!("{:?}", player.screen()).contains("Pauses in 30 min"));
    drop(context.take_commands());
    assert!(player.on_task(&mut context, task, &TaskOutcome::Completed(Vec::new())));
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::PauseAudio)));
    assert_eq!(player.sleep, Sleep::Off);
    assert!(format!("{:?}", player.screen()).contains("Paused by the sleep timer"));
    // The next press clears the notice.
    player.press(&mut context, action_id(super::VOLUME_UP));
    assert!(!format!("{:?}", player.screen()).contains("Paused by the sleep timer"));
}

/// A seek or a new speed must not hand the listener a fresh half hour.
#[test]
fn a_timer_in_minutes_keeps_counting_across_a_seek() {
    let mut player = loaded(12);
    player.playback = AudioPlaybackState::Playing;
    let mut context = crate::Context::default();
    player.sleep = Sleep::Minutes(30);
    player.sleep_until = Some(Instant::now() + Duration::from_secs(10 * 60));
    player.seek(&mut context, 100_000);
    let seconds = context
        .take_commands()
        .into_iter()
        .find_map(|command| match command {
            Command::Spawn {
                work: crate::Task::Sleep { seconds },
                ..
            } => Some(seconds),
            _ => None,
        })
        .expect("the timer is set again");
    assert!((595..=600).contains(&seconds), "{seconds}");
}

#[test]
fn an_end_of_chapter_timer_is_set_from_where_the_book_is_at_its_speed() {
    let mut player = loaded(12);
    player.playback = AudioPlaybackState::Playing;
    player.speed_percent = 150;
    let mut context = crate::Context::default();
    player.sleep = Sleep::EndOfChapter;
    player.schedule_sleep(&mut context);
    // 525 s are left in chapter 7; at one and a half times that is 350 s.
    let task = context
        .take_commands()
        .into_iter()
        .find_map(|command| match command {
            Command::Spawn {
                work: crate::Task::Sleep { seconds },
                ..
            } => Some(seconds),
            _ => None,
        });
    assert_eq!(task, Some(350));
}

#[test]
fn resuming_after_a_long_pause_rewinds_a_little() {
    let mut player = loaded(12);
    player.paused_since = Instant::now().checked_sub(Duration::from_secs(2 * 3_600));
    let mut context = crate::Context::default();
    player.press(&mut context, action_id(super::PLAY));
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::SeekAudio {
                    position_ms: 3_645_000
                },
                DeviceRequest::PlayAudio
            ]
        ),
        "{requests:?}"
    );
    // A short interruption needs no run-up.
    let mut quick = loaded(12);
    quick.paused_since = Some(Instant::now());
    quick.press(&mut context, action_id(super::PLAY));
    assert!(matches!(
        sent(&mut context).as_slice(),
        [DeviceRequest::PlayAudio]
    ));
}

#[test]
fn losing_the_headphones_keeps_the_place_and_the_play_control() {
    let mut player = loaded(12);
    player.playback = AudioPlaybackState::Playing;
    let mut context = crate::Context::default();
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadAudio,
        &DeviceResult::Failed(crate::DeviceError::Unreachable),
    );
    assert_eq!(player.playback, AudioPlaybackState::Paused);
    assert_eq!(player.position_ms, 3_675_000);
    let drawn = format!("{:?}", player.screen());
    assert!(drawn.contains("Connect Bluetooth headphones"), "{drawn}");
    assert!(drawn.contains("Play"), "{drawn}");
}

/// Tapping a book with headphones already on must not open the picker:
/// the player has asked what is connected, and Play waits for the answer.
#[test]
fn a_play_before_the_first_bluetooth_reading_waits_for_it() {
    let mut player = AudioPlayer::library("0/night.m4b", "Night");
    let mut context = crate::Context::default();
    player.start(&mut context);
    player.play(&mut context);
    assert_eq!(player.view, super::View::Player);
    drop(context.take_commands());
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadBluetooth,
        &DeviceResult::Bluetooth {
            available: true,
            enabled: true,
            devices: vec![headphones(true)],
            restart_on_exit: false,
        },
    );
    assert_eq!(player.view, super::View::Player);
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::LoadAudio { .. })));

    // With nothing connected the same wait ends at the picker.
    let mut alone = AudioPlayer::library("0/night.m4b", "Night");
    alone.start(&mut context);
    alone.play(&mut context);
    drop(context.take_commands());
    alone.on_device_result(
        &mut context,
        &DeviceRequest::ReadBluetooth,
        &DeviceResult::Bluetooth {
            available: true,
            enabled: false,
            devices: Vec::new(),
            restart_on_exit: false,
        },
    );
    assert_eq!(alone.view, super::View::Bluetooth);
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::SetBluetooth { enabled: true })));
}

#[test]
fn the_page_keys_skip_half_a_minute() {
    let mut player = loaded(12);
    let mut context = crate::Context::default();
    player.skip(&mut context, true);
    player.skip(&mut context, false);
    let requests = sent(&mut context);
    assert!(
        matches!(
            requests.as_slice(),
            [
                DeviceRequest::SeekAudio {
                    position_ms: 3_705_000
                },
                DeviceRequest::SeekAudio {
                    position_ms: 3_675_000
                }
            ]
        ),
        "{requests:?}"
    );
}

#[test]
fn a_tap_on_the_connected_device_disconnects_it_when_nothing_waits_to_play() {
    // It used to do nothing at all, under a row that offered nothing.
    let mut player = loaded(2);
    player.view = super::View::Bluetooth;
    let mut context = crate::Context::default();
    let words = format!("{:?}", player.bluetooth_screen(&context));
    assert!(words.contains("Tap to disconnect"), "{words}");
    assert!(player.press(&mut context, action_id(super::DEVICE_ACTIONS[0])));
    assert!(sent(&mut context)
        .iter()
        .any(|request| matches!(request, DeviceRequest::DisconnectBluetooth { .. })));
}

#[test]
fn a_scan_says_it_is_looking_until_its_results_are_read() {
    let mut player = AudioPlayer::shelf("book.mp3z", "Book");
    player.view = super::View::Bluetooth;
    player.bluetooth_enabled = super::Switch::On;
    player.bluetooth_known = true;
    let mut context = crate::Context::default();
    assert!(player.press(&mut context, action_id(super::BLUETOOTH_SCAN)));
    let words = format!("{:?}", player.bluetooth_screen(&context));
    assert!(
        words.contains("Looking for headphones and speakers"),
        "{words}"
    );
    assert!(words.contains("Scanning"), "{words}");
    // A second tap does not start the scan over.
    let _ = sent(&mut context);
    assert!(player.press(&mut context, action_id(super::BLUETOOTH_SCAN)));
    assert!(sent(&mut context).is_empty());
    player.on_device_result(
        &mut context,
        &DeviceRequest::ReadBluetooth,
        &DeviceResult::Bluetooth {
            available: true,
            enabled: true,
            devices: vec![headphones(false)],
            restart_on_exit: false,
        },
    );
    let words = format!("{:?}", player.bluetooth_screen(&context));
    assert!(!words.contains("Scanning"), "{words}");
}
