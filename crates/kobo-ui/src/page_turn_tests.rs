use super::*;

fn paged() -> Screen {
    Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(2),
            text: "A page of a book.".to_owned(),
            links: Vec::new(),
        }],
    )
    .with_top_bar(TopBar::new(NodeId(3), "Reading"))
    .with_page_turns(ActionId(10), ActionId(20))
}

#[test]
fn the_left_of_the_page_goes_back_and_the_rest_goes_on() {
    // The gesture every Kobo has had since the first one.
    for (name, metrics) in super::tests::PANELS {
        let layout = paged().layout_for(&metrics);
        let middle = metrics.height / 2;
        assert_eq!(
            layout.hit_test(metrics.width / 8, middle),
            Some(ActionId(10)),
            "{name} did not go back"
        );
        assert_eq!(
            layout.hit_test(metrics.width * 7 / 8, middle),
            Some(ActionId(20)),
            "{name} did not go on"
        );
    }
}

#[test]
fn a_control_always_beats_the_zone_underneath_it() {
    // The failure this covers is the worst one available: tapping a button
    // and turning the page instead.
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(2),
            action: ActionId(99),
            label: "Press me".to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_page_turns(ActionId(10), ActionId(20));
    let layout = screen.layout();
    let button = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("a button");
    let (x, y) = (
        button.rect.x + button.rect.width / 2,
        button.rect.y + button.rect.height / 2,
    );
    assert_eq!(layout.hit_test(x, y), Some(ActionId(99)));
}

#[test]
fn the_bars_are_never_page_turns() {
    // Back and the navigation are the two controls a reader must be able
    // to hit without aiming.
    let screen = paged().with_nav_bar(NavBar::new(
        NodeId(4),
        vec![
            BarAction::new(ActionId(5), "One"),
            BarAction::new(ActionId(6), "Two"),
        ],
        Some(0),
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    assert_eq!(layout.hit_page_turn(CLARA_BW_METRICS.width / 8, 10), None);
    assert_eq!(
        layout.hit_page_turn(
            CLARA_BW_METRICS.width / 8,
            CLARA_BW_METRICS.height - CLARA_BW_METRICS.nav_bar_height() / 2
        ),
        None
    );
}

#[test]
fn a_screen_that_did_not_ask_for_them_has_none() {
    let layout = Screen::new(1, vec![]).layout();
    assert_eq!(layout.hit_test(10, 500), None);
}
