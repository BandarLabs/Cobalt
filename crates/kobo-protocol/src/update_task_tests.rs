use super::*;

fn update(method: UpdateMethod) -> Task {
    Task::Update {
        method,
        url: "https://reader.test/v1/entries".into(),
        body: "{\"title\":\"Café\"}".into(),
        content_type: "application/json".into(),
        credential: Some(Credential::in_header("miniflux", "X-Auth-Token")),
        headers: vec![Header::new("Accept", "application/json")],
        max_bytes: 1024,
    }
}

#[test]
fn update_wire_round_trips_and_refuses_unknown_methods_and_old_versions() {
    for method in [UpdateMethod::Put, UpdateMethod::Patch] {
        let frame = Frame {
            version: VERSION,
            request_id: 7,
            message: Message::Spawn {
                task: TaskId(9),
                work: update(method),
            },
        };
        let bytes = encode(&frame).unwrap();
        assert_eq!(decode(&bytes).unwrap(), frame);
        assert_eq!(bytes[HEADER_LEN + 4], 4, "new task tag");
        let mut unknown = bytes.clone();
        unknown[HEADER_LEN + 5] = 2;
        assert!(decode(&unknown).is_err());
        for version in [
            DICTIONARY_VERSION,
            LEGACY_VERSION,
            FOLIO_VERSION,
            SELECTED_GRID_VERSION,
        ] {
            let mut older = frame.clone();
            older.version = version;
            assert!(encode(&older).is_err());
            let mut older_wire = bytes.clone();
            older_wire[4] = version;
            assert!(decode(&older_wire).is_err());
        }
        for end in HEADER_LEN..bytes.len() {
            assert!(decode(&bytes[..end]).is_err(), "truncation {end}");
        }
    }
}

#[test]
fn an_auto_hidden_top_bar_costs_nothing_for_the_screens_that_never_ask() {
    let plain = Screen::new(1, Vec::new());
    let asking = Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true);
    let frame = |screen: Screen, version| Frame {
        version,
        request_id: 3,
        message: Message::SetScreen(screen),
    };
    // The whole point of carrying it in the presence byte: a screen that
    // does not ask encodes the bytes it always did, so nothing published
    // has to be republished for a flag it never sets. The current protocol
    // adds only its accent byte at the very end, zero for a screen with none.
    let current = encode(&frame(plain.clone(), VERSION)).unwrap();
    assert_eq!(
        encode(&frame(plain.clone(), SUSPEND_VERSION)).unwrap()[HEADER_LEN..],
        current[HEADER_LEN..current.len() - 1]
    );
    assert_eq!(current.last(), Some(&0));

    let decoded = decode(&encode(&frame(asking.clone(), VERSION)).unwrap()).unwrap();
    let Message::SetScreen(screen) = decoded.message else {
        panic!("screen");
    };
    assert!(screen.auto_hide_top_bar, "the ask must survive the wire");

    // An older runtime was never told about hiding, so asking it is not
    // encoded at all rather than encoded and silently misread.
    let older = decode(&encode(&frame(asking, SUSPEND_VERSION)).unwrap()).unwrap();
    let Message::SetScreen(screen) = older.message else {
        panic!("screen");
    };
    assert!(
        !screen.auto_hide_top_bar,
        "a protocol that cannot hide a bar must not claim to have hidden one"
    );
}

#[test]
fn suspend_still_reaches_an_application_built_before_this_protocol() {
    // Suspend arrived at protocol 14, and the guard that admits it was
    // written as "the current version". Moving the current version left
    // every published application unable to be told the cover had closed.
    for message in [
        Message::PrepareSuspend { generation: 4 },
        Message::SuspendReady {
            generation: 4,
            ready: true,
        },
    ] {
        let frame = Frame {
            version: SUSPEND_VERSION,
            request_id: 2,
            message,
        };
        assert_eq!(
            decode(&encode(&frame).unwrap()).unwrap(),
            frame,
            "a protocol 14 application must still be suspendable"
        );
    }
}

#[test]
fn an_application_published_before_this_protocol_still_speaks_to_it() {
    // The case that must never break: a reader updates, and every
    // application already installed on it carries on. Those binaries were
    // built against protocol 14 and will never be rebuilt, so 14 has to
    // keep working in a runtime that has moved on to 15.
    let screen = Screen::new(
        7,
        vec![Node::Heading {
            id: NodeId(1),
            level: 1,
            text: "Chapter".into(),
        }],
    )
    .with_top_bar(TopBar::new(NodeId(2), "Books"))
    .with_reading(true)
    .with_typography(kobo_ui::Typography::Folio);
    let frame = Frame {
        version: SUSPEND_VERSION,
        request_id: 5,
        message: Message::SetScreen(screen),
    };
    assert_eq!(
        decode(&encode(&frame).unwrap()).unwrap(),
        frame,
        "a protocol 14 screen must survive a protocol 15 runtime unchanged"
    );

    // The handshake decides the session, and the runtime answers in the
    // version the application opened with rather than its own newest.
    let hello = Frame {
        version: SUSPEND_VERSION,
        request_id: 1,
        message: Message::Hello {
            name: "books".into(),
        },
    };
    let decoded = decode(&encode(&hello).unwrap()).unwrap();
    assert_eq!(
        decoded.version, SUSPEND_VERSION,
        "the runtime must keep speaking the version the application opened with"
    );
}

#[test]
fn a_bar_hidden_by_a_protocol_that_has_no_such_flag_is_refused() {
    // Byte 4 is the version and the presence byte follows the screen id.
    // A payload that sets the auto-hide bit while naming protocol 14 is
    // malformed: granting it would let a crafted frame take the way out
    // off a screen on a runtime that cannot give it back.
    let frame = Frame {
        version: VERSION,
        request_id: 1,
        message: Message::SetScreen(Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true)),
    };
    let mut bytes = encode(&frame).unwrap();
    assert_eq!(bytes[HEADER_LEN + 4], 2, "an auto-hidden bar with no bar");
    bytes[4] = SUSPEND_VERSION;
    assert!(decode(&bytes).is_err());
}

#[test]
fn update_tasks_enforce_existing_body_and_header_bounds() {
    let mut work = update(UpdateMethod::Put);
    assert!(work.is_sendable());
    if let Task::Update { body, .. } = &mut work {
        *body = "x".repeat(MAX_POST_BODY_LEN + 1);
    }
    assert!(!work.is_sendable());
    assert!(encoded_task_len(&work).is_err());
    let mut work = update(UpdateMethod::Patch);
    if let Task::Update { headers, .. } = &mut work {
        headers.push(Header::new("Bad\r\nHeader", "value"));
    }
    assert!(!work.is_sendable());
    assert!(encoded_task_len(&work).is_err());
}
