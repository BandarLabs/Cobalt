use super::*;
use kobo_ui::FieldKind;

fn ask(kind: FieldKind) -> DeviceRequest {
    DeviceRequest::TypeText {
        prompt: "New item".into(),
        hint: "Added \u{201c}milk\u{201d}.".into(),
        initial: if kind == FieldKind::Secret {
            String::new()
        } else {
            "café".into()
        },
        submit: "Add".into(),
        kind,
        save_as: (kind == FieldKind::Secret).then(|| SecretTarget {
            name: "komga".into(),
            server: Some("https://books.example".into()),
            username: Some("reader".into()),
        }),
    }
}

fn frame(version: u8, message: Message) -> Frame {
    Frame {
        version,
        request_id: 7,
        message,
    }
}

fn answers() -> Vec<TextOutcome> {
    vec![
        TextOutcome::Typed("water the fern".into()),
        TextOutcome::Typed(String::new()),
        TextOutcome::Saved,
        TextOutcome::Cancelled,
        TextOutcome::Denied(DenyReason::NotDeclared),
        TextOutcome::Failed(DeviceError::Backend),
    ]
}

#[test]
fn a_text_request_and_every_answer_to_it_round_trip_at_protocol_sixteen() {
    for kind in [
        FieldKind::Text,
        FieldKind::Secret,
        FieldKind::Number,
        FieldKind::Address,
    ] {
        let request = frame(TEXT_ENTRY_VERSION, Message::DeviceRequest(ask(kind)));
        let bytes = encode(&request).unwrap();
        assert_eq!(decode(&bytes).unwrap(), request);
        for length in HEADER_LEN..bytes.len() {
            assert!(
                decode(&bytes[..length]).is_err(),
                "{kind:?} cut at {length}"
            );
        }
    }
    for outcome in answers() {
        let answer = frame(
            TEXT_ENTRY_VERSION,
            Message::DeviceResult(DeviceResult::Text(outcome)),
        );
        let bytes = encode(&answer).unwrap();
        assert_eq!(decode(&bytes).unwrap(), answer);
    }
}

#[test]
fn a_text_request_and_its_answer_are_refused_before_protocol_sixteen() {
    // An older runtime has no keyboard to draw and an older application no
    // way to read the answer, so both ends refuse rather than guess.
    let request = encode(&frame(
        TEXT_ENTRY_VERSION,
        Message::DeviceRequest(ask(FieldKind::Text)),
    ))
    .unwrap();
    let answer = encode(&frame(
        TEXT_ENTRY_VERSION,
        Message::DeviceResult(DeviceResult::Text(TextOutcome::Cancelled)),
    ))
    .unwrap();
    for version in ACCEPTED_VERSIONS
        .into_iter()
        .filter(|version| *version < TEXT_ENTRY_VERSION)
    {
        assert_eq!(
            encode(&frame(
                version,
                Message::DeviceRequest(ask(FieldKind::Text))
            )),
            Err(ProtocolError::UnsupportedVersion(version))
        );
        assert_eq!(
            encode(&frame(
                version,
                Message::DeviceResult(DeviceResult::Text(TextOutcome::Saved))
            )),
            Err(ProtocolError::UnsupportedVersion(version))
        );
        for bytes in [&request, &answer] {
            let mut older = bytes.clone();
            older[4] = version;
            assert!(decode(&older).is_err(), "protocol {version} read it");
        }
    }
}

#[test]
fn a_secret_field_names_where_it_is_kept_and_cannot_be_handed_a_value() {
    // The application never holds the secret, so it has nothing to put in
    // the field to begin with, and a secret with nowhere to go is refused
    // before the owner types anything into it.
    let DeviceRequest::TypeText {
        prompt,
        hint,
        submit,
        save_as,
        ..
    } = ask(FieldKind::Secret)
    else {
        unreachable!()
    };
    let refused = [
        DeviceRequest::TypeText {
            prompt: prompt.clone(),
            hint: hint.clone(),
            initial: "old-token-12".into(),
            submit: submit.clone(),
            kind: FieldKind::Secret,
            save_as: save_as.clone(),
        },
        DeviceRequest::TypeText {
            prompt: prompt.clone(),
            hint: hint.clone(),
            initial: String::new(),
            submit: submit.clone(),
            kind: FieldKind::Secret,
            save_as: None,
        },
        DeviceRequest::TypeText {
            prompt: prompt.clone(),
            hint: hint.clone(),
            initial: String::new(),
            submit: submit.clone(),
            kind: FieldKind::Text,
            save_as: save_as.clone(),
        },
        DeviceRequest::TypeText {
            prompt: prompt.clone(),
            hint: hint.clone(),
            initial: String::new(),
            submit: submit.clone(),
            kind: FieldKind::Secret,
            save_as: Some(SecretTarget {
                username: Some("user:name".into()),
                ..save_as.clone().unwrap()
            }),
        },
        DeviceRequest::TypeText {
            prompt: prompt.clone(),
            hint: hint.clone(),
            initial: String::new(),
            submit,
            kind: FieldKind::Secret,
            save_as: Some(SecretTarget {
                server: Some("http://books.example".into()),
                ..save_as.unwrap()
            }),
        },
    ];
    for request in refused {
        assert!(
            encode(&frame(VERSION, Message::DeviceRequest(request.clone()))).is_err(),
            "{request:?}"
        );
    }
}

#[test]
fn a_field_must_say_what_it_asks_for_and_cannot_hold_more_than_the_keyboard_types() {
    let base = |prompt: &str, submit: &str, initial: String| DeviceRequest::TypeText {
        prompt: prompt.into(),
        hint: String::new(),
        initial,
        submit: submit.into(),
        kind: FieldKind::Text,
        save_as: None,
    };
    for request in [
        base("", "Add", String::new()),
        base("New item", "", String::new()),
        base("New\nitem", "Add", String::new()),
        base(
            "New item",
            "Add",
            "x".repeat(kobo_ui::keyboard::MAX_TEXT + 1),
        ),
        base(
            "New item",
            "Add a very long label nobody can read",
            String::new(),
        ),
    ] {
        assert!(
            encode(&frame(VERSION, Message::DeviceRequest(request.clone()))).is_err(),
            "{request:?}"
        );
    }
    assert!(encode(&frame(
        VERSION,
        Message::DeviceRequest(base(
            "New item",
            "Add",
            "x".repeat(kobo_ui::keyboard::MAX_TEXT)
        ))
    ))
    .is_ok());
    assert!(encode(&frame(
        VERSION,
        Message::DeviceResult(DeviceResult::Text(TextOutcome::Typed("a\nb".into())))
    ))
    .is_err());
}

#[test]
fn a_typed_secret_is_stored_through_the_same_request_a_handed_over_one_is() {
    let plain = SecretTarget::named("lichess");
    assert!(matches!(
        plain.install("lip_token"),
        DeviceRequest::SetSecret { name, value } if name == "lichess" && value.as_str() == "lip_token"
    ));
    let basic = SecretTarget {
        name: "komga".into(),
        server: Some("https://books.example".into()),
        username: Some("reader".into()),
    };
    assert!(matches!(
        basic.install("  open sesame "),
        DeviceRequest::SetServerSecret { name, server, value }
            if name == "komga"
                && server == "https://books.example"
                && value.as_str() == "reader:  open sesame "
    ));
    // A secret is never written into a log line by accident.
    assert!(!format!("{:?}", basic.install("open sesame")).contains("sesame"));
}
