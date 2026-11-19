// Synthetic public sample data; no account, network access, or device capture.
#[test]
fn native_review_snapshots() {
    use kobo_sdk::{AppRunner, Command, StoreResult, TaskOutcome, CLARA_BW_METRICS};
    let mut runner = AppRunner::with_metrics(Readeck::default(), CLARA_BW_METRICS);
    runner.start();
    let commands = runner.store_result(StoreResult::Loaded {
        key: SERVER.into(),
        value: Some(b"https://readeck.example".to_vec()),
    });
    let task = commands.iter().find_map(|command| match command {
        Command::Spawn { task, .. } => Some(*task),
        _ => None,
    }).expect("inbox request");
    let commands = runner.task_outcome(task, TaskOutcome::Completed(br#"[
        {"id":"000000000000000001","title":"A quieter way to read","description":"Making time for the articles you saved.","has_article":true,"authors":["Sample Library"],"reading_time":4},
        {"id":"000000000000000002","title":"Walking the long way home","description":"Small discoveries on familiar paths.","has_article":true,"authors":["Sample Library"],"reading_time":7},
        {"id":"000000000000000003","title":"Notes from the garden","description":"What a season of growing can teach us.","has_article":true,"authors":["Sample Library"],"reading_time":5}
    ]"#.to_vec()));
    let screen = commands.iter().rev().find_map(|command| match command {
        Command::SetScreen(screen) => Some(screen),
        _ => None,
    }).expect("inbox screen");
    audit_capture(screen, &runner.context(), "inbox");
}
