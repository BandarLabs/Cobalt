use super::{
    archive_name, parse_index, parse_places, play_action, read_checkpoint, resume_from,
    size_on_disk, write_checkpoint, write_places, Audiobook, Book, Checkpoint, Place, Stage, CHECK,
    CHECKPOINT_KEY, NOW_PLAYING, PLACES_KEY, SAMPLE_NAME, SAMPLE_TITLE, SECRETS,
};
use crate::pipeline;
use kobo_sdk::{action_id, AudioEntry, StandardState, CLARA_BW_METRICS, MAX_ROWS};
use kobo_sdk::{Context, KoboApp, StoreResult};
use std::time::Duration;

fn entry(id: &str, title: &str, kind: u8, parts: u16, bytes: u64) -> AudioEntry {
    AudioEntry {
        id: id.to_owned(),
        title: title.to_owned(),
        kind,
        parts,
        bytes,
    }
}

fn book(id: &str, title: &str) -> Book {
    Book {
        id: id.to_owned(),
        title: title.to_owned(),
        kind: 7,
        parts: 1,
        bytes: 1_000,
    }
}

#[test]
fn a_title_becomes_a_safe_kobo_filename() {
    assert_eq!(archive_name("Moon: Past & Future"), "moon-past-future.mp3z");
}

#[test]
fn compose_progress_complete_and_failure_screens_fit_a_clara() {
    let mut app = Audiobook::default();
    for stage in [Stage::Compose, Stage::Setup, Stage::Research, Stage::Failed] {
        app.stage = stage;
        app.title = "A researched history of the night sky".to_owned();
        app.summary = "An original, source-grounded tour of how people learned to understand the Moon, planets, and stars.".to_owned();
        app.archive_name = "history-of-the-night-sky.mp3z".to_owned();
        app.trouble = Some((
            StandardState::Error,
            "The provider could not complete this request.".to_owned(),
        ));
        let issues = app
            .screen(&kobo_sdk::Context::default())
            .validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "{stage:?}: {issues:?}");
    }
}

/// The hint used to be written to a field the compose screen never drew,
/// so a topic under three characters made the Create button do nothing at
/// all. It has to reach the screen, under the field it is about.
#[test]
fn a_short_topic_puts_a_visible_hint_on_the_compose_screen() {
    let app = Audiobook {
        stage: Stage::Compose,
        hint: Some("That is too short. Add a few words."),
        ..Audiobook::default()
    };
    let screen = app.screen(&kobo_sdk::Context::default());
    let drawn = format!("{screen:?}");
    assert!(drawn.contains("That is too short"), "{drawn}");
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
}

/// Every stage the library can be in has to fit, including a reader with
/// more audiobooks on it than one panel holds, a book playing, and a
/// listing cut short.
#[test]
fn every_library_screen_fits_a_clara() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let context = runner.context();
    let mut app = Audiobook::default();
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
    app.library = Some(Vec::new());
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
    let entries = (0..40)
        .map(|index| {
            entry(
                &format!("0/Audiobooks/A researched history of the night sky, part {index}.m4b"),
                &format!("A researched history of the night sky, part {index}"),
                2,
                1,
                940_000_000,
            )
        })
        .collect::<Vec<_>>();
    app.places = vec![Place {
        id: entries[7].id.clone(),
        position_ms: 3_600_000,
        duration_ms: 30_000_000,
        speed_percent: 125,
        finished: false,
        last_played: 1_700_000_000,
        title: "The Night Sky, Part Seven".to_owned(),
        author: "Ada Example".to_owned(),
    }];
    app.player = Some(kobo_sdk::audio::AudioPlayer::library(
        entries[7].id.clone(),
        "The Night Sky, Part Seven",
    ));
    app.playing = Some(entries[7].id.clone());
    app.listed(&context, &entries, true);
    assert!(app.pages.len() > 1, "40 audiobooks are more than one page");
    // The book on the go leads, under the row for the player.
    assert_eq!(app.pages[0][0], 7);
    for page in 0..app.pages.len() {
        app.page = page;
        let issues = app
            .screen(&kobo_sdk::Context::default())
            .validate(&CLARA_BW_METRICS);
        assert!(issues.is_empty(), "page {page}: {issues:?}");
    }
    let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
    assert!(drawn.contains("Now playing"), "{drawn}");
    assert!(drawn.contains("first two thousand"), "{drawn}");
}

/// A book is called what it said when it was opened, else what the studio
/// saved it as, else its file name turned back into words by the runtime.
#[test]
fn a_book_is_named_by_what_it_said_then_the_index_then_its_file() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let context = runner.context();
    let mut app = Audiobook {
        titles: vec![("moon.mp3z".to_owned(), "The Moon".to_owned())],
        ..Audiobook::default()
    };
    app.listed(
        &context,
        &[
            entry("0/Audiobooks/moon.mp3z", "moon", 7, 1, 4_000_000),
            entry(
                "0/Audiobooks/stray-recording.mp3z",
                "stray-recording",
                7,
                1,
                1_000_000,
            ),
            entry("0/Audiobooks/Dune", "Dune", 1, 42, 5_000_000_000),
        ],
        false,
    );
    app.places = vec![Place {
        id: "0/Audiobooks/Dune".to_owned(),
        title: "Dune, by Frank Herbert".to_owned(),
        author: "Frank Herbert".to_owned(),
        ..Place::default()
    }];
    let books = app.library.clone().expect("the runtime answered");
    assert_eq!(app.title_of(&books[0]), "The Moon");
    assert_eq!(app.title_of(&books[1]), "stray-recording");
    assert_eq!(app.title_of(&books[2]), "Dune, by Frank Herbert");
    assert_eq!(Audiobook::status_of(&books[0], None), "MP3Z · 3 MB");
    assert_eq!(
        Audiobook::status_of(&books[2], app.place_of("0/Audiobooks/Dune")),
        "Frank Herbert · MP3 · 42 parts · 4.6 GB"
    );
}

/// The library lists what is on the go first, newest listening first,
/// then what has not been started, then what is finished.
#[test]
fn books_on_the_go_come_first_and_finished_ones_last() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let context = runner.context();
    let mut app = Audiobook {
        places: vec![
            Place {
                id: "0/b.m4b".to_owned(),
                position_ms: 100_000,
                duration_ms: 200_000,
                last_played: 10,
                ..Place::default()
            },
            Place {
                id: "0/c.m4b".to_owned(),
                position_ms: 100_000,
                duration_ms: 200_000,
                last_played: 20,
                ..Place::default()
            },
            Place {
                id: "0/a.m4b".to_owned(),
                finished: true,
                duration_ms: 200_000,
                ..Place::default()
            },
        ],
        ..Audiobook::default()
    };
    app.listed(
        &context,
        &[
            entry("0/a.m4b", "a", 2, 1, 10),
            entry("0/b.m4b", "b", 2, 1, 10),
            entry("0/c.m4b", "c", 2, 1, 10),
            entry("0/d.m4b", "d", 2, 1, 10),
        ],
        false,
    );
    let order = app.pages.concat();
    assert_eq!(order, [2, 1, 3, 0]);
    let drawn = format!("{:?}", app.screen(&Context::default()));
    assert!(drawn.contains("1 min left"), "{drawn}");
    assert!(drawn.contains("Finished"), "{drawn}");
}

/// Where the listener got to survives the record, and a broken line loses
/// one book rather than every book.
#[test]
fn places_round_trip_and_a_broken_line_loses_only_itself() {
    let places = vec![
        Place {
            id: "0/Audiobooks/The Quiet Shelf.m4b".to_owned(),
            position_ms: 3_675_000,
            duration_ms: 7_200_000,
            speed_percent: 125,
            finished: false,
            last_played: 1_700_000_000,
            title: "The Quiet Shelf".to_owned(),
            author: "Ada Example".to_owned(),
        },
        Place {
            id: "shelf:the-moon.mp3z".to_owned(),
            finished: true,
            ..Place::default()
        },
    ];
    let written = write_places(&places);
    assert_eq!(parse_places(&written), places);
    let mut broken = written.clone();
    broken.extend_from_slice("\u{1e}v1\u{1f}only".as_bytes());
    assert_eq!(parse_places(&broken), places);
    assert!(parse_places(b"").is_empty());
}

/// Coming back to a book after a while starts a little before where it
/// stopped, and more the longer it has been.
#[test]
fn coming_back_rewinds_by_how_long_it_has_been() {
    assert_eq!(resume_from(100_000, Duration::from_secs(5)), 100_000);
    assert_eq!(resume_from(100_000, Duration::from_secs(600)), 90_000);
    assert_eq!(resume_from(100_000, Duration::from_secs(86_400)), 70_000);
    assert_eq!(resume_from(5_000, Duration::from_secs(86_400)), 0);
}

/// Opening a book goes to where it was left, at the speed it was left at,
/// and starts it; backing out keeps it playing with the way back at the
/// head of the library.
#[test]
fn a_book_opens_where_it_was_left_and_keeps_playing_behind_the_library() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        places: vec![Place {
            id: "0/Audiobooks/night.m4b".to_owned(),
            position_ms: 3_600_000,
            duration_ms: 7_200_000,
            speed_percent: 150,
            finished: false,
            last_played: 1,
            title: "Night".to_owned(),
            author: "Ada Example".to_owned(),
        }],
        ..Audiobook::default()
    };
    app.listed(
        &context,
        &[entry("0/Audiobooks/night.m4b", "night", 2, 1, 10)],
        false,
    );
    app.play(&mut context, 0);
    assert_eq!(app.stage, Stage::Player);
    assert_eq!(app.playing.as_deref(), Some("0/Audiobooks/night.m4b"));
    let player = app.player.as_ref().expect("a player");
    assert_eq!(player.speed_percent(), 150);
    let drawn = format!("{:?}", app.screen(&context));
    assert!(drawn.contains("Night"), "{drawn}");
    assert!(drawn.contains("Ada Example"), "{drawn}");
    // Back keeps the player and the book, and the library leads back to it.
    app.on_action(&mut context, kobo_sdk::ActionId::BACK);
    assert_eq!(app.stage, Stage::Library);
    assert!(app.player.is_some());
    let drawn = format!("{:?}", app.screen(&context));
    assert!(drawn.contains("Now playing"), "{drawn}");
    app.on_action(&mut context, action_id(NOW_PLAYING));
    assert_eq!(app.stage, Stage::Player);
}

/// A pause writes the place at once; a reading that only moved the clock
/// a little does not.
#[test]
fn the_place_is_written_at_a_pause_and_once_a_minute_while_playing() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        player: Some(kobo_sdk::audio::AudioPlayer::library(
            "0/Audiobooks/night.m4b",
            "Night",
        )),
        playing: Some("0/Audiobooks/night.m4b".to_owned()),
        ..Audiobook::default()
    };
    let reading = |state, position_ms| kobo_sdk::DeviceResult::Audio {
        available: true,
        state,
        position_ms,
        duration_ms: 7_200_000,
        volume: 70,
    };
    // The load lands.
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::LoadAudio {
            source: kobo_sdk::AudioSource::Library("0/Audiobooks/night.m4b".to_owned()),
        },
        reading(kobo_sdk::AudioPlaybackState::Ready, 0),
    );
    drop(context.take_commands());
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::ReadAudio,
        reading(kobo_sdk::AudioPlaybackState::Playing, 20_000),
    );
    let writes = |commands: &[kobo_sdk::Command]| {
        commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    kobo_sdk::Command::Store(kobo_sdk::StoreRequest::Save { key, .. }) if key == PLACES_KEY
                )
            })
            .count()
    };
    assert_eq!(
        writes(&context.take_commands()),
        0,
        "twenty seconds in is not worth a write"
    );
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::ReadAudio,
        reading(kobo_sdk::AudioPlaybackState::Playing, 70_000),
    );
    assert_eq!(writes(&context.take_commands()), 1, "a minute in is");
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::PauseAudio,
        reading(kobo_sdk::AudioPlaybackState::Paused, 75_000),
    );
    assert_eq!(writes(&context.take_commands()), 1, "a pause always is");
    assert_eq!(app.places[0].position_ms, 75_000);
    // The end of the book is remembered as finished, from the start.
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::ReadAudio,
        reading(kobo_sdk::AudioPlaybackState::Finished, 7_200_000),
    );
    assert!(app.places[0].finished);
    assert_eq!(app.places[0].position_ms, 0);
}

/// A runtime that cannot list the reader's audiobooks still lists the
/// books made here, from the shelf, and plays them from the shelf.
#[test]
fn without_a_listing_the_shelf_is_the_library() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        titles: vec![("moon.mp3z".to_owned(), "The Moon".to_owned())],
        ..Audiobook::default()
    };
    app.on_device_result(
        &mut context,
        kobo_sdk::DeviceRequest::ListAudio,
        kobo_sdk::DeviceResult::Denied(kobo_sdk::DenyReason::Unsupported),
    );
    assert!(context.take_commands().iter().any(|command| matches!(
        command,
        kobo_sdk::Command::Store(kobo_sdk::StoreRequest::ShelfList)
    )));
    app.on_store(
        &mut context,
        StoreResult::Shelf(vec![
            ("moon.mp3z".to_owned(), 4_000_000),
            ("moon.part-001".to_owned(), 900_000),
            ("notes.txt".to_owned(), 12),
        ]),
    );
    let books = app.library.clone().expect("the shelf answered");
    assert_eq!(books.len(), 1);
    assert_eq!(app.title_of(&books[0]), "The Moon");
    app.play(&mut context, 0);
    assert_eq!(app.playing.as_deref(), Some("shelf:moon.mp3z"));
    let drawn = format!("{:?}", app.screen(&context));
    assert!(drawn.contains("The Moon"), "{drawn}");
}

/// An audiobook took four minutes and fourteen narration calls to make.
/// Cancelling the next one must not lose it, or the book playing.
#[test]
fn a_reset_keeps_what_is_already_on_the_shelf() {
    let mut app = Audiobook {
        stage: Stage::Narrate,
        titles: vec![("moon.mp3z".to_owned(), "The Moon".to_owned())],
        library: Some(vec![book("0/Audiobooks/moon.mp3z", "moon")]),
        player: Some(kobo_sdk::audio::AudioPlayer::shelf("moon.mp3z", "The Moon")),
        playing: Some("shelf:moon.mp3z".to_owned()),
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    };
    app.reset();
    assert_eq!(app.stage, Stage::Library);
    assert!(app.topic.is_empty());
    assert_eq!(app.library.expect("the library survived").len(), 1);
    assert_eq!(app.titles.len(), 1);
    assert!(app.player.is_some());
}

#[test]
fn the_index_survives_a_round_trip_and_ignores_a_broken_line() {
    let saved = "moon.mp3z\tThe Moon\nrubbish\ntides.mp3z\tThe Tides\n";
    assert_eq!(
        parse_index(saved.as_bytes()),
        vec![
            ("moon.mp3z".to_owned(), "The Moon".to_owned()),
            ("tides.mp3z".to_owned(), "The Tides".to_owned()),
        ]
    );
    assert!(parse_index(b"").is_empty());
}

#[test]
fn a_size_is_reported_in_the_coarsest_honest_unit() {
    assert_eq!(size_on_disk(9_437_184), "9 MB");
    assert_eq!(size_on_disk(4_096), "4 KB");
    assert_eq!(size_on_disk(0), "1 KB");
    assert_eq!(size_on_disk(5_000_000_000), "4.6 GB");
}

/// Every row on the library has its own action, or tapping one book plays
/// another.
#[test]
fn each_row_has_its_own_action() {
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..MAX_ROWS {
        assert!(seen.insert(action_id(&play_action(index))), "{index}");
    }
}

/// A topic submit asks the runtime which accounts are installed rather
/// than spending first and finding out later.
#[test]
fn a_submit_checks_every_account_before_spending() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Compose,
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    };
    app.begin(&mut context);
    assert_eq!(app.stage, Stage::Setup);
    assert!(app.task.is_none(), "nothing is spent during the check");
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
}

/// The preflight answer names every missing key at once, with the exact
/// command for each, while the reader has spent nothing.
#[test]
fn a_missing_key_is_named_with_the_command_that_installs_it() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Setup,
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    };
    app.checked_secrets(&mut context, &["openai".to_owned()]);
    assert_eq!(app.stage, Stage::Keys);
    assert!(app.task.is_none(), "nothing was spent");
    let drawn = format!("{:?}", app.screen(&Context::default()));
    assert!(drawn.contains("secret set exa"), "{drawn}");
    assert!(drawn.contains("secret set elevenlabs"), "{drawn}");
    assert!(!drawn.contains("secret set openai"), "{drawn}");
    assert!(drawn.contains("Check again"), "{drawn}");
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
}

/// The dead end this replaced: the only way out threw the topic away.
/// Check again asks the runtime once more and keeps what was typed.
#[test]
fn checking_again_keeps_the_topic_and_asks_the_runtime_once_more() {
    let mut runner = kobo_sdk::AppRunner::new(Audiobook {
        stage: Stage::Keys,
        missing: vec!["exa"],
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    });
    let commands = runner.action(action_id(CHECK));
    assert_eq!(runner.app().stage, Stage::Setup);
    assert_eq!(runner.app().topic, "the moon");
    assert!(commands.iter().any(|command| matches!(
        command,
        kobo_sdk::Command::Device(kobo_sdk::DeviceRequest::CheckSecrets { .. })
    )));
}

/// Every key present leads to the estimate, not to the first payment.
#[test]
fn the_cost_and_time_are_shown_before_anything_is_spent() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Setup,
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    };
    let all = SECRETS
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    app.checked_secrets(&mut context, &all);
    assert_eq!(app.stage, Stage::Confirm);
    assert!(app.task.is_none(), "nothing is spent before Create");
    let drawn = format!("{:?}", app.screen(&Context::default()));
    assert!(drawn.contains("minutes"), "{drawn}");
    assert!(drawn.contains("ElevenLabs characters"), "{drawn}");
    let estimate = pipeline::Estimate::for_a_book();
    assert!(
        drawn.contains(&format!(
            "${} to ${}",
            estimate.dollars.0, estimate.dollars.1
        )),
        "{drawn}"
    );
    assert!(drawn.contains("An estimate"), "{drawn}");
    app.start_creation(&mut context);
    assert_eq!(app.stage, Stage::Research);
    assert!(app.task.is_some(), "Create starts the research");
}

/// A runtime that cannot answer the check leaves the flow to find a
/// missing key at the stage that spends it, the way it always has.
#[test]
fn an_unanswerable_preflight_defers_to_the_stages() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Setup,
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    };
    app.skip_preflight(&mut context);
    assert_eq!(
        app.stage,
        Stage::Confirm,
        "the estimate is still shown first"
    );
    assert!(app.task.is_none());
}

/// A checkpoint carries the script and the stopping point, and only a
/// complete record comes back.
#[test]
fn a_checkpoint_round_trips_and_rejects_short_records() {
    let checkpoint = Checkpoint {
        topic: "the moon".to_owned(),
        language: pipeline::Language::default(),
        title: "The Moon".to_owned(),
        summary: "A tour of the night sky.".to_owned(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec![
            "Part one, spoken.".to_owned(),
            "Part two, spoken.".to_owned(),
            "Part three, spoken.".to_owned(),
        ],
        next_part: 2,
    };
    let saved = write_checkpoint(&checkpoint);
    assert_eq!(read_checkpoint(&saved), Some(checkpoint.clone()));
    assert_eq!(read_checkpoint(b""), None);
    assert_eq!(read_checkpoint(b"v1\tonly\tthree\tfields\npart"), None);
    // A stopping point past the parts is no record at all.
    let text = String::from_utf8(write_checkpoint(&checkpoint)).unwrap();
    let broken = text.replace("\t2\n", "\t9\n");
    assert_eq!(read_checkpoint(broken.as_bytes()), None);
}

/// Narration interrupted at part three of five resumes at part three,
/// not from the beginning: the calls already paid for are not made again.
#[test]
fn an_interrupted_narration_resumes_where_it_stopped() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Failed,
        title: "The Moon".to_owned(),
        parts: vec![
            "one".to_owned(),
            "two".to_owned(),
            "three".to_owned(),
            "four".to_owned(),
            "five".to_owned(),
        ],
        next_part: 2,
        ..Audiobook::default()
    };
    app.resume(&mut context);
    assert_eq!(app.stage, Stage::Narrate);
    assert_eq!(
        app.next_part, 2,
        "part three is asked for again, not part one"
    );
    assert!(app.task.is_some(), "narration is back in flight");
}

fn store_requests(commands: &[kobo_sdk::Command]) -> Vec<kobo_sdk::StoreRequest> {
    commands
        .iter()
        .filter_map(|command| match command {
            kobo_sdk::Command::Store(request) => Some(request.clone()),
            _ => None,
        })
        .collect()
}

fn speech_requests(commands: &[kobo_sdk::Command]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            kobo_sdk::Command::Spawn {
                work: kobo_sdk::Task::Post { url, body, .. },
                ..
            } if url.contains("text-to-speech") => Some(body.clone()),
            _ => None,
        })
        .collect()
}

fn moon_checkpoint(saved: usize) -> Checkpoint {
    Checkpoint {
        topic: "the moon".to_owned(),
        language: pipeline::Language::default(),
        title: "The Moon".to_owned(),
        summary: String::new(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
        next_part: saved,
    }
}

/// After a restart the parts already narrated are read back off the
/// shelf, and only the part after them is paid for. This used to start
/// again from part one and pay for every part a second time.
#[test]
fn a_stored_checkpoint_reads_saved_parts_back_instead_of_narrating_them_again() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Compose,
        checkpoint: Some(moon_checkpoint(2)),
        ..Audiobook::default()
    };
    app.resume(&mut context);
    assert_eq!(app.stage, Stage::Restore);
    let commands = context.take_commands();
    assert!(
        speech_requests(&commands).is_empty(),
        "nothing is paid for yet"
    );
    assert!(store_requests(&commands).iter().any(|request| matches!(
        request,
        kobo_sdk::StoreRequest::ShelfRead { name, .. } if name == "the-moon.part-001"
    )));
    for (index, name) in ["the-moon.part-001", "the-moon.part-002"]
        .iter()
        .enumerate()
    {
        app.on_store(
            &mut context,
            StoreResult::ShelfRead {
                name: (*name).to_owned(),
                offset: 0,
                bytes: vec![u8::try_from(index).unwrap(); 300],
                size: 300,
            },
        );
    }
    assert_eq!(app.stage, Stage::Narrate);
    assert_eq!(app.tracks.len(), 2, "both saved parts came back");
    assert_eq!(app.next_part, 2);
    let speech = speech_requests(&context.take_commands());
    assert_eq!(speech.len(), 1, "{speech:?}");
    assert!(speech[0].contains("three"), "only part three is narrated");
}

/// A part the shelf cannot give back is narrated again, and so is
/// everything after it, but nothing before it.
#[test]
fn a_saved_part_that_is_missing_is_narrated_again_from_there() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Compose,
        checkpoint: Some(moon_checkpoint(2)),
        ..Audiobook::default()
    };
    app.resume(&mut context);
    app.on_store(
        &mut context,
        StoreResult::ShelfRead {
            name: "the-moon.part-001".to_owned(),
            offset: 0,
            bytes: vec![1; 300],
            size: 300,
        },
    );
    let _ = context.take_commands();
    app.on_store(
        &mut context,
        StoreResult::Denied(kobo_sdk::StoreError::Missing),
    );
    assert_eq!(app.stage, Stage::Narrate);
    assert_eq!(app.next_part, 1);
    let speech = speech_requests(&context.take_commands());
    assert_eq!(speech.len(), 1);
    assert!(speech[0].contains("two"), "{speech:?}");
}

/// Each narrated part is written to the shelf, and the checkpoint moves
/// only once it is there, so a restart is never promised audio that was
/// not saved.
#[test]
fn a_narrated_part_is_kept_on_the_shelf_before_the_checkpoint_counts_it() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Narrate,
        title: "The Moon".to_owned(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned()],
        ..Audiobook::default()
    };
    app.received_voice(&mut context, vec![7; 300]);
    let requests = store_requests(&context.take_commands());
    assert!(requests.iter().any(|request| matches!(
        request,
        kobo_sdk::StoreRequest::ShelfWrite { name, last: true, .. } if name == "the-moon.part-001"
    )));
    assert!(
        !requests.iter().any(|request| matches!(
            request,
            kobo_sdk::StoreRequest::Save { key, .. } if key == CHECKPOINT_KEY
        )),
        "the checkpoint counted a part before it was saved"
    );
    app.on_store(
        &mut context,
        StoreResult::ShelfWritten {
            name: "the-moon.part-001".to_owned(),
            size: 300,
        },
    );
    let saved = store_requests(&context.take_commands())
        .into_iter()
        .find_map(|request| match request {
            kobo_sdk::StoreRequest::Save { key, value } if key == CHECKPOINT_KEY => {
                read_checkpoint(&value)
            }
            _ => None,
        })
        .expect("the checkpoint moved");
    assert_eq!(saved.next_part, 1);
    assert_eq!(app.checkpoint.map(|kept| kept.next_part), Some(1));
}

/// The book's own upload waits for the last part's write, so the two
/// never answer on the same shelf at once.
#[test]
fn the_book_is_packaged_only_once_the_last_part_is_on_the_shelf() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Narrate,
        title: "The Moon".to_owned(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec!["one".to_owned()],
        ..Audiobook::default()
    };
    app.received_voice(&mut context, vec![7; 300]);
    assert_eq!(
        app.stage,
        Stage::Narrate,
        "packaged before the part was saved"
    );
    assert!(app.upload.is_none());
    app.on_store(
        &mut context,
        StoreResult::ShelfWritten {
            name: "the-moon.part-001".to_owned(),
            size: 300,
        },
    );
    assert_eq!(app.stage, Stage::Save);
    assert!(app.upload.is_some());
}

/// Once the book is saved its parts are inside it, so they go.
#[test]
fn a_saved_book_removes_the_parts_it_was_made_from() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Save,
        title: "The Moon".to_owned(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned()],
        checkpoint: Some(moon_checkpoint(2)),
        upload: Some(kobo_sdk::ShelfUpload::new("the-moon.mp3z", vec![1; 10])),
        ..Audiobook::default()
    };
    if let Some(upload) = app.upload.as_mut() {
        upload.start(&mut context);
    }
    let _ = context.take_commands();
    app.on_store(
        &mut context,
        StoreResult::ShelfWritten {
            name: "the-moon.mp3z".to_owned(),
            size: 10,
        },
    );
    let removed = store_requests(&context.take_commands())
        .into_iter()
        .filter_map(|request| match request {
            kobo_sdk::StoreRequest::ShelfRemove { name } => Some(name),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        removed.contains(&"the-moon.part-001".to_owned()),
        "{removed:?}"
    );
    assert!(
        removed.contains(&"the-moon.part-002".to_owned()),
        "{removed:?}"
    );
    assert!(app.checkpoint.is_none());
}

/// A cancel after some parts are saved leaves the composer offering a
/// resume that reads those parts back, not one from the first part.
#[test]
fn a_cancel_keeps_the_saved_parts_resumable() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Narrate,
        title: "The Moon".to_owned(),
        archive_name: "the-moon.mp3z".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
        ..Audiobook::default()
    };
    app.received_voice(&mut context, vec![7; 300]);
    app.on_store(
        &mut context,
        StoreResult::ShelfWritten {
            name: "the-moon.part-001".to_owned(),
            size: 300,
        },
    );
    app.reset();
    assert_eq!(app.checkpoint.as_ref().map(|kept| kept.next_part), Some(1));
    app.resume(&mut context);
    assert_eq!(app.stage, Stage::Restore);
}

/// The failure screen leads with resuming when there is something to
/// resume, and does not offer it when there is not.
#[test]
fn the_failed_screen_offers_resume_only_when_it_can() {
    let mut app = Audiobook {
        stage: Stage::Failed,
        trouble: Some((StandardState::Error, "The request failed.".to_owned())),
        ..Audiobook::default()
    };
    let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
    assert!(!drawn.contains("Resume"), "{drawn}");
    app.parts = vec!["one".to_owned()];
    let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
    assert!(drawn.contains("Resume"), "{drawn}");
    assert!(drawn.contains("Start over"), "{drawn}");
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
}

/// An empty shelf is the moment the sample is for: no accounts, no
/// network, just proof the player works.
#[test]
fn an_empty_library_offers_the_free_sample() {
    let mut app = Audiobook {
        library: Some(Vec::new()),
        ..Audiobook::default()
    };
    let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
    assert!(drawn.contains("Play the sample"), "{drawn}");
    assert!(app
        .screen(&Context::default())
        .validate(&CLARA_BW_METRICS)
        .is_empty());
    // A reader with books on it does not need the offer; the sample is
    // there once saved, listed like any other book.
    app.titles = vec![(SAMPLE_NAME.to_owned(), SAMPLE_TITLE.to_owned())];
    app.listed(
        &kobo_sdk::AppRunner::new(Audiobook::default()).context(),
        &[entry(
            &format!("0/Audiobooks/{SAMPLE_NAME}"),
            "the-quiet-shelf",
            7,
            1,
            900_000,
        )],
        false,
    );
    let drawn = format!("{:?}", app.screen(&kobo_sdk::Context::default()));
    assert!(drawn.contains(SAMPLE_TITLE), "{drawn}");
}

/// The sample plays like any book once it is on the shelf, and saves
/// itself through the same upload when it is not.
#[test]
fn the_sample_saves_then_plays_like_any_book() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        library: Some(Vec::new()),
        ..Audiobook::default()
    };
    app.play_sample(&mut context);
    assert_eq!(app.stage, Stage::Save);
    assert!(app.upload.is_some(), "the sample is being saved first");
    assert!(app.saving_sample);
    let mut saved = Audiobook {
        library: Some(vec![book(
            &format!("0/Audiobooks/{SAMPLE_NAME}"),
            "the-quiet-shelf",
        )]),
        ..Audiobook::default()
    };
    saved.play_sample(&mut context);
    assert_eq!(saved.stage, Stage::Player);
    assert!(saved.upload.is_none(), "no second save for a saved sample");
}

/// A key that goes missing before the script exists gets the keys
/// screen; one that goes missing during narration keeps the resume and
/// names the command.
#[test]
fn a_key_missing_part_way_names_its_command_and_keeps_what_was_made() {
    let runner = kobo_sdk::AppRunner::new(Audiobook::default());
    let mut context = runner.context();
    let mut app = Audiobook {
        stage: Stage::Research,
        ..Audiobook::default()
    };
    app.task = context.spawn(pipeline::research("the moon"));
    let task = app.task.expect("research in flight");
    app.on_task(
        &mut context,
        task,
        kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
    );
    assert_eq!(app.stage, Stage::Keys);
    assert_eq!(app.missing, vec!["exa"]);

    let mut app = Audiobook {
        stage: Stage::Narrate,
        title: "The Moon".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned()],
        ..Audiobook::default()
    };
    app.task = context.spawn(pipeline::speech("one", pipeline::Language::default()));
    let task = app.task.expect("narration in flight");
    app.on_task(
        &mut context,
        task,
        kobo_sdk::TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential),
    );
    assert_eq!(app.stage, Stage::Failed);
    let (state, advice) = app.trouble.clone().expect("a failure was recorded");
    assert_eq!(state, StandardState::PermissionDenied);
    assert!(advice.contains("kobo secret set elevenlabs"), "{advice}");
    assert!(format!("{:?}", app.screen(&Context::default())).contains("Resume"));
}

/// A dropped connection used to leave "Try another topic" as the only
/// way on, which threw the typed topic away.
#[test]
fn a_dropped_connection_offers_to_try_the_same_topic_again() {
    let mut runner = kobo_sdk::AppRunner::new(Audiobook {
        stage: Stage::Research,
        topic: "the moon".to_owned(),
        ..Audiobook::default()
    });
    runner.app_mut().fail_task(kobo_sdk::TaskError::Offline);
    let drawn = format!("{:?}", runner.app().screen(&Context::default()));
    assert!(drawn.contains("Try again"), "{drawn}");
    assert!(drawn.contains("New topic"), "{drawn}");
    assert!(!drawn.contains("Try another topic"), "{drawn}");
    runner.action(action_id(CHECK));
    assert_eq!(runner.app().stage, Stage::Setup);
    assert_eq!(runner.app().topic, "the moon");
}

/// The resume button stays one short word, and the message says where
/// it carries on.
#[test]
fn a_failure_with_something_to_resume_says_which_part_it_carries_on_with() {
    let app = Audiobook {
        stage: Stage::Failed,
        title: "A researched history of the night sky and the people who mapped it".to_owned(),
        parts: vec!["one".to_owned(), "two".to_owned(), "three".to_owned()],
        next_part: 1,
        trouble: Some((
            StandardState::Error,
            "ElevenLabs did not answer.".to_owned(),
        )),
        ..Audiobook::default()
    };
    let drawn = format!("{:?}", app.screen(&Context::default()));
    assert!(drawn.contains("carries on from part 2 of 3"), "{drawn}");
    assert!(!drawn.contains("Resume 'A researched"), "{drawn}");
}

/// A refused key names the provider and the command, and a refusal that
/// is not about the key never sends the owner to check an address these
/// fixed endpoints do not have.
#[test]
fn a_provider_refusal_names_the_provider_and_never_a_server_address() {
    let mut app = Audiobook {
        stage: Stage::Narrate,
        ..Audiobook::default()
    };
    app.fail_task(kobo_sdk::TaskError::Unauthorized);
    let (_, advice) = app.trouble.clone().expect("a failure");
    assert!(advice.starts_with("ElevenLabs"), "{advice}");
    assert!(advice.contains("kobo secret set elevenlabs"), "{advice}");
    let mut app = Audiobook {
        stage: Stage::Write,
        ..Audiobook::default()
    };
    app.fail_task(kobo_sdk::TaskError::NotFound);
    let (_, advice) = app.trouble.clone().expect("a failure");
    assert!(advice.starts_with("OpenAI"), "{advice}");
    assert!(!advice.contains("address"), "{advice}");
}
