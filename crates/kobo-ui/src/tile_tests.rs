use super::tests::PANELS;
use super::*;

fn grid(count: u32) -> Screen {
    Screen::new(
        1,
        vec![Node::TileGrid {
            shape: TileShape::Square,
            id: NodeId(1),
            tiles: (0..count)
                .map(|index| Tile::new(ActionId(index + 1), format!("App {index}"), Glyph::App))
                .collect(),
        }],
    )
}

#[test]
fn tiles_never_exceed_the_panels_column_budget() {
    for (name, metrics) in PANELS {
        let layout = grid(9).layout_for(&metrics);
        let tops = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .map(|node| node.rect.y)
            .collect::<Vec<_>>();
        let first_row = tops.iter().filter(|top| **top == tops[0]).count();
        assert!(
            first_row <= metrics.grid_columns(TileShape::Square),
            "{name}: {first_row} tiles on a row, budget is {}",
            metrics.grid_columns(TileShape::Square)
        );
    }
}

#[test]
fn tile_rows_sit_no_further_apart_than_the_grid_sits_below_the_bar() {
    // A cell is much taller than the mark inside it, so a gap that is
    // right between two columns reads as roughly twice as much air between
    // two rows. On the panel the grid began one tight step under the top
    // bar's rule and then left two of them between its own rows, and the
    // second and third rows looked like a separate screen.
    for (name, metrics) in PANELS {
        let screen = grid(9).with_top_bar(TopBar::new(NodeId(9), "Cobalt"));
        let layout = screen.layout_for(&metrics);
        let mut tops = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .map(|node| (node.rect.y, node.rect.height))
            .collect::<Vec<_>>();
        tops.sort_unstable();
        tops.dedup();
        let rule = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::Divider)
            .map(|node| node.rect.y + node.rect.height)
            .expect("a rule under the bar");
        let above = tops[0].0 - rule;
        for pair in tops.windows(2) {
            let [(top, height), (next, _)] = pair else {
                continue;
            };
            let between = next - (top + height);
            assert!(
                between <= above,
                "{name}: {between}px between rows but only {above}px above the first"
            );
        }
    }
}

#[test]
fn every_tile_is_large_enough_to_tap_on_every_panel() {
    for (name, metrics) in PANELS {
        let layout = grid(6).layout_for(&metrics);
        for node in &layout.nodes {
            if matches!(node.kind, LayoutKind::Tile(..)) {
                assert!(
                    node.rect.width >= metrics.touch_target_minimum()
                        && node.rect.height >= metrics.touch_target_minimum(),
                    "{name}: tile is {}x{}",
                    node.rect.width,
                    node.rect.height
                );
            }
        }
    }
}

#[test]
fn tiles_do_not_overlap_each_other() {
    for (name, metrics) in PANELS {
        let layout = grid(7).layout_for(&metrics);
        let rects = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
            .map(|node| node.rect)
            .collect::<Vec<_>>();
        for (index, rect) in rects.iter().enumerate() {
            for other in rects.iter().skip(index + 1) {
                assert!(
                    rect.intersection(*other).is_none(),
                    "{name}: two tiles overlap"
                );
            }
        }
    }
}

#[test]
fn a_tile_name_is_drawn_no_wider_than_the_width_it_was_wrapped_to() {
    // A shelf of books is a grid of covers with real titles under them,
    // and a real title takes two lines. Those two lines were wrapped at
    // caption size and drawn at body size, so each one came out wider than
    // its cell and ran into the book beside it. Whatever the painter picks
    // has to be what the text was measured with.
    for (name, metrics) in PANELS {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                shape: TileShape::Portrait,
                id: NodeId(1),
                tiles: [
                    "Frankenstein; or, the modern prometheus",
                    "Alice's Adventures in Wonderland",
                    "Moby Dick; Or, The Whale",
                    "Crime and Punishment",
                ]
                .iter()
                .enumerate()
                .map(|(index, title)| {
                    Tile::new(ActionId(index as u32 + 1), (*title).to_owned(), Glyph::Book)
                })
                .collect(),
            }],
        );
        let layout = screen.layout_for(&metrics);
        let labels = layout
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::TileLabel)
            .collect::<Vec<_>>();
        assert!(!labels.is_empty(), "{name}: no tile carried a name");
        assert!(
            labels.iter().any(|node| node.text_lines.len() > 1),
            "{name}: no name wrapped, so this proves nothing"
        );
        for node in labels {
            let size = tile_label_size(node.rect, node.text_lines.len(), !legacy_typography());
            for line in &node.text_lines {
                let (width, _) = measure_text(line, size);
                assert!(
                    width <= node.rect.width,
                    "{name}: {line:?} is drawn {width} wide in a {} wide cell",
                    node.rect.width
                );
            }
        }
    }
}

#[test]
fn tapping_a_tile_returns_that_tiles_action() {
    let screen = grid(4);
    let layout = screen.layout();
    for node in &layout.nodes {
        if let LayoutKind::Tile(action, _) = node.kind {
            assert_eq!(
                layout.hit_test(node.rect.x + 2, node.rect.y + 2),
                Some(action)
            );
        }
    }
}

#[test]
fn a_grid_stays_inside_the_screen_margins() {
    for (name, metrics) in PANELS {
        let layout = grid(5).layout_for(&metrics);
        for node in &layout.nodes {
            if matches!(node.kind, LayoutKind::Tile(..)) {
                assert!(
                    node.rect.x >= metrics.screen_margin()
                        && node.rect.x + node.rect.width <= metrics.width - metrics.screen_margin(),
                    "{name}: a tile runs into the margin"
                );
            }
        }
    }
}

#[test]
fn an_empty_grid_occupies_no_space() {
    let layout = grid(0).layout();
    assert!(!layout
        .nodes
        .iter()
        .any(|node| matches!(node.kind, LayoutKind::Tile(..))));
}
