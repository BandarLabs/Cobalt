use super::tests::PANELS;
use super::*;

#[test]
fn an_attention_banner_is_at_least_finger_tall_on_every_panel() {
    let screen = Screen::new(
        1,
        vec![Node::Banner {
            id: NodeId(1),
            level: BannerLevel::Attention,
            text: "Battery low".into(),
            action: None,
        }],
    );
    for (name, metrics) in PANELS {
        let layout = screen.layout_for(&metrics);
        let banner = layout.nodes.first().expect("banner");
        assert!(
            banner.rect.height >= metrics.touch_target_minimum(),
            "{name}: banner is only {} tall",
            banner.rect.height
        );
    }
}

#[test]
fn a_skeleton_occupies_the_space_the_real_rows_will() {
    // The point of a skeleton is that nothing moves when data arrives.
    // Every caller in the tree stands one in front of a list, so the unit
    // it counts is a row, not a line of prose. Set in paragraph lines it
    // was under half the height of what followed it and the screen jumped.
    let lines = 5;
    let skeleton = Screen::new(
        1,
        vec![Node::Skeleton {
            id: NodeId(1),
            lines,
        }],
    )
    .layout();
    let real = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: (0..u32::from(lines))
                .map(|index| {
                    Row::new(
                        ActionId(index + 1),
                        "A headline",
                        "news.example.com",
                        RowLead::Icon(Glyph::News),
                    )
                })
                .collect(),
        }],
    )
    .layout();
    let extent = |layout: &Layout| {
        layout
            .nodes
            .iter()
            .map(|node| node.rect.y + node.rect.height)
            .max()
            .unwrap_or(0)
            - layout.nodes[0].rect.y
    };
    assert_eq!(extent(&skeleton), extent(&real));
}

#[test]
fn skeleton_line_counts_are_clamped_rather_than_trusted() {
    let layout = Screen::new(
        1,
        vec![Node::Skeleton {
            id: NodeId(1),
            lines: 255,
        }],
    )
    .layout();
    assert_eq!(
        layout.nodes[0].rect.height,
        12 * skeleton_band(&CLARA_BW_METRICS) + 11 * skeleton_gap(&CLARA_BW_METRICS)
    );
}

#[test]
fn progress_snaps_to_coarse_steps_so_a_download_cannot_flood_the_panel() {
    // One refresh per percent would be a hundred refreshes per download.
    let distinct = (0..=100)
        .map(|value| Percent::new(value).coarse().get())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), 21);
    assert_eq!(Percent::new(0).coarse().get(), 0);
    assert_eq!(Percent::new(100).coarse().get(), 100);
    assert_eq!(Percent::new(52).coarse().get(), 50);
}

#[test]
fn activity_offers_cancel_as_a_finger_sized_target() {
    let screen = Screen::new(
        1,
        vec![Node::Activity {
            id: NodeId(1),
            label: "Fetching".into(),
            progress: Some(Percent::new(30)),
            cancel: Some(BarAction::new(ActionId(1), "Cancel")),
            transferred: None,
            failure: None,
        }],
    );
    for (name, metrics) in PANELS {
        let layout = screen.layout_for(&metrics);
        let cancel = layout
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::ChoiceFreeform(_)))
            .expect("cancel row");
        assert!(
            cancel.rect.height >= metrics.touch_target_minimum(),
            "{name}: cancel is {} tall",
            cancel.rect.height
        );
        assert_eq!(
            layout.hit_test(cancel.rect.x + 2, cancel.rect.y + 2),
            Some(ActionId(1))
        );
    }
}

#[test]
fn indeterminate_activity_draws_no_bar() {
    // A bar that invents its own position is worse than no bar.
    let layout = Screen::new(
        1,
        vec![Node::Activity {
            id: NodeId(1),
            label: "Connecting".into(),
            progress: None,
            cancel: None,
            transferred: None,
            failure: None,
        }],
    )
    .layout();
    assert!(!layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::ActivityProgress));
}

/// One sample of every node kind, so a new arm is covered by the structural
/// tests below the moment it is added rather than whenever someone
/// remembers.
fn one_of_every_node() -> Vec<Node> {
    vec![
        Node::Heading {
            id: NodeId(1),
            text: "Heading".into(),
            level: 1,
        },
        Node::Text {
            id: NodeId(2),
            text: "Some body text that is long enough to wrap onto a second line.".into(),
            links: Vec::new(),
        },
        Node::Button {
            id: NodeId(3),
            action: ActionId(3),
            label: "Button".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        },
        Node::Card {
            id: NodeId(4),
            children: vec![Node::Text {
                id: NodeId(5),
                text: "Inside a card".into(),
                links: Vec::new(),
            }],
        },
        Node::Divider { id: NodeId(6) },
        Node::Stepper {
            id: NodeId(96),
            label: "120%".into(),
            less: BarAction::new(ActionId(30), String::new()).with_glyph(Glyph::Minus),
            more: BarAction::new(ActionId(31), String::new()).with_glyph(Glyph::Plus),
            less_state: ControlState::Enabled,
            more_state: ControlState::Disabled,
            fill: Some(75),
        },
        Node::Spacer {
            id: NodeId(7),
            space: Space::Medium,
        },
        Node::Progress {
            id: NodeId(8),
            value: Percent::new(40),
        },
        Node::PagedList {
            id: NodeId(9),
            page: 0,
            items: vec!["One".into(), "Two".into()],
        },
        Node::Grid {
            id: NodeId(10),
            columns: 3,
            square: true,
            cells: (0..9)
                .map(|index| Cell::new(ActionId(100 + index), "O"))
                .collect(),
        },
        Node::Rows {
            id: NodeId(11),
            rows: vec![Row::new(ActionId(11), "Row", "Summary", Glyph::App)],
        },
        Node::TileGrid {
            shape: TileShape::Square,
            id: NodeId(12),
            tiles: vec![Tile::new(ActionId(12), "Tile", Glyph::App)],
        },
        Node::Choice {
            id: NodeId(13),
            prompt: "Pick one".into(),
            options: vec![BarAction::new(ActionId(13), "Option")],
            selected: Some(0),
            freeform: Some(Freeform::new(ActionId(14), "Something else")),
        },
        Node::Banner {
            id: NodeId(15),
            level: BannerLevel::Attention,
            text: "Careful".into(),
            action: None,
        },
        Node::Skeleton {
            id: NodeId(16),
            lines: 3,
        },
        Node::Activity {
            id: NodeId(17),
            label: "Working".into(),
            progress: Some(Percent::new(50)),
            cancel: Some(BarAction::new(ActionId(18), "Stop")),
            transferred: None,
            failure: None,
        },
    ]
}

#[test]
fn no_node_kind_lets_the_next_one_land_on_top_of_it() {
    // Every layout arm must return the y it finished at, not the height it
    // consumed. Returning a height silently rewinds the cursor to near the
    // top of the screen, so the following node is drawn over this one, and
    // because hit testing takes the last match a tap then reaches the wrong
    // control. That is exactly how the grid shipped, so the rule is
    // enforced structurally rather than remembered.
    for node in one_of_every_node() {
        let name = format!("{node:?}");
        let name = name.split_whitespace().next().unwrap_or("?").to_owned();
        let screen = Screen::new(
            1,
            vec![
                node,
                Node::Button {
                    id: NodeId(900),
                    action: ActionId(900),
                    label: "After".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                },
            ],
        );
        let layout = screen.layout();
        let after = layout
            .nodes
            .iter()
            .find(|candidate| {
                candidate.kind
                    == LayoutKind::Button(ActionId(900), ControlState::Enabled, Emphasis::Normal)
            })
            .expect("the following button was laid out")
            .rect;
        for other in &layout.nodes {
            if other.kind
                == LayoutKind::Button(ActionId(900), ControlState::Enabled, Emphasis::Normal)
            {
                continue;
            }
            assert!(
                other.rect.y + other.rect.height <= after.y,
                "{name} leaves {:?} overlapping the node after it at {after:?}",
                other.rect
            );
        }
    }
}

#[test]
fn every_tappable_rectangle_is_reachable_by_a_tap_at_its_centre() {
    // Overlapping controls are invisible on a panel that renders both, so
    // the only way to catch them is to ask the hit tester whether each
    // control can still be reached where a finger would land.
    let screen = Screen::new(1, one_of_every_node());
    let layout = screen.layout();
    let targets = layout
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::Button(action, ControlState::Enabled, _)
            | LayoutKind::BarAction(action)
            | LayoutKind::BarGlyph(action, _)
            | LayoutKind::Tile(action, _)
            | LayoutKind::Row(action)
            | LayoutKind::Cell(action, ..)
            | LayoutKind::ChoiceOption(action, _)
            | LayoutKind::StepperControl(action, ControlState::Enabled, _)
            | LayoutKind::ChoiceFreeform(action) => Some((action, node.rect)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!targets.is_empty(), "the sample screen has no controls");
    for (action, rect) in targets {
        let x = rect.x + rect.width / 2;
        let y = rect.y + rect.height / 2;
        assert_eq!(
            layout.hit_test(x, y),
            Some(action),
            "a tap in the middle of {rect:?} did not reach it"
        );
    }
}
