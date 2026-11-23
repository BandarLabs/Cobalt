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
fn the_bar_title_is_set_as_a_title_and_stays_under_the_pages_heading() {
    // The bar is the running head of the screen. In the body size and weight
    // it was one more line of grey text between the clock and the page, and
    // the bar carried nothing; in the heading cut at title size it names the
    // screen, and a page's own heading is still a step above it.
    let style = layout_text_style(&LayoutNode {
        id: NodeId(1),
        rect: Rect::default(),
        kind: LayoutKind::TopBarTitle,
        text_lines: vec!["Cobalt".to_owned()],
    });
    assert!(style.is_some_and(|(size, _)| size == FontSize::Title));
    assert!(FontSize::Title.tenth_mm() < FontSize::Heading.tenth_mm());
    for (name, metrics) in PANELS {
        assert!(
            FontSize::Title.line_height() <= metrics.top_bar_height(),
            "{name}: the title is taller than its bar"
        );
    }
}

#[test]
fn the_way_back_stands_on_the_margin_with_the_clock_and_the_page() {
    for (name, metrics) in PANELS {
        let screen = Screen::new(1, Vec::new()).with_top_bar(TopBar::new(NodeId(1), "Battery"));
        let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
        let back = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Back)
            .map(|node| node.rect)
            .expect("the way back");
        // The stroke of the arrow begins three eighths of the way across the
        // mark, less half its own weight.
        let mark = bar_mark(back);
        let ink = mark.x + mark.width * 3 / 8 - mark.width / 24;
        assert!(
            (ink - metrics.screen_margin()).abs() <= 1,
            "{name}: the arrow starts at {ink}, the margin is {}",
            metrics.screen_margin()
        );
        assert!(back.x >= 0, "{name}: the target leaves the panel");
        assert!(
            back.width >= metrics.touch_target_minimum()
                && back.height >= metrics.touch_target_minimum(),
            "{name}: the way back is smaller than a finger"
        );
        let title = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TopBarTitle)
            .map(|node| node.rect)
            .expect("the title");
        assert!(
            title.x >= back.x + back.width,
            "{name}: the title is under the arrow"
        );
    }
}

#[test]
fn a_picture_in_the_bar_ends_on_the_margin_and_words_stay_inside_it() {
    for (name, metrics) in PANELS {
        let margin = metrics.screen_margin();
        let picture = Screen::new(1, Vec::new()).with_top_bar(
            TopBar::new(NodeId(1), "Cobalt")
                .with_action(BarAction::new(ActionId(7), "Notices").with_glyph(Glyph::Bell)),
        );
        let layout = picture.layout_with(&metrics, &Chrome::with_back(false));
        let target = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::BarGlyph(..)))
            .map(|node| node.rect)
            .expect("the picture");
        let mark = bar_mark(target);
        assert!(
            (mark.x + mark.width - (metrics.width - margin)).abs() <= 1,
            "{name}: the picture ends at {}, the margin is at {}",
            mark.x + mark.width,
            metrics.width - margin
        );
        assert!(target.x + target.width <= metrics.width, "{name}");

        let words = Screen::new(1, Vec::new()).with_top_bar(
            TopBar::new(NodeId(1), "Cobalt").with_action(BarAction::new(ActionId(7), "Edit")),
        );
        let layout = words.layout_with(&metrics, &Chrome::with_back(false));
        let target = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::BarAction(_)))
            .map(|node| node.rect)
            .expect("the words");
        assert_eq!(target.x + target.width, metrics.width - margin, "{name}");
    }
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

#[test]
fn an_action_bar_is_set_in_ink_and_marks_none_of_its_verbs() {
    // Undo, Notes and Erase were drawn the grey of a tab nobody is on, and
    // read as switched off.
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "A board".into(),
            links: Vec::new(),
        }],
    )
    .with_nav_bar(NavBar::actions(
        NodeId(2),
        vec![
            BarAction::new(ActionId(3), "Undo"),
            BarAction::new(ActionId(4), "Notes"),
        ],
    ));
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let entries: Vec<_> = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::ActionBarEntry(..)))
        .collect();
    assert_eq!(entries.len(), 2);
    assert!(!layout.nodes.iter().any(|node| matches!(
        node.kind,
        LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
    )));
    let width = usize::try_from(CLARA_BW_METRICS.width).unwrap();
    let height = usize::try_from(CLARA_BW_METRICS.height).unwrap();
    let mut surface = Surface::new(width, height);
    render(&screen, &mut surface, None);
    let word = screen
        .layout()
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::ActionBarEntry(..)))
        .expect("an entry")
        .rect;
    let darkest = (word.y..word.y + word.height)
        .flat_map(|y| (word.x..word.x + word.width).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
            surface.pixels.get(y * width + x).copied()
        })
        .min()
        .unwrap_or(255);
    assert!(
        darkest < tone::MUTED,
        "the label is no darker than a muted one: {darkest}"
    );
}

#[test]
fn a_long_destination_name_takes_room_from_short_ones_rather_than_reaching_the_edge() {
    // At the largest sizes "Kobo reader" filled its quarter of the bar and
    // was drawn against the panel's right edge.
    let names = ["Apps", "Books", "Settings", "Kobo reader"];
    let bar = NavBar::new(
        NodeId(1),
        names
            .iter()
            .enumerate()
            .map(|(index, name)| BarAction::new(ActionId(index as u32 + 1), *name))
            .collect(),
        Some(0),
    );
    let screen = Screen::new(1, Vec::new()).with_nav_bar(bar);
    for scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale: scale,
            ..CLARA_BW_METRICS
        };
        let layout = with_text_scale(scale, || screen.layout_for(&metrics));
        let slots: Vec<_> = layout
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::NavDestination(..) | LayoutKind::NavDestinationSelected(..)
                )
            })
            .collect();
        assert_eq!(slots.len(), names.len());
        let margin = metrics.space(Space::Small);
        for slot in &slots {
            let word = with_text_scale(scale, || {
                measure_text(&slot.text_lines[0], FontSize::Caption).0
            });
            assert!(
                word + 2 * margin <= slot.rect.width,
                "{scale:?}: {:?} has no margin in its slot",
                slot.text_lines[0]
            );
        }
        let last = slots.last().expect("a last slot");
        assert_eq!(last.rect.x + last.rect.width, metrics.width, "{scale:?}");
    }
}
