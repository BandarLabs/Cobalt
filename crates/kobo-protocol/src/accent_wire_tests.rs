use super::*;

/// Protocol 16, the newest one an application can have been built against
/// before a screen could name its colour.
const BEFORE: u8 = CONTROLS_VERSION;

fn frame(version: u8, screen: Screen) -> Frame {
    Frame {
        version,
        request_id: 3,
        message: Message::SetScreen(screen),
    }
}

fn round_trip(version: u8, screen: &Screen) -> Screen {
    let bytes = encode(&frame(version, screen.clone())).expect("encodes");
    match decode(&bytes).expect("decodes").message {
        Message::SetScreen(screen) => screen,
        other => panic!("expected a screen, got {other:?}"),
    }
}

fn accented(accent: Option<Accent>) -> Screen {
    Screen::new(7, Vec::new())
        .with_top_bar(TopBar::new(NodeId(1), "Hacker News"))
        .with_accent(accent)
}

#[test]
fn an_accent_survives_the_wire_at_the_current_protocol() {
    for accent in Accent::ALL {
        let screen = accented(Some(accent));
        assert_eq!(round_trip(VERSION, &screen), screen, "{}", accent.name());
    }
    let plain = accented(None);
    assert_eq!(round_trip(VERSION, &plain), plain);
}

#[test]
fn a_screen_encoded_for_an_older_runtime_is_written_exactly_as_it_was() {
    // An application built against protocol 16 cannot name an accent, and a
    // version-17 encoder asked to write at 16 on its behalf must not add the
    // byte either: a version-16 reader would be left with one byte it has no
    // reading for.
    let plain = encode(&frame(BEFORE, accented(None))).expect("encodes");
    let coloured = encode(&frame(BEFORE, accented(Some(Accent::Orange)))).expect("encodes");
    assert_eq!(
        plain, coloured,
        "an accent changed the bytes of a protocol-16 screen"
    );
    assert_eq!(
        round_trip(BEFORE, &accented(Some(Accent::Orange))).accent,
        None
    );
    // And the current protocol costs exactly the one byte more.
    let current = encode(&frame(VERSION, accented(None))).expect("encodes");
    assert_eq!(current.len(), plain.len() + 1);
}

#[test]
fn the_accent_is_the_last_byte_of_the_screen() {
    let bytes = encode(&frame(VERSION, accented(Some(Accent::Teal)))).expect("encodes");
    assert_eq!(bytes.last(), Some(&Accent::Teal.wire_value()));
    let none = encode(&frame(VERSION, accented(None))).expect("encodes");
    assert_eq!(none.last(), Some(&0));
}

#[test]
fn an_accent_this_runtime_has_no_name_for_is_read_as_none_rather_than_refused() {
    // A newer application naming a colour added after this runtime shipped
    // still opens; its bar is paper and its controls are the platform's.
    let mut bytes = encode(&frame(VERSION, accented(Some(Accent::Teal)))).expect("encodes");
    let last = bytes.len() - 1;
    bytes[last] = 250;
    match decode(&bytes).expect("an unknown accent decodes").message {
        Message::SetScreen(screen) => assert_eq!(screen.accent, None),
        other => panic!("expected a screen, got {other:?}"),
    }
}

#[test]
fn a_screen_with_an_overlay_keeps_its_accent_after_the_overlay() {
    // The accent follows the overlay, which was the last thing a screen
    // carried, so a reader that stops after the overlay reads a whole
    // version-16 screen and a reader that goes on finds exactly one byte.
    let screen = accented(Some(Accent::Purple)).with_overlay(kobo_ui::Overlay {
        id: NodeId(9),
        kind: kobo_ui::OverlayKind::Modal,
        title: "Sure?".to_owned(),
        nodes: vec![Node::Text {
            id: NodeId(10),
            text: "Yes".to_owned(),
            links: Vec::new(),
        }],
    });
    assert_eq!(round_trip(VERSION, &screen), screen);
}

#[test]
fn every_protocol_from_the_dictionary_on_is_still_accepted() {
    for version in DICTIONARY_VERSION..=VERSION {
        assert!(accepted_version(version), "protocol {version} was dropped");
    }
    assert!(!accepted_version(VERSION + 1));
    assert!(!accepted_version(DICTIONARY_VERSION - 1));
}
