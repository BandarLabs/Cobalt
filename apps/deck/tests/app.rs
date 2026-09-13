#[path = "../src/model.rs"]
mod model;
use kobo_sdk::{action_id, ScreenBuilder};
use kobo_ui::{CellStyle, Chrome, LayoutKind, CLARA_BW_METRICS};
use model::*;
#[test]
fn daemon_response_decodes_stable_key_data() {
    let deck=decode(r#"{"version":4,"pages":[{"name":"Build","keys":[{"id":"abc","label":"Test","detail":"cargo test","confirm":false,"state":"idle"}]}]}"#).expect("deck");
    assert_eq!(deck.version, 4);
    assert_eq!(deck.pages[0].keys[0].id, "abc");
}
#[test]
fn fallback_is_an_empty_unarmed_deck() {
    assert!(Deck::fallback().pages[0].keys.is_empty());
}
#[test]
fn cli_assignment_round_trips_into_the_rendered_grid() {
    let deck = decode(
        r#"{"version":"1","pages":[{"name":"Home","keys":[{"id":"pad-todo","label":"Todo","detail":"launch todo","confirm":false,"state":"idle"},{"id":"pad-url","label":"Example","detail":"example.com","confirm":false,"state":"idle"}]}]}"#,
    )
    .expect("cli snapshot");
    assert_eq!(deck.pages[0].name, "Home");
    assert_eq!(deck.pages[0].keys[0].label, "Todo");
    assert_eq!(deck.pages[0].keys[1].label, "Example");
    let cells = pad_cells(&deck.pages[0]);
    assert_eq!(cells.len(), PAD_COUNT);
    assert!(
        cells
            .iter()
            .any(|(name, label, _)| name.contains("pad-todo") && label == "Todo"),
        "{cells:?}"
    );
    assert!(
        cells
            .iter()
            .any(|(name, label, _)| name.contains("pad-url") && label == "Example"),
        "{cells:?}"
    );
    let screen = ScreenBuilder::new("deck-grid")
        .top_bar("Deck")
        .pads(cells)
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for name in ["press-pad-todo", "press-pad-url"] {
        let rect = layout.rect_of_action(action_id(name)).expect(name);
        assert!(rect.height >= CLARA_BW_METRICS.touch_target_minimum());
        assert_eq!(rect.width, rect.height, "{name} should be square");
    }
    // The two assigned pads are keys; the rest of the deck is places for one.
    let keys = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::Pad, _)))
        .count();
    let places = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::EmptyPad, _)))
        .count();
    assert_eq!(keys, 2);
    assert_eq!(keys + places, PAD_COUNT);
    let issues = screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues;
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn clara_bw_portrait_grid_is_tappable() {
    let screen = ScreenBuilder::new("deck-grid")
        .top_bar("Deck")
        .pads([
            ("press-a", "Test", Some(kobo_sdk::Glyph::Grid)),
            ("press-b", "Deploy", Some(kobo_sdk::Glyph::Grid)),
        ])
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for name in ["press-a", "press-b"] {
        assert!(
            layout.rect_of_action(action_id(name)).expect("key").height
                >= CLARA_BW_METRICS.touch_target_minimum()
        );
    }
    // Two keys and thirteen places for one. A place is drawn in a hairline
    // rather than the bezel a key gets, so a deck with three things on it
    // does not read as a panel of twelve controls that do nothing.
    let keys = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::Pad, _)))
        .count();
    let places = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Cell(_, CellStyle::EmptyPad, _)))
        .count();
    assert_eq!(keys, 2);
    assert_eq!(keys + places, PAD_COUNT);
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}
