use super::*;
use kobo_ui::OptionRow;

/// Protocol 15, the newest one an application can have been built against
/// before the controls existed.
const BEFORE: u8 = PICTURE_BLEED_VERSION;

fn frame(version: u8, nodes: Vec<Node>) -> Frame {
    Frame {
        version,
        request_id: 3,
        message: Message::SetScreen(Screen::new(1, nodes)),
    }
}

fn round_trip(version: u8, node: &Node) -> Result<Node, ProtocolError> {
    let bytes = encode(&frame(version, vec![node.clone()]))?;
    match decode(&bytes)?.message {
        Message::SetScreen(mut screen) => Ok(screen.nodes.remove(0)),
        other => panic!("expected a screen, got {other:?}"),
    }
}

fn node_bytes(version: u8, node: &Node) -> Result<Vec<u8>, ProtocolError> {
    let mut output = Vec::new();
    let mut count = 0;
    encode_node(&mut output, node, 0, &mut count, version)?;
    let mut predicted = 0;
    assert_eq!(
        encoded_node_len(node, 0, &mut predicted, version)?,
        output.len(),
        "the predicted length disagrees with the encoder for {node:?}"
    );
    Ok(output)
}

fn decoded_at(version: u8, bytes: &[u8]) -> Result<Node, ProtocolError> {
    let mut reader = Reader::new(bytes);
    let mut count = 0;
    let node = decode_node(&mut reader, 0, &mut count, version)?;
    assert!(reader.is_finished(), "bytes left over");
    Ok(node)
}

fn new_controls() -> Vec<Node> {
    vec![
        Node::Toggle {
            id: NodeId(1),
            action: ActionId(2),
            label: "Sleep".into(),
            summary: String::new(),
            on: false,
            state: ControlState::Enabled,
        },
        Node::SelectionBar {
            id: NodeId(3),
            count: 0,
            actions: Vec::new(),
        },
        Node::Options {
            id: NodeId(4),
            prompt: String::new(),
            options: vec![OptionRow::new(ActionId(5), "UTC", "")],
            selected: None,
            page: 3,
            previous: ActionId(6),
            next: ActionId(7),
        },
        Node::Code {
            id: NodeId(8),
            payload: "https://example.org".into(),
            caption: String::new(),
        },
        Node::Steps {
            id: NodeId(9),
            current: 0,
            total: 2,
            labels: Vec::new(),
        },
    ]
}

fn plain_field() -> Node {
    field(FieldKind::Text, [None, None, None])
}

fn field(kind: FieldKind, [label, hint, error]: [Option<&str>; 3]) -> Node {
    Node::Field {
        id: NodeId(40),
        action: ActionId(41),
        value: "ab".into(),
        placeholder: "c".into(),
        clear: None,
        label: label.map(str::to_owned),
        hint: hint.map(str::to_owned),
        error: error.map(str::to_owned),
        kind,
    }
}

#[test]
fn every_new_control_round_trips_at_protocol_16() {
    for node in new_controls() {
        assert_eq!(round_trip(CONTROLS_VERSION, &node), Ok(node.clone()));
        node_bytes(CONTROLS_VERSION, &node).expect("encodes");
    }
}

#[test]
fn no_new_control_can_be_sent_at_protocol_15() {
    for node in new_controls() {
        assert_eq!(
            node_bytes(BEFORE, &node),
            Err(ProtocolError::InvalidValue("protocol 16 control")),
            "{node:?}"
        );
        assert!(encode(&frame(BEFORE, vec![node])).is_err());
    }
}

#[test]
fn a_protocol_15_frame_naming_a_new_tag_is_refused_rather_than_read() {
    for node in new_controls() {
        let bytes = node_bytes(CONTROLS_VERSION, &node).expect("encodes");
        assert!(decoded_at(CONTROLS_VERSION, &bytes).is_ok());
        assert_eq!(
            decoded_at(BEFORE, &bytes),
            Err(ProtocolError::InvalidValue("node tag")),
            "{node:?}"
        );
    }
}

#[test]
fn a_field_with_nothing_new_is_encoded_at_protocol_15_exactly_as_it_always_was() {
    // Tag, id, action, the two strings and the clear flag, written out by
    // hand from the protocol 15 layout rather than by the encoder under test.
    let mut expected = vec![24];
    expected.extend_from_slice(&40u32.to_be_bytes());
    expected.extend_from_slice(&41u32.to_be_bytes());
    expected.extend_from_slice(&[0, 2, b'a', b'b', 0, 1, b'c', 0]);
    assert_eq!(node_bytes(BEFORE, &plain_field()), Ok(expected.clone()));
    // Protocol 16 writes the same bytes and then the kind and three flags.
    let mut longer = expected;
    longer.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(node_bytes(CONTROLS_VERSION, &plain_field()), Ok(longer));
}

#[test]
fn a_banner_and_a_row_with_nothing_new_read_at_protocol_15_as_they_always_did() {
    let banner = Node::Banner {
        id: NodeId(1),
        level: BannerLevel::Attention,
        text: "Battery low".into(),
        action: None,
    };
    let rows = Node::Rows {
        id: NodeId(2),
        rows: vec![Row::new(ActionId(3), "Birds", "", Glyph::Book)
            .done(true)
            .with_trailing("12")
            .with_menu(ActionId(4))],
    };
    for node in [banner, rows, plain_field()] {
        let old = node_bytes(BEFORE, &node).expect("encodes");
        let new = node_bytes(CONTROLS_VERSION, &node).expect("encodes");
        assert!(new.starts_with(&old) && new.len() > old.len(), "{node:?}");
        assert_eq!(decoded_at(BEFORE, &old), Ok(node.clone()));
        assert_eq!(decoded_at(CONTROLS_VERSION, &new), Ok(node));
    }
}

#[test]
fn nothing_new_on_a_field_banner_or_row_can_be_sent_at_protocol_15() {
    let refused = [
        field(FieldKind::Secret, [None, None, None]),
        field(FieldKind::Text, [Some("Token"), None, None]),
        field(FieldKind::Text, [None, Some("From the account page"), None]),
        field(FieldKind::Text, [None, None, Some("Refused")]),
        Node::Banner {
            id: NodeId(1),
            level: BannerLevel::Success,
            text: "Saved".into(),
            action: None,
        },
        Node::Banner {
            id: NodeId(1),
            level: BannerLevel::Info,
            text: "Offline".into(),
            action: Some(BarAction::new(ActionId(9), "Retry")),
        },
        Node::Rows {
            id: NodeId(2),
            rows: vec![Row::new(ActionId(3), "Birds", "", Glyph::Book).checked(false)],
        },
        Node::Rows {
            id: NodeId(2),
            rows: vec![Row::new(ActionId(3), "Birds", "", Glyph::Book).with_badge(2)],
        },
        Node::Rows {
            id: NodeId(2),
            rows: vec![
                Row::new(ActionId(3), "Birds", "", Glyph::Book).with_button(ActionId(4), "Open")
            ],
        },
    ];
    for node in refused {
        assert!(node_bytes(BEFORE, &node).is_err(), "{node:?}");
        assert!(
            encode(&frame(BEFORE, vec![node.clone()])).is_err(),
            "{node:?}"
        );
        assert_eq!(round_trip(CONTROLS_VERSION, &node), Ok(node));
    }
}

#[test]
fn a_check_state_or_a_new_banner_level_from_protocol_15_is_refused() {
    let row = Node::Rows {
        id: NodeId(2),
        rows: vec![Row::new(ActionId(3), "Birds", "", Glyph::Book)],
    };
    let mut bytes = node_bytes(BEFORE, &row).expect("encodes");
    // The state byte follows the action, both strings and the fixed lead.
    let state = 1 + 4 + 1 + 4 + 7 + 2 + ROW_LEAD_LEN;
    assert_eq!(bytes[state], 0);
    for checked in 2..=4 {
        bytes[state] = checked;
        assert_eq!(
            decoded_at(BEFORE, &bytes),
            Err(ProtocolError::InvalidValue("row state"))
        );
    }

    let banner = Node::Banner {
        id: NodeId(1),
        level: BannerLevel::Info,
        text: String::new(),
        action: None,
    };
    let mut bytes = node_bytes(BEFORE, &banner).expect("encodes");
    for level in 2..=3 {
        bytes[5] = level;
        assert_eq!(
            decoded_at(BEFORE, &bytes),
            Err(ProtocolError::InvalidValue("banner level"))
        );
    }
}

#[test]
fn every_field_kind_has_one_byte_and_an_unknown_byte_is_refused() {
    for kind in [
        FieldKind::Text,
        FieldKind::Secret,
        FieldKind::Number,
        FieldKind::Address,
    ] {
        assert_eq!(decode_field_kind(encode_field_kind(kind)), Ok(kind));
    }
    assert_eq!(
        decode_field_kind(4),
        Err(ProtocolError::InvalidValue("field kind"))
    );
}

#[test]
fn a_code_longer_than_a_reader_can_scan_is_refused_on_the_wire() {
    let long = Node::Code {
        id: NodeId(1),
        payload: "x".repeat(kobo_ui::MAX_CODE_BYTES + 1),
        caption: String::new(),
    };
    assert_eq!(
        node_bytes(CONTROLS_VERSION, &long),
        Err(ProtocolError::InvalidValue("code payload"))
    );
    let fits = Node::Code {
        id: NodeId(1),
        payload: "x".repeat(kobo_ui::MAX_CODE_BYTES),
        caption: String::new(),
    };
    let mut bytes = node_bytes(CONTROLS_VERSION, &fits).expect("encodes");
    // One more byte of payload, written by a peer that never checked.
    bytes.insert(7, b'x');
    let length = u16::try_from(kobo_ui::MAX_CODE_BYTES + 1).unwrap();
    bytes[5..7].copy_from_slice(&length.to_be_bytes());
    assert_eq!(
        decoded_at(CONTROLS_VERSION, &bytes),
        Err(ProtocolError::InvalidValue("code payload"))
    );
}

#[test]
fn a_selection_bar_is_sent_with_three_verbs_at_most_and_more_are_refused() {
    let busy = Node::SelectionBar {
        id: NodeId(1),
        count: 7,
        actions: (0..5)
            .map(|index| BarAction::new(ActionId(10 + index), "Verb"))
            .collect(),
    };
    let Ok(Node::SelectionBar { actions, .. }) = round_trip(CONTROLS_VERSION, &busy) else {
        panic!("expected a selection bar");
    };
    assert_eq!(actions.len(), kobo_ui::MAX_SELECTION_ACTIONS);
    let mut bytes = node_bytes(CONTROLS_VERSION, &busy).expect("encodes");
    bytes[9] = 4;
    assert_eq!(
        decoded_at(CONTROLS_VERSION, &bytes),
        Err(ProtocolError::TooManyNodes)
    );
}

#[test]
fn an_option_answer_naming_no_option_arrives_unmarked_rather_than_refused() {
    let list = Node::Options {
        id: NodeId(1),
        prompt: "Zone".into(),
        options: vec![OptionRow::new(ActionId(2), "UTC", "")],
        selected: Some(7),
        page: 0,
        previous: ActionId(3),
        next: ActionId(4),
    };
    let Ok(Node::Options { selected, .. }) = round_trip(CONTROLS_VERSION, &list) else {
        panic!("expected an option list");
    };
    assert_eq!(selected, None);
}

#[test]
fn an_option_list_cannot_name_the_runtimes_own_back_action() {
    let list = Node::Options {
        id: NodeId(1),
        prompt: String::new(),
        options: vec![OptionRow::new(ActionId(2), "UTC", "")],
        selected: None,
        page: 0,
        previous: ActionId::BACK,
        next: ActionId(4),
    };
    let bytes = node_bytes(CONTROLS_VERSION, &list).expect("encodes");
    assert_eq!(
        decoded_at(CONTROLS_VERSION, &bytes),
        Err(ProtocolError::InvalidValue("reserved action id"))
    );
}

#[test]
fn steps_with_no_total_or_with_labels_for_some_steps_only_are_refused() {
    for (total, labels) in [
        (0, 0),
        (u8::try_from(kobo_ui::MAX_STEPS).unwrap() + 1, 0),
        (3, 2),
    ] {
        let steps = Node::Steps {
            id: NodeId(1),
            current: 0,
            total,
            labels: vec![String::from("A step"); labels],
        };
        assert_eq!(
            node_bytes(CONTROLS_VERSION, &steps),
            Err(ProtocolError::InvalidValue("steps")),
            "{total} {labels}"
        );
    }
    let past = Node::Steps {
        id: NodeId(1),
        current: 9,
        total: 3,
        labels: Vec::new(),
    };
    assert_eq!(
        round_trip(CONTROLS_VERSION, &past),
        Ok(Node::Steps {
            id: NodeId(1),
            current: 2,
            total: 3,
            labels: Vec::new(),
        })
    );
}

#[test]
fn a_toggle_flag_byte_naming_more_than_on_and_disabled_is_refused() {
    let toggle = new_controls().remove(0);
    let mut bytes = node_bytes(CONTROLS_VERSION, &toggle).expect("encodes");
    bytes[9] = 0b100;
    assert_eq!(
        decoded_at(CONTROLS_VERSION, &bytes),
        Err(ProtocolError::InvalidValue("toggle flags"))
    );
}
