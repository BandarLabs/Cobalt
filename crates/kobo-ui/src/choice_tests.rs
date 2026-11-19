use super::tests::PANELS;
use super::*;

fn choice(options: usize, freeform: bool) -> Screen {
    Screen::new(
        1,
        vec![Node::Choice {
            id: NodeId(1),
            prompt: "How should this be filed?".into(),
            options: (0..options)
                .map(|index| BarAction::new(ActionId(index as u32 + 1), format!("Option {index}")))
                .collect(),
            selected: None,
            freeform: freeform.then(|| Freeform::new(ActionId(99), "Type something else")),
        }],
    )
}

#[test]
fn every_option_is_a_full_width_finger_sized_row() {
    for (name, metrics) in PANELS {
        let layout = choice(4, true).layout_for(&metrics);
        let usable = metrics.width - 2 * metrics.screen_margin();
        for node in &layout.nodes {
            if matches!(
                node.kind,
                LayoutKind::ChoiceOption(_, _) | LayoutKind::ChoiceFreeform(_)
            ) {
                assert_eq!(node.rect.width, usable, "{name}: option is not full width");
                assert!(
                    node.rect.height >= metrics.touch_target_minimum(),
                    "{name}: option is {} tall",
                    node.rect.height
                );
            }
        }
    }
}

#[test]
fn the_answer_already_given_is_the_only_one_marked() {
    let Screen { nodes, .. } = choice(4, false);
    let [Node::Choice {
        id,
        prompt,
        options,
        freeform,
        ..
    }] = &nodes[..]
    else {
        unreachable!("the fixture is one choice")
    };
    let screen = Screen::new(
        1,
        vec![Node::Choice {
            id: *id,
            prompt: prompt.clone(),
            options: options.clone(),
            selected: Some(2),
            freeform: freeform.clone(),
        }],
    );
    let marked = screen
        .layout()
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::ChoiceOption(_, chosen) => Some(chosen),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(marked, vec![false, false, true, false]);
}

#[test]
fn an_answer_beyond_the_options_marks_nothing_rather_than_panicking() {
    let Screen { nodes, .. } = choice(3, false);
    let [Node::Choice { id, options, .. }] = &nodes[..] else {
        unreachable!("the fixture is one choice")
    };
    let screen = Screen::new(
        1,
        vec![Node::Choice {
            id: *id,
            prompt: String::new(),
            options: options.clone(),
            selected: Some(200),
            freeform: None,
        }],
    );
    assert!(screen
        .layout()
        .nodes
        .iter()
        .all(|node| !matches!(node.kind, LayoutKind::ChoiceOption(_, true))));
}

#[test]
fn the_freeform_row_comes_last() {
    let layout = choice(3, true).layout();
    let freeform = layout
        .nodes
        .iter()
        .position(|node| matches!(node.kind, LayoutKind::ChoiceFreeform(_)))
        .expect("freeform");
    let last_option = layout
        .nodes
        .iter()
        .rposition(|node| matches!(node.kind, LayoutKind::ChoiceOption(_, _)))
        .expect("option");
    assert!(freeform > last_option);
}

#[test]
fn options_beyond_the_cap_are_dropped_rather_than_shrunk() {
    // Shrinking rows to fit would produce targets too small to hit, so the
    // node refuses the surplus instead. A longer list is a paged list.
    let layout = choice(MAX_CHOICE_OPTIONS + 4, false).layout();
    let count = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::ChoiceOption(_, _)))
        .count();
    assert_eq!(count, MAX_CHOICE_OPTIONS);
}

#[test]
fn options_do_not_overlap_and_each_reports_its_own_action() {
    let layout = choice(4, true).layout();
    let rows = layout
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                LayoutKind::ChoiceOption(_, _) | LayoutKind::ChoiceFreeform(_)
            )
        })
        .collect::<Vec<_>>();
    for (index, row) in rows.iter().enumerate() {
        for other in rows.iter().skip(index + 1) {
            assert!(row.rect.intersection(other.rect).is_none(), "rows overlap");
        }
        let (LayoutKind::ChoiceOption(expected, _) | LayoutKind::ChoiceFreeform(expected)) =
            row.kind
        else {
            unreachable!("a choice lays out only options and a freeform field")
        };
        assert_eq!(
            layout.hit_test(row.rect.x + 2, row.rect.y + 2),
            Some(expected)
        );
    }
}

#[test]
fn a_single_row_can_be_patched_without_repainting_the_screen() {
    // Selecting an option should cost one small refresh of that row.
    let screen = choice(4, false);
    let layout = screen.layout();
    let rect = layout.rect_of_action(ActionId(2)).expect("row rectangle");
    let full = layout.bounds().expect("bounds");
    assert!(rect.height * 4 < full.height);
}
