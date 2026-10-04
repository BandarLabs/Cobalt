use super::*;
use kobo_sdk::{AppRunner, Command, Node, StoreError, StoreRequest, TaskError, CLARA_BW_METRICS};

const ORIGIN: &str = "https://read.example";
const ID: &str = "0123456789ABCDEFGH";
fn last_screen(commands: &[Command]) -> &Screen {
    commands
        .iter()
        .rev()
        .find_map(|c| {
            if let Command::SetScreen(s) = c {
                Some(s)
            } else {
                None
            }
        })
        .expect("screen")
}
fn key(screen: &Screen, label: &str) -> ActionId {
    screen
        .nodes
        .iter()
        .find_map(|node| match node {
            Node::Grid { cells, .. } => cells
                .iter()
                .find_map(|c| (c.label == label).then_some(c.action)),
            Node::Button {
                action,
                label: text,
                ..
            } => (text == label).then_some(*action),
            _ => None,
        })
        .expect("visible action")
}
fn spawned(commands: &[Command]) -> (TaskId, &Task) {
    commands
        .iter()
        .find_map(|c| {
            if let Command::Spawn { task, work } = c {
                Some((*task, work))
            } else {
                None
            }
        })
        .expect("spawn")
}
fn no_spawn(commands: &[Command]) {
    assert!(!commands.iter().any(|c| matches!(c, Command::Spawn { .. })));
}
fn batch(count: usize, readable: bool) -> Vec<u8> {
    format!("[{}]", (0..count).map(|i| format!(r#"{{"id":"{i:018}","title":"Article {i}","has_article":{readable},"authors":["A Writer"],"reading_time":4}}"#)).collect::<Vec<_>>().join(",")).into_bytes()
}
fn configured() -> (AppRunner<Readeck>, Vec<Command>) {
    let mut runner = AppRunner::new(Readeck::default());
    no_spawn(&runner.start());
    let commands = runner.store_result(StoreResult::Loaded {
        key: SERVER.into(),
        value: Some(ORIGIN.as_bytes().to_vec()),
    });
    runner.store_result(StoreResult::Loaded {
        key: SCALE.into(),
        value: None,
    });
    (runner, commands)
}
fn loaded() -> AppRunner<Readeck> {
    let (mut runner, commands) = configured();
    runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(batch(3, true)));
    runner
}
fn read(runner: &mut AppRunner<Readeck>) -> Vec<Command> {
    let commands = runner.action(action_id("article-0"));
    no_spawn(&commands);
    let id = runner.app_mut().open.clone().unwrap();
    let commands = runner.store_result(StoreResult::Loaded {
        key: place_key(ORIGIN, &id),
        value: None,
    });
    let (task, work) = spawned(&commands);
    assert!(
        matches!(work, Task::Fetch { credential: Some(c), .. } if c == &Credential::bearer("readeck"))
    );
    runner.task_outcome(
        task,
        TaskOutcome::Completed(
            format!(
                "<article>{}</article>",
                "<p>A paragraph for a long readable article, with words to wrap across lines.</p>"
                    .repeat(100)
            )
            .into_bytes(),
        ),
    )
}
fn search(runner: &mut AppRunner<Readeck>, query: &str) -> Vec<Command> {
    let commands = runner.action(action_id("search"));
    let submit = key(last_screen(&commands), "Search");
    runner.app_mut().keyboard = Keyboard::with_text(query);
    runner.action(submit)
}
fn enter_token(runner: &mut AppRunner<Readeck>, origin: &str) -> Vec<Command> {
    let commands = runner.action(action_id("settings"));
    runner.app_mut().keyboard = Keyboard::with_text(origin);
    let commands = runner.action(key(last_screen(&commands), "Next"));
    let commands = runner.action(key(last_screen(&commands), "Enter account key"));
    // Type via the public masked keyboard; the token never appears on a screen.
    let commands = runner.action(key(last_screen(&commands), "a"));
    assert!(!format!("{:?}", last_screen(&commands)).contains("super-secret"));
    runner.action(key(last_screen(&commands), "Save"))
}
#[test]
fn computer_saved_token_saves_only_address_before_fetching() {
    let mut runner = loaded();
    runner.action(action_id("settings"));
    runner.app_mut().keyboard = Keyboard::with_text("https://second.example/library/");
    let commands = runner.action(action_id("saved-token"));
    no_spawn(&commands);
    assert!(!commands
        .iter()
        .any(|c| matches!(c, Command::Device(DeviceRequest::SetServerSecret { .. }))));
    assert!(commands.iter().any(|c| matches!(c, Command::Store(StoreRequest::Save { key, value }) if key == SERVER && value == b"https://second.example/library")));
    let commands = runner.store_result(StoreResult::Saved { key: SERVER.into() });
    assert!(
        matches!(spawned(&commands).1, Task::Fetch { url, credential: Some(c), .. } if url.starts_with("https://second.example/library/") && c == &Credential::bearer("readeck"))
    );
}

#[test]
fn saved_token_does_not_bypass_server_validation() {
    for server in [
        "http://unsafe.example",
        "https:///path",
        "https://:443/path",
        "https://read.example:bad/path",
    ] {
        let mut runner = loaded();
        runner.action(action_id("settings"));
        runner.app_mut().keyboard = Keyboard::with_text(server);
        let commands = runner.action(action_id("saved-token"));
        no_spawn(&commands);
        assert!(
            !commands
                .iter()
                .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))),
            "{server}"
        );
        assert!(runner.app_mut().problem.is_some(), "{server}");
    }
}

#[test]
fn token_and_server_save_acknowledgements_precede_any_network_request() {
    let mut runner = loaded();
    let commands = enter_token(&mut runner, "https://second.example/library/");
    no_spawn(&commands);
    assert!(commands.iter().any(
        |c| matches!(c, Command::Device(DeviceRequest::SetServerSecret { name, server, .. })
        if name == "readeck" && server == "https://second.example/library")
    ));
    assert!(!commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
    let commands = runner.device_result(DeviceResult::Done);
    no_spawn(&commands);
    assert!(commands.iter().any(
        |c| matches!(c, Command::Store(StoreRequest::Save { key, value })
        if key == SERVER && value == b"https://second.example/library")
    ));
    assert_eq!(runner.app_mut().server.as_deref(), Some(ORIGIN));
    let commands = runner.store_result(StoreResult::Saved { key: SERVER.into() });
    assert!(
        matches!(spawned(&commands).1, Task::Fetch { url, .. } if url.starts_with("https://second.example/library/api/"))
    );
    assert_eq!(
        runner.app_mut().server.as_deref(),
        Some("https://second.example/library")
    );
}
#[test]
fn failed_server_save_does_not_claim_connection_and_can_retry() {
    let mut runner = loaded();
    enter_token(&mut runner, "https://second.example");
    runner.device_result(DeviceResult::Done);
    let commands = runner.store_result(StoreResult::Denied(StoreError::Unwritable));
    no_spawn(&commands);
    assert!(runner.app_mut().pending_server.is_some());
    runner.action(action_id("dismiss"));
    let commands = runner.action(action_id("save-server"));
    assert!(commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { key, .. }) if key == SERVER)));
    let commands = runner.store_result(StoreResult::Saved { key: SERVER.into() });
    assert!(
        matches!(spawned(&commands).1, Task::Fetch { url, .. } if url.starts_with("https://second.example/"))
    );
}
#[test]
fn saved_token_address_failure_never_claims_token_was_saved() {
    let mut runner = loaded();
    runner.action(action_id("settings"));
    runner.app_mut().keyboard = Keyboard::with_text("https://second.example");
    runner.action(action_id("saved-token"));
    let commands = runner.store_result(StoreResult::Denied(StoreError::Unwritable));
    no_spawn(&commands);
    assert_eq!(
        runner.app_mut().problem.as_deref(),
        Some("The server address could not be saved. Continue to retry.")
    );
    assert_eq!(runner.app_mut().server.as_deref(), Some(ORIGIN));
    runner.action(action_id("dismiss"));
    let commands = runner.action(action_id("save-server"));
    assert!(commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { key, .. }) if key == SERVER)));
    let commands = runner.store_result(StoreResult::Saved { key: SERVER.into() });
    assert!(
        matches!(spawned(&commands).1, Task::Fetch { url, .. } if url.starts_with("https://second.example/"))
    );
}

#[test]
fn server_validation_rejects_ambiguous_or_untrusted_addresses() {
    for server in ["https://read.example", "https://read.example:8443/library"] {
        assert!(valid_server(server));
    }
    for server in [
        "http://read.example",
        "https:///path",
        "https://:443/path",
        "https://read.example:bad/path",
        "https://a@read.example",
        "https://read.example?a=b",
        "https://read.example/#x",
        "https://read.example/../admin",
        "https://read.example/%2e%2e/admin",
        "https://read.example/\\admin",
    ] {
        assert!(!valid_server(server), "{server}");
    }
}
#[test]
fn restarts_load_address_but_never_load_a_token_from_app_state() {
    let (runner, commands) = configured();
    assert!(
        matches!(spawned(&commands).1, Task::Fetch { url, credential: Some(c), .. }
        if url == &format!("{ORIGIN}{LIST_PATH}") && c.secret == "readeck")
    );
    drop(runner);
}
#[test]
fn whole_library_search_encodes_query_and_counts_unreadable_entries() {
    let mut runner = loaded();
    let commands = search(&mut runner, " café & / ");
    assert!(matches!(spawned(&commands).1, Task::Fetch { url, .. }
        if url.ends_with("offset=0&sort=-created&search=caf%C3%A9%20%26%20%2F")));
    runner.task_outcome(
        spawned(&commands).0,
        TaskOutcome::Completed(batch(50, false)),
    );
    let commands = runner.action(action_id("next"));
    assert!(matches!(spawned(&commands).1, Task::Fetch { url, .. } if url.contains("offset=50")));
    runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(batch(1, true)));
    assert_eq!(runner.app_mut().search.as_ref().unwrap().offset, 50);
    let commands = runner.action(action_id("previous"));
    assert!(matches!(spawned(&commands).1, Task::Fetch { url, .. } if url.contains("offset=0")));
}
#[test]
fn cancelled_search_cannot_overwrite_restored_inbox() {
    let mut runner = loaded();
    let commands = search(&mut runner, "hello");
    let task = spawned(&commands).0;
    runner.action(ActionId::BACK);
    runner.task_outcome(task, TaskOutcome::Completed(batch(10, true)));
    assert_eq!(runner.app_mut().bookmarks.len(), 3);
    assert!(runner.app_mut().search.is_none());
}
#[test]
fn malformed_next_batch_preserves_results_and_offset() {
    let mut runner = loaded();
    let commands = search(&mut runner, "hello");
    runner.task_outcome(
        spawned(&commands).0,
        TaskOutcome::Completed(batch(50, true)),
    );
    let mut commands = Vec::new();
    for _ in 0..100 {
        commands = runner.action(action_id("next"));
        if commands.iter().any(|c| matches!(c, Command::Spawn { .. })) {
            break;
        }
    }
    runner.task_outcome(
        spawned(&commands).0,
        TaskOutcome::Completed(b"not json".to_vec()),
    );
    assert_eq!(runner.app_mut().bookmarks.len(), 50);
    assert_eq!(runner.app_mut().search.as_ref().unwrap().offset, 0);
    assert!(runner.app_mut().problem.is_some());
}
#[test]
fn mutations_wait_for_server_and_failures_keep_the_article() {
    let mut runner = loaded();
    read(&mut runner);
    runner.action(action_id(kobo_read::action::CONTROLS));
    let commands = runner.action(action_id("archive"));
    let (task, work) = spawned(&commands);
    assert!(
        matches!(work, Task::Update { method: UpdateMethod::Patch, body, .. } if body == r#"{"is_archived":true}"#)
    );
    assert_eq!(runner.app_mut().bookmarks.len(), 3);
    no_spawn(&runner.action(action_id("mark-read")));
    runner.task_outcome(task, TaskOutcome::Failed(TaskError::Unauthorized));
    assert_eq!(runner.app_mut().bookmarks.len(), 3);
    runner.action(action_id("dismiss"));
    let commands = runner.action(action_id("archive"));
    runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(Vec::new()));
    assert_eq!(runner.app_mut().bookmarks.len(), 2);
    assert_eq!(runner.app_mut().view, View::Inbox);
}
#[test]
fn deletion_requires_confirmation_and_favorite_is_independent_of_inbox_membership() {
    let mut runner = loaded();
    read(&mut runner);
    runner.action(action_id(kobo_read::action::CONTROLS));
    let commands = runner.action(action_id("favorite"));
    assert!(!runner.app_mut().bookmarks[0].marked);
    runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(Vec::new()));
    assert!(runner.app_mut().bookmarks[0].marked);
    assert_eq!(runner.app_mut().bookmarks.len(), 3);
    runner.action(action_id(kobo_read::action::CONTROLS));
    no_spawn(&runner.action(action_id("delete")));
    no_spawn(&runner.action(action_id("keep-article")));
    runner.action(action_id("delete"));
    let commands = runner.action(action_id("confirm-delete"));
    runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(Vec::new()));
    assert_eq!(runner.app_mut().bookmarks.len(), 2);
}
#[test]
fn reader_has_app_controls_and_no_unsynced_local_hold_action() {
    let mut runner = loaded();
    let commands = read(&mut runner);
    assert!(last_screen(&commands).hold.is_none());
    let commands = runner.action(action_id(kobo_read::action::CONTROLS));
    assert_eq!(key(last_screen(&commands), "Archive"), action_id("archive"));
    runner.action(ActionId::BACK);
    assert_eq!(runner.app_mut().view, View::Reading);
    runner.action(ActionId::BACK);
    assert_eq!(runner.app_mut().view, View::Inbox);
}
#[test]
fn reading_position_is_saved_and_scoped_to_exact_server_and_case_sensitive_id() {
    let mut runner = loaded();
    read(&mut runner);
    let commands = runner.action(action_id(kobo_read::action::FORWARD));
    let saved = commands
        .iter()
        .find_map(|c| match c {
            Command::Store(StoreRequest::Save { key, value }) if key.starts_with("p.") => {
                Some(value.clone())
            }
            _ => None,
        })
        .expect("saved position");
    let memory = decode_place(ORIGIN, &saved).unwrap();
    assert_eq!(memory, runner.app_mut().book.memory().unwrap().clone());
    assert!(decode_place("https://other.example", &saved).is_none());
    assert_ne!(place_key(ORIGIN, ID), place_key(ORIGIN, &ID.to_lowercase()));
    assert_ne!(
        place_key(ORIGIN, ID),
        place_key("https://other.example", ID)
    );
}
#[test]
fn saved_position_and_reading_size_are_restored_after_restart() {
    let mut runner = loaded();
    read(&mut runner);
    runner.action(action_id(kobo_read::action::FORWARD));
    runner.action(action_id(kobo_read::action::LARGER));
    let memory = runner.app_mut().book.memory().unwrap().clone();
    let id = runner.app_mut().open.clone().unwrap();
    let bytes = encode_place(ORIGIN, &memory);
    let (mut restarted, commands) = configured();
    restarted.app_mut().scale = memory.scale;
    restarted.task_outcome(spawned(&commands).0, TaskOutcome::Completed(batch(3, true)));
    restarted.action(action_id("article-0"));
    let commands = restarted.store_result(StoreResult::Loaded {
        key: place_key(ORIGIN, &id),
        value: Some(bytes),
    });
    restarted.task_outcome(
        spawned(&commands).0,
        TaskOutcome::Completed(
            format!(
                "<article>{}</article>",
                "<p>A paragraph for a long readable article, with words to wrap across lines.</p>"
                    .repeat(100)
            )
            .into_bytes(),
        ),
    );
    assert_eq!(restarted.app_mut().book.memory().unwrap(), &memory);
    for id in ["a".repeat(18), "Z".repeat(22)] {
        assert!(kobo_protocol::is_valid_key(&place_key(ORIGIN, &id)));
    }
}

#[test]
fn rejected_token_does_not_save_address_or_fetch() {
    let mut runner = loaded();
    enter_token(&mut runner, "https://second.example");
    let commands = runner.device_result(DeviceResult::Denied(kobo_sdk::DenyReason::PolicyRejected));
    no_spawn(&commands);
    assert!(!commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
    assert!(runner.app_mut().account.is_open());
    assert_eq!(runner.app_mut().server.as_deref(), Some(ORIGIN));
}

#[test]
fn failed_position_load_never_fetches_or_overwrites_the_saved_position() {
    let mut runner = loaded();
    runner.action(action_id("article-0"));
    let commands = runner.store_result(StoreResult::Denied(StoreError::Unwritable));
    no_spawn(&commands);
    assert!(!commands
        .iter()
        .any(|c| matches!(c, Command::Store(StoreRequest::Save { .. }))));
    assert!(runner.app_mut().problem.is_some());
}

#[test]
fn ordinary_images_fetch_without_token_and_other_origins_are_not_requested() {
    let mut runner = loaded();
    runner.action(action_id("article-0"));
    let id = runner.app_mut().open.clone().unwrap();
    let commands = runner.store_result(StoreResult::Loaded {
        key: place_key(ORIGIN, &id),
        value: None,
    });
    let commands = runner.task_outcome(spawned(&commands).0, TaskOutcome::Completed(
        br#"<p>An illustration.</p><img src="/pictures/one.png"/><img src="https://other.example/two.png"/>"#.to_vec()));
    let (_, work) = spawned(&commands);
    assert!(
        matches!(work, Task::Fetch { url, credential: None, .. } if url == "https://read.example/pictures/one.png")
    );
    assert!(!commands.iter().any(|c| matches!(c, Command::Spawn { work: Task::Fetch { url, .. }, .. } if url.contains("other.example"))));
}
#[test]
fn screens_fit_clara_and_elipsa() {
    use kobo_sdk::{Chrome, DiagnosticSeverity, DisplayMetrics};
    for metrics in [
        CLARA_BW_METRICS,
        DisplayMetrics {
            width: 1404,
            height: 1872,
            pixels_per_inch: 227,
            ..CLARA_BW_METRICS
        },
    ] {
        let mut runner = AppRunner::with_metrics(Readeck::default(), metrics);
        runner.start();
        let commands = runner.store_result(StoreResult::Loaded {
            key: SERVER.into(),
            value: Some(ORIGIN.as_bytes().to_vec()),
        });
        runner.store_result(StoreResult::Loaded {
            key: SCALE.into(),
            value: None,
        });
        let commands = runner.task_outcome(
            spawned(&commands).0,
            TaskOutcome::Completed(batch(50, true)),
        );
        let mut screens = vec![last_screen(&commands).clone()];
        screens.push(last_screen(&read(&mut runner)).clone());
        screens.push(last_screen(&runner.action(action_id(kobo_read::action::CONTROLS))).clone());
        runner.action(ActionId::BACK);
        runner.action(ActionId::BACK);
        screens.push(last_screen(&runner.action(action_id("settings"))).clone());
        for screen in screens {
            let errors: Vec<_> = screen
                .diagnostics(&metrics, &Chrome::measuring(false))
                .issues
                .into_iter()
                .filter(|i| i.severity == DiagnosticSeverity::Error)
                .collect();
            assert!(errors.is_empty(), "layout: {errors:?}");
        }
    }
}
