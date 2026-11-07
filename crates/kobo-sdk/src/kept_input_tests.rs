//! Builder calls that used to lose what they were given: a page position
//! asked for before the turns, and a band past its third slot.

use crate::{action_id, ScreenBuilder};

#[test]
fn a_page_position_given_before_the_turns_is_kept() {
    let early = ScreenBuilder::new("inbox")
        .page_position(2, 5)
        .reading_menu("menu")
        .page_turns("previous", "next")
        .build();
    let late = ScreenBuilder::new("inbox")
        .page_turns("previous", "next")
        .page_position(2, 5)
        .reading_menu("menu")
        .build();
    let turns = early.page_turns.expect("turns were asked for");
    assert_eq!(turns.position, Some((2, 5)));
    assert_eq!(turns.menu, Some(action_id("menu")));
    assert_eq!(early.page_turns, late.page_turns);
}

#[test]
fn a_page_position_without_turns_still_draws_nothing() {
    let screen = ScreenBuilder::new("inbox").page_position(2, 5).build();
    assert!(screen.page_turns.is_none());
}

#[test]
fn a_band_of_four_buttons_wraps_into_two_rows_and_keeps_all_four() {
    // On an upright Clara the fourth used to be dropped without a word.
    let screen = ScreenBuilder::new("controls")
        .band(
            crate::BandAlign::Middle,
            ["one", "two", "three", "four"].map(|name| {
                (crate::SlotWidth::Fill, move |slot: ScreenBuilder| {
                    slot.button(name, name)
                })
            }),
        )
        .build();
    let metrics = crate::CLARA_BW_METRICS;
    let report = screen.diagnostics(&metrics, &crate::Chrome::measuring(true));
    assert!(report.issues.is_empty(), "{:?}", report.issues);
    for name in ["one", "two", "three", "four"] {
        assert!(
            report.layout.rect_of_action(action_id(name)).is_some(),
            "{name} was not drawn"
        );
    }
    let bands = screen
        .nodes
        .iter()
        .filter(|node| matches!(node, kobo_ui::Node::Band { slots, .. } if slots.len() == 2))
        .count();
    assert_eq!(bands, 2);
}

#[test]
fn a_band_node_with_more_slots_than_are_drawn_is_reported() {
    let slot = |name: &str| {
        kobo_ui::BandSlot::new(
            crate::SlotWidth::Fill,
            vec![kobo_ui::Node::Text {
                id: kobo_ui::NodeId(9),
                text: name.to_owned(),
                links: Vec::new(),
            }],
        )
    };
    let screen = kobo_ui::Screen::new(
        1,
        vec![kobo_ui::Node::Band {
            id: kobo_ui::NodeId(1),
            align: crate::BandAlign::Middle,
            slots: vec![slot("a"), slot("b"), slot("c"), slot("d")],
        }],
    );
    assert!(screen
        .validate(&crate::CLARA_BW_METRICS)
        .iter()
        .any(|issue| matches!(
            issue.kind,
            kobo_ui::LayoutIssueKind::CollectionTruncated {
                provided: 4,
                visible: 3,
                ..
            }
        )));
}
