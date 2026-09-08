use super::*;

fn save(queue: &mut Outbox) -> Vec<u8> {
    let snapshot = queue.checkpoint().unwrap();
    assert!(queue.saved(snapshot.revision));
    snapshot.bytes
}

#[test]
fn no_remote_mutation_is_released_before_local_save_acknowledgement() {
    let mut queue = Outbox::default();
    queue
        .enqueue("article:1:read".into(), "true".into())
        .unwrap();
    let pending = queue.checkpoint().unwrap();
    assert!(queue.begin().is_none()); // Includes failed or missing store acknowledgements.
    assert!(queue.needs_save());
    queue.saved(pending.revision);
    assert!(queue.begin().is_some());
    assert!(queue.begin().is_none()); // One in flight.
}

#[test]
fn stale_store_ack_cannot_make_a_later_edit_durable() {
    let mut queue = Outbox::default();
    queue
        .enqueue("article:1:star".into(), "true".into())
        .unwrap();
    let older = queue.checkpoint().unwrap();
    queue
        .enqueue("article:1:star".into(), "false".into())
        .unwrap();
    assert!(queue.saved(older.revision));
    assert!(queue.begin().is_none());
    assert!(!queue.saved(u64::MAX));
    save(&mut queue);
    assert_eq!(queue.begin().unwrap().value, "false");
}

#[test]
fn a_crash_after_provider_ack_replays_the_same_idempotent_intent() {
    let mut queue = Outbox::default();
    queue
        .enqueue("article:1:read".into(), "true".into())
        .unwrap();
    let disk = save(&mut queue);
    let sent = queue.begin().unwrap();
    queue.finish(sent.sequence, Ok(())).unwrap();
    assert!(queue.entries().is_empty());
    assert!(queue.needs_save());
    let mut restarted = Outbox::restore(Some(&disk)).unwrap();
    assert_eq!(restarted.begin(), Some(sent));
    let committed_removal = save(&mut queue);
    assert!(Outbox::restore(Some(&committed_removal))
        .unwrap()
        .entries()
        .is_empty());
}

#[test]
fn a_new_value_does_not_rewrite_the_request_already_in_flight() {
    let mut queue = Outbox::default();
    let first = queue.enqueue("star:1".into(), "true".into()).unwrap();
    assert_eq!(
        queue.enqueue("star:1".into(), "false".into()).unwrap(),
        first
    );
    save(&mut queue);
    let sent = queue.begin().unwrap();
    let later = queue.enqueue("star:1".into(), "true".into()).unwrap();
    assert_ne!(first, later);
    assert_eq!(queue.entries()[0].value, "false");
    assert_eq!(queue.entries()[1].value, "true");
    assert!(!queue.finish(later, Ok(())).unwrap()); // A response for other work cannot remove this one.
    assert!(queue.finish(sent.sequence, Ok(())).unwrap());
    assert!(!queue.finish(sent.sequence, Ok(())).unwrap());
    assert!(queue.begin().is_none());
    save(&mut queue);
    assert_eq!(queue.begin().unwrap().sequence, later);
}

#[test]
fn retries_and_conflicts_survive_restart_without_automatic_resubmission() {
    for failure in [
        Failure::Retry("Service unavailable".into()),
        Failure::Conflict("Changed on server".into()),
    ] {
        let mut queue = Outbox::default();
        queue.enqueue("read:1".into(), "true".into()).unwrap();
        save(&mut queue);
        let sent = queue.begin().unwrap();
        queue.finish(sent.sequence, Err(failure.clone())).unwrap();
        let disk = save(&mut queue);
        let mut restarted = Outbox::restore(Some(&disk)).unwrap();
        assert_eq!(restarted.failure(), Some(&failure));
        assert!(restarted.begin().is_none());
        restarted.retry().unwrap();
        assert!(restarted.begin().is_none());
        save(&mut restarted);
        assert_eq!(restarted.begin(), Some(sent));
    }
}

#[test]
fn full_and_invalid_enqueues_leave_saved_work_unchanged() {
    let mut queue = Outbox::default();
    queue.enqueue("valid".into(), "pending".into()).unwrap();
    save(&mut queue);
    let before = queue.clone();
    assert_eq!(
        queue.enqueue(String::new(), "x".into()),
        Err(Error::InvalidKey)
    );
    assert_eq!(
        queue.enqueue("other".into(), "x".repeat(MAX_VALUE_BYTES + 1)),
        Err(Error::Full)
    );
    assert_eq!(queue, before);
    for i in 1..MAX_MUTATIONS {
        queue.enqueue(format!("record-{i}"), "x".into()).unwrap();
    }
    assert_eq!(
        queue.enqueue("one-too-many".into(), "x".into()),
        Err(Error::Full)
    );
    assert_eq!(queue.entries().len(), MAX_MUTATIONS);
}

#[test]
fn corrupt_and_future_snapshots_are_distinct_from_an_expected_empty_store() {
    assert_eq!(Outbox::restore(None).unwrap(), Outbox::default());
    assert_eq!(Outbox::restore(Some(b"garbage")), Err(Error::Corrupt));
    assert_eq!(
        Outbox::restore(Some(b"{\"version\":2}")),
        Err(Error::UnsupportedVersion)
    );
    let mut queue = Outbox::default();
    queue
        .enqueue(
            "note:日本語".into(),
            "A \"quoted\" value\nwith a second line".into(),
        )
        .unwrap();
    let snapshot = queue.checkpoint().unwrap();
    let restored = Outbox::restore(Some(&snapshot.bytes)).unwrap();
    assert_eq!(restored.entries(), queue.entries());
    assert!(!restored.needs_save());
    for size in 0..snapshot.bytes.len() {
        assert!(Outbox::restore(Some(&snapshot.bytes[..size])).is_err());
    }
}

#[test]
fn queue_limit_reserves_enough_space_to_persist_a_failure() {
    let mut queue = Outbox::default();
    for i in 0..MAX_MUTATIONS {
        if queue
            .enqueue(format!("record-{i}"), "x".repeat(MAX_VALUE_BYTES))
            .is_err()
        {
            break;
        }
    }
    save(&mut queue);
    let sent = queue.begin().unwrap();
    queue
        .finish(sent.sequence, Err(Failure::Retry("\0".repeat(512))))
        .unwrap();
    let snapshot = queue.checkpoint().unwrap();
    assert!(snapshot.bytes.len() <= MAX_CHECKPOINT_BYTES);
    assert!(Outbox::restore(Some(&snapshot.bytes))
        .unwrap()
        .failure()
        .is_some());
}
