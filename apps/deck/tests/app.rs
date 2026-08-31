#[path = "../src/model.rs"]
mod model;
use kobo_sdk::{action_id, ScreenBuilder};
use kobo_ui::{Chrome, CLARA_BW_METRICS};
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
fn clara_bw_portrait_grid_is_tappable() {
    let screen = ScreenBuilder::new("deck-grid")
        .top_bar("Deck")
        .grid(
            2,
            false,
            [("press-a", "Test\ncargo test"), ("press-b", "Deploy")],
        )
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for name in ["press-a", "press-b"] {
        assert!(
            layout.rect_of_action(action_id(name)).expect("key").height
                >= CLARA_BW_METRICS.touch_target_minimum()
        );
    }
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}
