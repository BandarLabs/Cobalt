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

#[test]
fn the_three_tones_of_a_word_game_print_three_different_amounts_of_ink() {
    // Solid, shaded and faded tiles are told apart from across the room by
    // how dark the square is, before any letter is read. So the middle of
    // each square, clear of its letter, is checked for the ink it carries.
    let tones = [
        CellTone::Solid,
        CellTone::Shaded,
        CellTone::Faded,
        CellTone::Plain,
    ];
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 4,
            square: true,
            cells: tones
                .iter()
                .enumerate()
                .map(|(index, tone)| Cell::new(ActionId(10 + index as u32), "I").with_tone(*tone))
                .collect(),
        }],
    );
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    let layout = screen.layout_with(&metrics, &chrome);
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(&screen, &metrics, &chrome, &mut surface, None);
    let field = |index: u32| {
        let rect = layout.rect_of_action(ActionId(10 + index)).expect("a tile");
        // A quarter in from the left, halfway down: inside the rule, and
        // clear of a narrow letter set in the middle.
        let x = (rect.x + rect.width / 4) as usize;
        let y = (rect.y + rect.height / 2) as usize;
        surface.pixels[y * surface.width + x]
    };
    assert_eq!(field(0), tone::INK, "a tile in its place is not solid");
    let shaded = field(1);
    assert!(
        shaded > tone::INK + 100 && shaded < tone::PAPER - 50,
        "a tile in the word elsewhere is {shaded}, not a mid grey"
    );
    assert_eq!(
        field(2),
        tone::PAPER,
        "an unused letter's tile is not paper"
    );
    assert_eq!(field(3), tone::PAPER);
    // The toned squares sit exactly where plain ones would, so a board that
    // starts using tones does not move.
    let plain = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 4,
            square: true,
            cells: (0..4)
                .map(|index| Cell::new(ActionId(10 + index), "I"))
                .collect(),
        }],
    );
    let plain = plain.layout_with(&metrics, &chrome);
    for index in 0..4 {
        assert_eq!(
            plain.rect_of_action(ActionId(10 + index)),
            layout.rect_of_action(ActionId(10 + index))
        );
    }
}

fn backgammon(numbered: bool) -> Screen {
    let cells = (0..24)
        .map(|index| {
            let mut cell = Cell::new(ActionId(100 + index), format!("Point {} empty", index + 1));
            if numbered {
                cell.corner = Some(u8::try_from(index + 1).unwrap());
            }
            cell
        })
        .collect();
    Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 12,
            square: true,
            cells,
        }],
    )
}

#[test]
fn a_numbered_backgammon_board_sits_in_a_frame_with_a_bar_and_prints_its_own_numbers() {
    for (_, metrics) in PANELS {
        let chrome = Chrome::measuring(true);
        let screen = backgammon(true);
        let issues = screen.diagnostics(&metrics, &chrome).issues;
        assert!(issues.is_empty(), "{metrics:?}: {issues:?}");
        let layout = screen.layout_with(&metrics, &chrome);
        let frames = layout
            .nodes
            .iter()
            .filter(|node| node.kind == LayoutKind::BackgammonFrame)
            .map(|node| node.rect)
            .collect::<Vec<_>>();
        let (board, bar) = (frames[0], frames[1]);
        assert!(
            board.x > 0 && board.x + board.width < metrics.width,
            "{metrics:?}"
        );
        let point = |index: u32| layout.rect_of_action(ActionId(100 + index)).unwrap();
        // The sixth point ends at the bar and the seventh starts after it.
        assert!(point(5).x + point(5).width <= bar.x);
        assert!(point(6).x >= bar.x + bar.width);
        assert!(point(0).width >= metrics.touch_target_minimum());
        let numbers = layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::PointNumber(_)))
            .count();
        assert_eq!(numbers, 24);
    }
}

#[test]
fn an_unnumbered_backgammon_board_keeps_the_columns_its_pictures_were_drawn_for() {
    // An installed application that draws its own numbers lines them up with
    // twelve equal columns across the whole panel, so that board keeps them.
    let metrics = CLARA_BW_METRICS;
    let layout = backgammon(false).layout_with(&metrics, &Chrome::measuring(true));
    let column = metrics.width / 12;
    for index in 0..12 {
        let rect = layout.rect_of_action(ActionId(100 + index)).unwrap();
        assert_eq!(
            (rect.x, rect.width),
            (i32::try_from(index).unwrap() * column, column)
        );
    }
    assert!(layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::BackgammonBoard));
}
