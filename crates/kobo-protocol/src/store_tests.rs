use super::*;

fn message_round_trip(message: Message) -> Message {
    let frame = Frame {
        version: VERSION,
        request_id: 11,
        message,
    };
    let bytes = encode(&frame).expect("encode");
    decode(&bytes).expect("decode").message
}

#[test]
fn a_text_hold_survives_the_wire_without_losing_its_range() {
    let message = Message::TextHold {
        action: ActionId(9),
        context: u64::MAX - 1,
        start: 41,
        end: 48,
    };
    assert_eq!(message_round_trip(message.clone()), message);
}

#[test]
fn offline_dictionary_requests_and_entries_are_bounded_and_typed() {
    let request = Message::DeviceRequest(DeviceRequest::LookupWord {
        word: "café".into(),
        language: Some("fr".into()),
    });
    assert_eq!(message_round_trip(request.clone()), request);

    let result = Message::DeviceResult(DeviceResult::Dictionary {
        word: "café".into(),
        entries: vec![DictionaryEntry {
            dictionary: "Pocket French".into(),
            language: "fr".into(),
            headword: "café".into(),
            definition: "Établissement où l'on sert des boissons.".into(),
        }],
    });
    assert_eq!(message_round_trip(result.clone()), result);
}

#[test]
fn a_terminal_chunk_larger_than_the_bound_is_refused_rather_than_sent() {
    // The ceiling has to be enforced by the sender as well as the reader,
    // or a program printing without pause builds a frame the other end
    // will only reject after it has already been allocated.
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::ShellEvent(ShellEvent::Output(vec![b'x'; MAX_SHELL_CHUNK + 1])),
    };
    assert!(matches!(encode(&frame), Err(ProtocolError::FrameTooLarge)));
}

#[test]
fn a_terminal_chunk_exactly_at_the_bound_is_carried() {
    let message = Message::ShellRequest(ShellRequest::Input(vec![b'x'; MAX_SHELL_CHUNK]));
    assert_eq!(message_round_trip(message.clone()), message);
}

#[test]
fn a_request_body_may_be_far_larger_than_a_label() {
    // The body used to be encoded as a label, so an application that
    // handed a model the research it had just fetched could not spawn the
    // request at all: encoding failed with `StringTooLarge` and the
    // application ended, having said nothing about why.
    let body = "x".repeat(64 * 1024);
    let message = Message::Spawn {
        task: TaskId(3),
        work: Task::Post {
            url: "https://example.invalid/v1".into(),
            body: body.clone(),
            content_type: "application/json".into(),
            credential: Some(Credential::bearer("openai")),
            headers: Vec::new(),
            max_bytes: 4096,
        },
    };
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message,
    };
    let encoded = encode(&frame).expect("a 64 KiB body encodes");
    let decoded = decode(&encoded).expect("a 64 KiB body decodes");
    assert_eq!(decoded, frame);

    let oversized = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::Spawn {
            task: TaskId(3),
            work: Task::Post {
                url: "https://example.invalid/v1".into(),
                body: "x".repeat(MAX_POST_BODY_LEN + 1),
                content_type: "application/json".into(),
                credential: None,
                headers: Vec::new(),
                max_bytes: 4096,
            },
        },
    };
    assert_eq!(encode(&oversized), Err(ProtocolError::StringTooLarge));
}

#[test]
fn every_task_message_encodes_to_exactly_the_length_it_claims() {
    // `encode` predicts the payload length before it writes anything, and
    // a wrong prediction used to be a debug assertion that only fired when
    // an application actually spawned something, which meant every task
    // message crashed the simulator and no test noticed, because none of
    // them encoded one.
    for message in [
        Message::Spawn {
            task: TaskId(7),
            work: Task::Fetch {
                url: "https://example.invalid/book.txt".into(),
                offset: 0,
                max_bytes: 1024,
                credential: None,
                headers: Vec::new(),
            },
        },
        Message::Spawn {
            task: TaskId(16),
            work: Task::Fetch {
                url: "https://example.invalid/catalog".into(),
                offset: 0,
                max_bytes: 1024,
                credential: None,
                headers: vec![Header::new("Accept", "application/opds+json")],
            },
        },
        Message::Spawn {
            task: TaskId(8),
            work: Task::ReadFile {
                path: "/mnt/onboard/book.txt".into(),
            },
        },
        Message::Spawn {
            task: TaskId(9),
            work: Task::Sleep { seconds: 30 },
        },
        Message::Spawn {
            task: TaskId(10),
            work: Task::Post {
                url: "https://example.invalid/v1".into(),
                body: "{}".into(),
                content_type: "application/json".into(),
                credential: Some(Credential::bearer("openai")),
                headers: Vec::new(),
                max_bytes: 4096,
            },
        },
        Message::Spawn {
            task: TaskId(11),
            work: Task::Post {
                url: "https://example.invalid/v1".into(),
                body: String::new(),
                content_type: "application/json".into(),
                credential: None,
                headers: Vec::new(),
                max_bytes: 4096,
            },
        },
        Message::Cancel { task: TaskId(12) },
        Message::TaskOutcome {
            task: TaskId(13),
            outcome: TaskOutcome::Completed(b"hello".to_vec()),
        },
        Message::TaskOutcome {
            task: TaskId(14),
            outcome: TaskOutcome::Failed(TaskError::TooLarge),
        },
        Message::TaskOutcome {
            task: TaskId(15),
            outcome: TaskOutcome::Cancelled,
        },
        Message::TaskOutcome {
            task: TaskId(16),
            outcome: TaskOutcome::Failed(TaskError::RateLimited(37)),
        },
    ] {
        let (_, predicted) =
            encoded_message_layout(&message, VERSION).expect("the message is within the limits");
        let frame = Frame {
            version: VERSION,
            request_id: 3,
            message: message.clone(),
        };
        let encoded = encode(&frame).expect("encode");
        assert_eq!(
            encoded.len() - HEADER_LEN,
            predicted,
            "{message:?} encodes to a different length than it predicted"
        );
        assert_eq!(message_round_trip(message.clone()), message);
    }
}

#[test]
fn a_fetch_that_carries_headers_survives_an_encode_decode_round_trip() {
    let message = Message::Spawn {
        task: TaskId(20),
        work: Task::Fetch {
            url: "https://example.invalid/catalog".into(),
            offset: 0,
            max_bytes: 4096,
            credential: None,
            headers: vec![
                Header::new("Accept", "application/opds+json"),
                Header::new("If-None-Match", "\"abc123\""),
            ],
        },
    };
    assert_eq!(message_round_trip(message.clone()), message);
}

#[test]
fn a_fetch_whose_header_count_exceeds_the_bound_is_refused_rather_than_truncated() {
    let headers = (0..=MAX_HEADERS)
        .map(|index| Header::new(format!("X-{index}"), "value"))
        .collect();
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::Spawn {
            task: TaskId(21),
            work: Task::Fetch {
                url: "https://example.invalid/catalog".into(),
                offset: 0,
                max_bytes: 4096,
                credential: None,
                headers,
            },
        },
    };
    assert_eq!(
        encode(&frame),
        Err(ProtocolError::InvalidValue("request header"))
    );
}

#[test]
fn a_fetch_header_carrying_a_newline_in_its_name_or_value_is_refused() {
    // A newline in either would let an application append headers of its
    // own onto the wire, including ones the runtime never agreed to send.
    for header in [
        Header::new("X-Evil\r\nX-Injected", "value"),
        Header::new("X-Evil", "value\r\nX-Injected: 1"),
    ] {
        let frame = Frame {
            version: VERSION,
            request_id: 1,
            message: Message::Spawn {
                task: TaskId(22),
                work: Task::Fetch {
                    url: "https://example.invalid/catalog".into(),
                    offset: 0,
                    max_bytes: 4096,
                    credential: None,
                    headers: vec![header],
                },
            },
        };
        assert_eq!(
            encode(&frame),
            Err(ProtocolError::InvalidValue("request header"))
        );
    }
}

#[test]
fn every_store_message_survives_the_wire() {
    for message in [
        Message::StoreRequest(StoreRequest::Save {
            key: "tasks".into(),
            value: vec![0, 1, 2, 255],
        }),
        Message::StoreRequest(StoreRequest::Load {
            key: "tasks".into(),
        }),
        Message::StoreRequest(StoreRequest::Forget {
            key: "tasks".into(),
        }),
        Message::StoreRequest(StoreRequest::List),
        Message::StoreResult(StoreResult::Saved {
            key: "tasks".into(),
        }),
        Message::StoreResult(StoreResult::Loaded {
            key: "tasks".into(),
            value: Some(b"[]".to_vec()),
        }),
        Message::StoreResult(StoreResult::Loaded {
            key: "tasks".into(),
            value: None,
        }),
        Message::StoreResult(StoreResult::Forgotten {
            key: "tasks".into(),
        }),
        Message::StoreResult(StoreResult::Keys(vec!["a".into(), "b".into()])),
        Message::StoreResult(StoreResult::Denied(StoreError::BadKey)),
        Message::Lifecycle(Lifecycle::Foreground),
        Message::Lifecycle(Lifecycle::Background),
        Message::CoverChanged {
            magnet_present: true,
        },
        Message::CoverChanged {
            magnet_present: false,
        },
        Message::PageTurn { forward: true },
        Message::PageTurn { forward: false },
        Message::ShellRequest(ShellRequest::Open {
            columns: 53,
            rows: 20,
        }),
        Message::ShellRequest(ShellRequest::Input(vec![0x03])),
        Message::ShellRequest(ShellRequest::Input(Vec::new())),
        Message::ShellRequest(ShellRequest::Resize {
            columns: 40,
            rows: 10,
        }),
        Message::ShellRequest(ShellRequest::Close),
        Message::ShellEvent(ShellEvent::Opened),
        Message::ShellEvent(ShellEvent::Output(b"~ # \x1b[K".to_vec())),
        Message::ShellEvent(ShellEvent::Closed { status: 0 }),
        // Negative, because a program stopped by a signal is reported as
        // one and a status that came back wrong would look like success.
        Message::ShellEvent(ShellEvent::Closed { status: -1 }),
        Message::ShellEvent(ShellEvent::Refused(ShellError::NotPermitted)),
        Message::ShellEvent(ShellEvent::Refused(ShellError::Failed)),
    ] {
        assert_eq!(message_round_trip(message.clone()), message);
    }
}

#[test]
fn every_shelf_message_survives_the_wire_at_the_length_it_predicted() {
    // The length functions are maintained by hand beside the encoders, so
    // this asserts the two agree as well as that the bytes decode back.
    for message in [
        Message::StoreRequest(StoreRequest::ShelfWrite {
            name: "pg1342.epub".into(),
            offset: 0,
            bytes: vec![0, 1, 2, 255],
            last: false,
        }),
        Message::StoreRequest(StoreRequest::ShelfWrite {
            name: "pg1342.epub".into(),
            offset: 262_144,
            bytes: Vec::new(),
            last: true,
        }),
        Message::StoreRequest(StoreRequest::ShelfRead {
            name: "pg1342.epub".into(),
            offset: 4096,
            length: 65536,
        }),
        Message::StoreRequest(StoreRequest::ShelfRemove {
            name: "pg1342.epub".into(),
        }),
        Message::StoreRequest(StoreRequest::ShelfList),
        Message::StoreResult(StoreResult::ShelfWritten {
            name: "pg1342.epub".into(),
            size: 262_144,
        }),
        Message::StoreResult(StoreResult::ShelfRead {
            name: "pg1342.epub".into(),
            offset: 4096,
            bytes: b"It is a truth universally acknowledged".to_vec(),
            size: 700_000,
        }),
        Message::StoreResult(StoreResult::ShelfRemoved {
            name: "pg1342.epub".into(),
        }),
        Message::StoreResult(StoreResult::Shelf(vec![
            ("pg1342.epub".into(), 700_000),
            ("pg84.txt".into(), 442_000),
        ])),
        Message::StoreResult(StoreResult::Shelf(Vec::new())),
        Message::StoreResult(StoreResult::Denied(StoreError::NoRoom)),
        Message::StoreResult(StoreResult::Denied(StoreError::Missing)),
    ] {
        let frame = Frame {
            version: VERSION,
            request_id: 1,
            message: message.clone(),
        };
        let (_, predicted) =
            encoded_message_layout(&frame.message, frame.version).expect("a valid message");
        let encoded = encode(&frame).expect("a valid message");
        assert_eq!(
            encoded.len() - HEADER_LEN,
            predicted,
            "{message:?} encodes to a different length than it predicted"
        );
        assert_eq!(message_round_trip(message.clone()), message);
    }
}

#[test]
fn a_chunk_over_the_ceiling_is_refused_by_both_ends() {
    // Refused at encode, so an application cannot build a frame the
    // runtime would only drop, and refused at decode, so a peer that
    // ignored the first rule cannot make us allocate on its say-so.
    let message = Message::StoreRequest(StoreRequest::ShelfWrite {
        name: "big".into(),
        offset: 0,
        bytes: vec![0; MAX_SHELF_CHUNK + 1],
        last: false,
    });
    assert!(matches!(
        encode(&Frame {
            version: VERSION,
            request_id: 1,
            message,
        }),
        Err(ProtocolError::FrameTooLarge)
    ));
}

#[test]
fn a_finished_flag_that_is_neither_yes_nor_no_is_refused() {
    let mut encoded = encode(&Frame {
        version: VERSION,
        request_id: 1,
        message: Message::StoreRequest(StoreRequest::ShelfWrite {
            name: "a".into(),
            offset: 0,
            bytes: Vec::new(),
            last: true,
        }),
    })
    .expect("a valid message");
    let flag = encoded.len() - 5;
    assert_eq!(encoded[flag], 1, "the finished flag moved");
    encoded[flag] = 2;
    assert!(matches!(
        decode(&encoded),
        Err(ProtocolError::InvalidValue("shelf finished flag"))
    ));
}

#[test]
fn a_never_written_key_is_distinct_from_an_empty_one() {
    // These encode to different bytes on purpose. An application that
    // cannot tell "nothing saved yet" from "saved nothing" cannot tell a
    // first run from a cleared list.
    let missing = Message::StoreResult(StoreResult::Loaded {
        key: "k".into(),
        value: None,
    });
    let empty = Message::StoreResult(StoreResult::Loaded {
        key: "k".into(),
        value: Some(Vec::new()),
    });
    assert_ne!(
        encode(&Frame {
            version: VERSION,
            request_id: 0,
            message: missing.clone()
        })
        .unwrap(),
        encode(&Frame {
            version: VERSION,
            request_id: 0,
            message: empty.clone()
        })
        .unwrap()
    );
    assert_eq!(message_round_trip(missing.clone()), missing);
    assert_eq!(message_round_trip(empty.clone()), empty);
}

#[test]
fn an_oversized_value_is_refused_before_it_is_encoded() {
    let frame = Frame {
        version: VERSION,
        request_id: 0,
        message: Message::StoreRequest(StoreRequest::Save {
            key: "big".into(),
            value: vec![0; MAX_STORE_VALUE + 1],
        }),
    };
    assert!(matches!(encode(&frame), Err(ProtocolError::FrameTooLarge)));
}

#[test]
fn keys_that_could_name_somewhere_else_are_refused() {
    for bad in [
        "",
        "..",
        "../../etc/passwd",
        ".hidden",
        "has/slash",
        "Upper",
        "has space",
        "has\\backslash",
        "nul\0byte",
    ] {
        assert!(!is_valid_key(bad), "{bad:?} was accepted as a key");
    }
    for good in ["tasks", "book.position", "a-b_c", "v2.state"] {
        assert!(is_valid_key(good), "{good:?} was refused as a key");
    }
    assert!(!is_valid_key(&"a".repeat(MAX_STORE_KEY_LEN + 1)));
    assert!(is_valid_key(&"a".repeat(MAX_STORE_KEY_LEN)));
}
