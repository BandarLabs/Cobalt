use super::tests::PANELS;
use super::*;

fn destinations(count: usize) -> Vec<BarAction> {
    (0..count)
        .map(|index| BarAction::new(ActionId(index as u32 + 1), format!("Tab {index}")))
        .collect()
}

fn kinds(layout: &Layout) -> Vec<LayoutKind> {
    layout.nodes.iter().map(|node| node.kind).collect()
}

#[test]
fn the_bar_names_the_screen_without_shouting_over_it() {
    // The bar says which screen you are on. It is a label, not a headline,
    // and at title size it was the loudest thing on every page, larger
    // than the first heading of the content underneath it, which inverts
    // the hierarchy Kobo's own reader uses.
    assert!(
        layout_text_style(&LayoutNode {
            id: NodeId(1),
            rect: Rect::default(),
            kind: LayoutKind::TopBarTitle,
            text_lines: vec!["Cobalt".to_owned()],
        })
        .is_some_and(|(size, _)| size.tenth_mm() < FontSize::Title.tenth_mm()),
        "the bar title is still set at title size"
    );
}

#[test]
fn back_is_absent_until_the_runtime_supplies_it() {
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));

    let without = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
    assert!(!kinds(&without).contains(&LayoutKind::Back));

    let with = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    assert!(kinds(&with).contains(&LayoutKind::Back));
}

#[test]
fn back_is_reachable_by_touch_and_reports_the_reserved_action() {
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Settings"));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(true));
    let back = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Back)
        .expect("back control");
    assert_eq!(
        layout.hit_test(back.rect.x + 1, back.rect.y + 1),
        Some(ActionId::BACK)
    );
}

#[test]
fn the_back_control_is_never_smaller_than_a_finger_on_any_panel() {
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
    for (name, metrics) in PANELS {
        let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
        let back = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Back)
            .expect("back control");
        assert!(
            back.rect.width >= metrics.touch_target_minimum()
                && back.rect.height >= metrics.touch_target_minimum(),
            "{name}: back control is {}x{}, below the {} minimum",
            back.rect.width,
            back.rect.height,
            metrics.touch_target_minimum()
        );
    }
}

#[test]
fn the_back_control_stays_inside_the_bar_it_belongs_to() {
    // It did not, once. The comfortable control size is ten millimetres
    // and the bar was narrowed to eight and a half, so the chevron was
    // laid out taller than the bar at a negative offset and drew above it,
    // over whatever the screen had put there.
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Title"));
    for (name, metrics) in PANELS {
        let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
        let bar = metrics.top_bar_height();
        for node in &layout.nodes {
            if !matches!(
                node.kind,
                LayoutKind::Back | LayoutKind::BarAction(_) | LayoutKind::BarGlyph(..)
            ) {
                continue;
            }
            assert!(
                node.rect.y >= 0 && node.rect.y + node.rect.height <= bar,
                "{name}: {:?} spans {}..{} outside a bar {bar} tall",
                node.kind,
                node.rect.y,
                node.rect.y + node.rect.height
            );
        }
    }
}

#[test]
fn a_title_that_would_wrap_is_truncated_rather_than_growing_the_bar() {
    // A bar that grows to fit its title moves every screen's content, so
    // the title yields instead. Long enough to overrun the widest panel at
    // the bar's own size, which is body size and not title size: the first
    // version of this title fitted once the bar stopped shouting.
    let long = "An extremely long screen title that could never fit across one line of \
                    any panel this system has ever been built for, however wide";
    let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), long));
    for (name, metrics) in PANELS {
        let layout = screen.layout_for(&metrics);
        let title = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TopBarTitle)
            .expect("title");
        assert_eq!(title.text_lines.len(), 1, "{name}: title wrapped");
        // And it says it was cut. Keeping the first wrapped line and
        // dropping the rest reads as the whole title, which on a news
        // headline is a different sentence rather than a shorter one.
        assert!(
            title.text_lines[0].ends_with('\u{2026}'),
            "{name}: a cut title did not say so: {:?}",
            title.text_lines[0]
        );
        assert!(
            long.starts_with(title.text_lines[0].trim_end_matches(['\u{2026}', ' '])),
            "{name}: the shown title is not the start of the real one: {:?}",
            title.text_lines[0]
        );
        let bar = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TopBar)
            .expect("bar");
        assert_eq!(
            bar.rect.height,
            metrics.top_bar_height(),
            "{name}: bar grew to fit its title"
        );
    }
}

#[test]
fn the_nav_bar_sits_on_the_bottom_edge_and_spans_the_panel() {
    let screen =
        Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(NodeId(1), destinations(3), Some(0)));
    for (name, metrics) in PANELS {
        let layout = screen.layout_for(&metrics);
        let slots = layout
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
                )
            })
            .collect::<Vec<_>>();
        assert!(!slots.is_empty(), "{name}: no destinations");
        let first = slots.first().expect("first");
        let last = slots.last().expect("last");
        assert_eq!(first.rect.x, 0, "{name}: bar does not reach the left edge");
        assert_eq!(
            last.rect.x + last.rect.width,
            metrics.width,
            "{name}: bar leaves a dead strip on the right"
        );
        assert_eq!(
            first.rect.y + first.rect.height,
            metrics.height,
            "{name}: bar is not on the bottom edge"
        );
    }
}

#[test]
fn destinations_are_never_narrower_than_a_finger_on_any_panel() {
    for (name, metrics) in PANELS {
        // Ask for more than the panel can carry, on purpose.
        let screen = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
            NodeId(1),
            destinations(5),
            Some(0),
        ));
        let layout = screen.layout_for(&metrics);
        for node in &layout.nodes {
            if matches!(
                node.kind,
                LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
            ) {
                assert!(
                    node.rect.width >= metrics.touch_target_minimum(),
                    "{name}: destination is {} wide, below the {} minimum",
                    node.rect.width,
                    metrics.touch_target_minimum()
                );
            }
        }
    }
}

#[test]
fn content_stops_above_the_nav_bar_rather_than_flowing_under_it() {
    let nodes = (0..40)
        .map(|index| Node::Text {
            id: NodeId(index),
            text: "A line of body copy that occupies a row".into(),
            links: Vec::new(),
        })
        .collect();
    let screen =
        Screen::new(1, nodes).with_nav_bar(NavBar::new(NodeId(99), destinations(3), Some(0)));
    for (name, metrics) in PANELS {
        let layout = screen.layout_for(&metrics);
        let content_bottom = metrics.height - metrics.nav_bar_height();
        for node in &layout.nodes {
            if node.kind == LayoutKind::Text {
                assert!(
                    node.rect.y < content_bottom,
                    "{name}: content starts underneath the nav bar"
                );
            }
        }
    }
}

#[test]
fn exactly_one_destination_reads_as_selected() {
    let screen =
        Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(NodeId(1), destinations(3), Some(2)));
    let layout = screen.layout();
    let selected = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::NavDestinationSelected(..)))
        .count();
    assert_eq!(selected, 1);
}
