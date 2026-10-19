use super::{decode_task_error, encode_task_error, Reader, TaskError};

/// Every variant, so that adding one without a tag fails here.
const EVERY: &[TaskError] = &[
    TaskError::Denied,
    TaskError::NoCredential,
    TaskError::Offline,
    TaskError::Unreachable,
    TaskError::TooLarge,
    TaskError::TimedOut,
    TaskError::NotFound,
    TaskError::Unauthorized,
    TaskError::RateLimited(37),
];

#[test]
fn every_task_error_survives_the_wire() {
    for error in EVERY {
        let mut bytes = Vec::new();
        encode_task_error(&mut bytes, *error);
        assert_eq!(decode_task_error(&mut Reader::new(&bytes)), Ok(*error));
    }
}

#[test]
fn no_two_task_errors_share_a_tag() {
    let mut tags: Vec<u8> = EVERY
        .iter()
        .map(|error| {
            let mut bytes = Vec::new();
            encode_task_error(&mut bytes, *error);
            bytes[0]
        })
        .collect();
    tags.sort_unstable();
    let count = tags.len();
    tags.dedup();
    assert_eq!(tags.len(), count, "two errors encode to the same tag");
}

#[test]
fn the_tags_that_were_already_on_the_wire_did_not_move() {
    // An app built before Offline existed is still talking to this daemon
    // over these numbers. Renumbering would make the two disagree about
    // what went wrong without either of them noticing.
    let tag = |error: TaskError| {
        let mut bytes = Vec::new();
        encode_task_error(&mut bytes, error);
        bytes[0]
    };
    assert_eq!(tag(TaskError::Denied), 0);
    assert_eq!(tag(TaskError::Unreachable), 1);
    assert_eq!(tag(TaskError::TooLarge), 2);
    assert_eq!(tag(TaskError::TimedOut), 3);
    assert_eq!(tag(TaskError::NotFound), 4);
    assert_eq!(tag(TaskError::Offline), 5);
    assert_eq!(tag(TaskError::NoCredential), 6);
    assert_eq!(tag(TaskError::Unauthorized), 7);
}

/// The two refusals have to stay distinguishable in words as well as on
/// the wire. A missing key blamed on the application sends whoever is
/// holding the reader looking in entirely the wrong place.
#[test]
fn a_missing_key_and_a_refused_permission_do_not_share_a_sentence() {
    let denied = TaskError::Denied.to_string();
    let absent = TaskError::NoCredential.to_string();
    assert_ne!(denied, absent);
    assert!(absent.contains("key"), "{absent}");
    assert!(!TaskError::NoCredential.worth_retrying());
}

#[test]
fn a_tag_from_the_future_is_refused_rather_than_guessed() {
    assert_eq!(
        decode_task_error(&mut Reader::new(&[255])),
        Err(super::ProtocolError::InvalidValue("task error"))
    );
}

#[test]
fn only_the_failures_that_could_pass_next_time_are_worth_retrying() {
    assert!(TaskError::Offline.worth_retrying());
    assert!(TaskError::Unreachable.worth_retrying());
    assert!(TaskError::TimedOut.worth_retrying());
    // A permission does not appear on the second ask, a response does not
    // shrink, and a 404 is a working host giving a real answer.
    assert!(!TaskError::Denied.worth_retrying());
    assert!(!TaskError::TooLarge.worth_retrying());
    assert!(!TaskError::NotFound.worth_retrying());
}

#[test]
fn the_two_network_failures_do_not_read_the_same() {
    // They ask opposite things of the person holding the reader: one is
    // answered by joining Wi-Fi, the other by waiting for a host.
    let offline = TaskError::Offline.to_string();
    let unreachable = TaskError::Unreachable.to_string();
    assert_ne!(offline, unreachable);
    assert!(offline.contains("not on a network"), "{offline}");
    assert!(unreachable.contains("host"), "{unreachable}");
}
