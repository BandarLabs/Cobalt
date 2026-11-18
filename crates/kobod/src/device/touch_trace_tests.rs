//! What the session log keeps about taps on a keyboard an application draws.
//!
//! The black box writes every tap's position and action to the book
//! partition. On a keyboard either one names the key, so these tests type a
//! password the way an owner would, through the same resolution every tap
//! goes through, and read back what the log would hold.

use super::{is_keyboard_key, metrics_for, resolve_touch, TERMINAL_KEYS};
use kobo_hal::touch::TouchEvent;
use kobo_ui::keyboard::{key_name, Keyboard, LAYER, SPACE};
use kobo_ui::{ActionId, Chrome, FieldKind, Node, NodeId, Screen};

const JOIN: &str = "join-network";

/// The screen Settings draws for a Wi-Fi password: a button of its own, then
/// the shared keyboard's keys, as `ScreenBuilder::keyboard` lays them out.
fn password_screen(keyboard: &Keyboard) -> Screen {
    let mut nodes = vec![Node::Button {
        id: NodeId(1),
        action: ActionId::from_name(JOIN),
        label: "Join".to_owned(),
        emphasis: kobo_ui::Emphasis::Normal,
        state: kobo_ui::ControlState::Enabled,
    }];
    for (index, row) in keyboard.keys("Join").iter().enumerate() {
        nodes.push(Node::Grid {
            id: NodeId(u32::try_from(index).unwrap() + 2),
            columns: u8::try_from(row.len()).unwrap(),
            square: false,
            cells: row.iter().map(kobo_ui::keyboard::Key::cell).collect(),
        });
    }
    Screen::new(1, nodes)
}

/// Taps `name` in the middle of where it is drawn, and returns what the tap
/// hit and the line the log would keep.
fn tap(screen: &Screen, name: &str) -> (Option<ActionId>, String) {
    let chrome = Chrome::default();
    let rect = screen
        .layout_with(&metrics_for(screen), &chrome)
        .rect_of_action(ActionId::from_name(name))
        .unwrap_or_else(|| panic!("{name} is drawn"));
    resolve_touch(
        TouchEvent::Up {
            x: u32::try_from(rect.x + rect.width / 2).unwrap(),
            y: u32::try_from(rect.y + rect.height / 2).unwrap(),
        },
        Some(screen),
        &chrome,
        false,
        kobo_ui::Orientation::Portrait,
        kobo_ui::LandscapeTurn::Clockwise,
    )
    .expect("a tap on a drawn screen is resolved")
}

/// The key that types `character` on the page the keyboard shows now.
fn key_for(keyboard: &Keyboard, character: char) -> Option<String> {
    if character == ' ' {
        return Some(SPACE.to_owned());
    }
    keyboard.rows().iter().enumerate().find_map(|(row, keys)| {
        keys.chars()
            .position(|key| key == character)
            .map(|column| key_name(row, column))
    })
}

/// Types `password` and presses Join on the keyboard, one tap at a time,
/// and returns every line the session log would keep.
fn typed(password: &str) -> Vec<String> {
    let mut keyboard = Keyboard::new();
    let mut lines = Vec::new();
    let mut press = |keyboard: &mut Keyboard, name: &str| {
        let (hit, line) = tap(&password_screen(keyboard), name);
        assert_eq!(hit, Some(ActionId::from_name(name)), "{name} was hit");
        keyboard.press(ActionId::from_name(name));
        lines.push(line);
    };
    for character in password.chars() {
        let name = key_for(&keyboard, character).unwrap_or_else(|| {
            press(&mut keyboard, LAYER);
            key_for(&keyboard, character).expect("every character is on one of the pages")
        });
        press(&mut keyboard, &name);
    }
    press(&mut keyboard, kobo_ui::keyboard::ENTER);
    assert_eq!(keyboard.text(), password, "the taps typed the password");
    lines
}

#[test]
fn a_password_typed_on_an_application_keyboard_cannot_be_read_back_from_the_log() {
    let password = "correct horse 42";
    let lines = typed(password);
    assert!(lines.len() > password.chars().count());
    // Every tap is in the log, so the log still says typing happened.
    for line in &lines {
        assert!(line.contains("keyboard key"), "{line}");
    }
    // And every one of them says the same thing, so nothing in the log
    // depends on which key was pressed.
    assert!(
        lines.iter().all(|line| line == &lines[0]),
        "the log tells keys apart: {lines:?}"
    );
    // Neither the position of a key nor its action is written down.
    let joined = lines.join("\n");
    assert!(
        !joined.chars().any(|character| character.is_ascii_digit()),
        "{joined}"
    );
    assert!(!joined.contains("ActionId"), "{joined}");
}

#[test]
fn a_different_password_of_the_same_shape_leaves_the_same_log() {
    assert_eq!(typed("hunter two"), typed("zzzzzz qqq"));
}

#[test]
fn a_tap_on_anything_but_a_key_is_still_logged_with_where_it_landed_and_what_it_hit() {
    let (hit, line) = tap(&password_screen(&Keyboard::new()), JOIN);
    assert_eq!(hit, Some(ActionId::from_name(JOIN)));
    assert!(
        line.contains(&format!("{:?}", ActionId::from_name(JOIN))),
        "{line}"
    );
    assert!(line.starts_with("touch up ("), "{line}");
}

#[test]
fn every_key_any_keyboard_draws_is_kept_out_of_the_log() {
    let pages: [&[&str]; 4] = [&[], &["kb.shift"], &["kb.layer"], &["kb.layer", "kb.shift"]];
    for kind in [
        FieldKind::Text,
        FieldKind::Secret,
        FieldKind::Number,
        FieldKind::Address,
    ] {
        for page in pages {
            let mut keyboard = Keyboard::for_kind(kind);
            for name in page {
                keyboard.press(ActionId::from_name(name));
            }
            for key in keyboard.keys("Go").iter().flatten() {
                assert!(
                    is_keyboard_key(ActionId::from_name(&key.name)),
                    "{kind:?} {page:?}: {} would be logged",
                    key.name
                );
            }
        }
    }
    for name in TERMINAL_KEYS
        .into_iter()
        .chain([kobo_ui::keyboard::CANCEL, kobod::text_entry::REVEAL])
    {
        assert!(is_keyboard_key(ActionId::from_name(name)), "{name}");
    }
    for name in [JOIN, "back", "next", "previous", "settings"] {
        assert!(!is_keyboard_key(ActionId::from_name(name)), "{name}");
    }
}
