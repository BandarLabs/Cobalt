use super::tests::PANELS;
use super::*;

fn list(count: u32, summary: &str) -> Screen {
    Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: (0..count)
                .map(|index| {
                    Row::new(
                        ActionId(index + 1),
                        format!("Entry {index}"),
                        summary.to_owned(),
                        Glyph::App,
                    )
                })
                .collect(),
        }],
    )
}

fn rects(layout: &Layout) -> Vec<Rect> {
    layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Row(_)))
        .map(|node| node.rect)
        .collect()
}

#[test]
fn every_row_is_large_enough_to_tap_on_every_panel() {
    for (name, metrics) in PANELS {
        let layout = list(4, "A short summary.").layout_for(&metrics);
        for rect in rects(&layout) {
            assert!(
                rect.height >= metrics.touch_target_minimum(),
                "{name}: row is only {} tall",
                rect.height
            );
        }
    }
}

#[test]
fn rows_never_overlap_each_other() {
    for (name, metrics) in PANELS {
        let layout = list(
            6,
            "A summary long enough to wrap onto a second line on any panel we support.",
        )
        .layout_for(&metrics);
        let rects = rects(&layout);
        for pair in rects.windows(2) {
            assert!(
                pair[0].y + pair[0].height <= pair[1].y,
                "{name}: a row starting at {} overlaps the one ending at {}",
                pair[1].y,
                pair[0].y + pair[0].height
            );
        }
    }
}

#[test]
fn a_tap_anywhere_in_a_row_chooses_that_row() {
    let metrics = CLARA_BW_METRICS;
    let layout = list(3, "Something to read.").layout_for(&metrics);
    for (index, rect) in rects(&layout).iter().enumerate() {
        let expected = ActionId(index as u32 + 1);
        for (x, y) in [
            (rect.x + 1, rect.y + 1),
            (rect.x + rect.width / 2, rect.y + rect.height / 2),
            (rect.x + rect.width - 2, rect.y + rect.height - 2),
        ] {
            assert_eq!(
                layout.hit_test(x, y),
                Some(expected),
                "tapping {x},{y} did not choose row {index}"
            );
        }
    }
}

#[test]
fn the_summary_is_actually_shown() {
    // The launcher carried a summary for every entry and drew none of them.
    let layout = list(2, "The part that explains the entry.").layout_for(&CLARA_BW_METRICS);
    let summaries = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RowSummary))
        .count();
    assert_eq!(summaries, 2);
}

#[test]
fn an_entry_without_a_summary_still_lays_out() {
    let layout = list(2, "").layout_for(&CLARA_BW_METRICS);
    assert_eq!(rects(&layout).len(), 2);
    assert!(!layout
        .nodes
        .iter()
        .any(|node| matches!(node.kind, LayoutKind::RowSummary)));
}

#[test]
fn no_rule_is_drawn_after_the_last_row() {
    // A trailing rule collided with the divider the launcher drew next.
    let layout = list(3, "Summary.").layout_for(&CLARA_BW_METRICS);
    let rules = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RowRule))
        .count();
    assert_eq!(rules, 2, "three rows need two separators");
}

#[test]
fn a_rule_between_rows_starts_where_the_text_does() {
    // Full width they were one more identical line per row, indexed to the
    // panel rather than to the list, and the screen read as ruled paper.
    let layout = list(3, "Summary.").layout_for(&CLARA_BW_METRICS);
    let rule = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RowRule))
        .expect("a separator");
    let title = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RowTitle))
        .expect("a title");
    assert_eq!(
        rule.rect.x, title.rect.x,
        "the separator did not skip the lead column"
    );
    assert!(
        rule.rect.x + rule.rect.width <= CLARA_BW_METRICS.width,
        "the separator ran off the panel"
    );
}

#[test]
fn tiles_pack_more_entries_than_rows_and_rows_say_more_about_each() {
    // The trade between the two primitives, asserted rather than
    // described. A tile is sized for an icon and a name, so nine of them
    // fit in less height than nine rows; a row spends that height on the
    // summary, which is text a tile has nowhere to put. Getting this
    // backwards is how a launcher ends up showing four enormous buttons.
    let metrics = CLARA_BW_METRICS;
    let summary = "A one line summary of the entry.";
    let rows = list(9, summary).layout_for(&metrics);
    let tiles = Screen::new(
        1,
        vec![Node::TileGrid {
            shape: TileShape::Square,
            id: NodeId(1),
            tiles: (0..9)
                .map(|index| Tile::new(ActionId(index + 1), "Entry", Glyph::App))
                .collect(),
        }],
    )
    .layout_for(&metrics);
    let said = |layout: &Layout| {
        layout
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(said(&rows).contains("summary"), "a row drops its summary");
    assert!(!said(&tiles).contains("summary"), "a tile grew a summary");
    let (Some(rows), Some(tiles)) = (rows.bounds(), tiles.bounds()) else {
        panic!("both layouts should have bounds");
    };
    assert!(
        tiles.height < rows.height,
        "rows took {} and tiles took {}",
        rows.height,
        tiles.height
    );
}

#[test]
fn the_list_length_is_bounded() {
    let metrics = DisplayMetrics {
        height: 8_000,
        ..CLARA_BW_METRICS
    };
    let layout = list(MAX_ROWS as u32 + 10, "Summary.").layout_for(&metrics);
    assert_eq!(rects(&layout).len(), MAX_ROWS);
}
