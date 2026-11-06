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

#[test]
fn a_sowing_board_stands_each_store_through_both_rows_and_names_its_owner_under_it() {
    let mut cells = Vec::new();
    for index in 0..16_u32 {
        let label = match index {
            0 => "Store Computer 2".to_owned(),
            15 => "Store You 1".to_owned(),
            7 | 8 => String::new(),
            _ => "4".to_owned(),
        };
        cells.push(Cell::new(ActionId(200 + index), label));
    }
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 8,
            square: true,
            cells,
        }],
    );
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    assert!(screen.diagnostics(&metrics, &chrome).issues.is_empty());
    let layout = screen.layout_with(&metrics, &chrome);
    let rect = |index: u32| layout.rect_of_action(ActionId(200 + index));
    let (far, near, pit) = (rect(0).unwrap(), rect(15).unwrap(), rect(1).unwrap());
    assert_eq!(far.height, pit.height * 2);
    assert_eq!((far.y, near.y), (pit.y, pit.y));
    assert!(pit.width >= metrics.touch_target_minimum());
    // The places the stores fill are neither drawn nor tapped.
    assert!(rect(7).is_none() && rect(8).is_none());
    let owners = layout
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::StoreLabel)
        .map(|node| node.text_lines.join(" "))
        .collect::<Vec<_>>();
    assert_eq!(owners, ["Computer", "You"]);
}

#[test]
fn a_reversi_board_is_one_ruled_grid_and_a_legal_square_is_a_grey_dot() {
    let cells = (0..64_u32)
        .map(|index| {
            let cell = Cell::new(ActionId(300 + index), "");
            match index {
                27 | 36 => cell.with_glyph(Glyph::WhiteDisc),
                28 | 35 => cell.with_glyph(Glyph::BlackDisc),
                19 => cell.with_glyph(Glyph::LegalPoint),
                _ => cell,
            }
        })
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 8,
            square: true,
            cells,
        }],
    );
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    let layout = screen.layout_with(&metrics, &chrome);
    let first = layout.rect_of_action(ActionId(300)).unwrap();
    let second = layout.rect_of_action(ActionId(301)).unwrap();
    assert_eq!(second.x, first.x + first.width, "the squares do not meet");
    assert!(layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::RuledBoard));
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(&screen, &metrics, &chrome, &mut surface, None);
    let centre = |index: u32| {
        let rect = layout.rect_of_action(ActionId(300 + index)).unwrap();
        let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        surface.pixels[y as usize * surface.width + x as usize]
    };
    assert_eq!(centre(28), tone::INK, "a black disc is solid");
    assert_eq!(centre(27), tone::PAPER, "a white disc is a ring");
    let dot = centre(19);
    assert!(
        dot > tone::INK && dot < tone::PAPER,
        "a legal square is {dot}, not grey"
    );
}

#[test]
fn noughts_and_crosses_is_drawn_as_a_hash_with_no_box_round_it() {
    let cells = (0..9_u32)
        .map(|index| {
            let cell = Cell::new(ActionId(400 + index), "");
            match index {
                0 => cell.with_glyph(Glyph::Circle),
                4 => cell.with_glyph(Glyph::Close),
                _ => cell,
            }
        })
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 3,
            square: true,
            cells,
        }],
    );
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    let layout = screen.layout_with(&metrics, &chrome);
    let first = layout.rect_of_action(ActionId(400)).unwrap();
    let second = layout.rect_of_action(ActionId(401)).unwrap();
    assert_eq!(second.x, first.x + first.width, "the squares do not meet");
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(&screen, &metrics, &chrome, &mut surface, None);
    let pixel = |x: i32, y: i32| surface.pixels[y as usize * surface.width + x as usize];
    // A rule between the first two squares, and none along the outer edge.
    let middle = first.y + first.height / 2;
    assert_eq!(pixel(first.x + first.width - 1, middle), tone::INK);
    assert_eq!(pixel(first.x, first.y + first.height / 4), tone::PAPER);
    assert_eq!(pixel(first.x + first.width / 4, first.y), tone::PAPER);
}

fn chess_board(marked: u32) -> Screen {
    let cells = (0..64_u32)
        .map(|index| {
            let cell = Cell::new(ActionId(500 + index), "");
            let cell = if index == 0 {
                cell.with_glyph(Glyph::ChessBlackRook)
            } else {
                cell
            };
            cell.with_selected(index == marked)
        })
        .collect();
    Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 8,
            square: true,
            cells,
        }],
    )
}

#[test]
fn a_chess_board_is_light_and_dark_squares_edge_to_edge_inside_one_thin_frame() {
    let screen = chess_board(18);
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    let layout = screen.layout_with(&metrics, &chrome);
    let frames: Vec<_> = layout
        .nodes
        .iter()
        .filter(|node| node.kind == LayoutKind::CheckeredFrame)
        .collect();
    assert_eq!(frames.len(), 1, "a board has one frame");
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(&screen, &metrics, &chrome, &mut surface, None);
    let pixel = |x: i32, y: i32| surface.pixels[y as usize * surface.width + x as usize];
    let square = |index: u32| layout.rect_of_action(ActionId(500 + index)).unwrap();
    let Paint::Grey(dark) = Palette::grey().square_dark() else {
        panic!("a grey panel's dark square is grey");
    };
    // Squares 27 and 28 sit side by side in the middle of the board, light
    // then dark, and 27 and 35 one above the other. Across either boundary
    // the panel goes straight from one field to the other with no rule.
    let (light, right, below) = (square(27), square(28), square(35));
    assert_eq!(right.x, light.x + light.width, "the squares do not meet");
    let middle = light.y + light.height / 2;
    for x in light.x + light.width / 2..light.x + light.width {
        assert_eq!(
            pixel(x, middle),
            tone::PAPER,
            "a rule inside a light square"
        );
    }
    for x in right.x..right.x + right.width / 2 {
        assert_eq!(pixel(x, middle), dark, "a rule inside a dark square");
    }
    let column = light.x + light.width / 2;
    assert_eq!(pixel(column, below.y - 1), tone::PAPER);
    assert_eq!(pixel(column, below.y), dark);
    // The frame closes the outside of the board in ink, and only the frame:
    // a rule's width in from the edge is the square's own field again.
    let edge = square(8);
    let rule = metrics.rule_thickness();
    let across = edge.y + edge.height / 2;
    assert_eq!(pixel(edge.x, across), tone::INK, "no frame");
    assert_eq!(pixel(edge.x + rule, across), dark, "the frame is not thin");
    // A marked square keeps a heavy ink outline inside its own edges, and
    // its neighbour keeps its field right up to the boundary.
    let marked = square(18);
    let across = marked.y + marked.height / 2;
    assert_eq!(pixel(marked.x + rule * 2 - 1, across), tone::INK);
    assert_eq!(pixel(marked.x - 1, across), dark);
    assert_eq!(pixel(marked.x + marked.width / 4, across), tone::PAPER);
}

#[test]
fn only_chess_and_draughts_boards_are_drawn_without_rules_between_squares() {
    let board = |columns: u8, cells: Vec<Cell>| {
        Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(1),
                columns,
                square: true,
                cells,
            }],
        )
    };
    let empty = |count: u32| -> Vec<Cell> {
        (0..count)
            .map(|index| Cell::new(ActionId(700 + index), ""))
            .collect()
    };
    let with = |count: u32, glyph: Glyph| -> Vec<Cell> {
        let mut cells = empty(count);
        cells[0] = cells[0].clone().with_glyph(glyph);
        cells
    };
    let digits: Vec<Cell> = (0..81_u32)
        .map(|index| Cell::new(ActionId(700 + index), (index % 9 + 1).to_string()))
        .collect();
    let mut crossword = empty(25);
    for cell in &mut crossword {
        cell.corner = Some(1);
    }
    let covered: Vec<Cell> = (0..81_u32)
        .map(|index| Cell::new(ActionId(700 + index), "").with_selected(index != 0))
        .collect();
    let others = [
        ("reversi", board(8, with(64, Glyph::BlackDisc))),
        ("sudoku", board(9, digits)),
        ("crossword", board(5, crossword)),
        ("minesweeper", board(9, covered)),
        ("morris", board(7, with(49, Glyph::BoardPoint))),
        ("noughts and crosses", board(3, with(9, Glyph::Circle))),
    ];
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    for (name, screen) in others {
        let layout = screen.layout_with(&metrics, &chrome);
        assert!(
            !layout.nodes.iter().any(|node| matches!(
                node.kind,
                LayoutKind::CheckeredFrame
                    | LayoutKind::Cell(_, CellStyle::BoardLight | CellStyle::BoardDark, _)
            )),
            "{name} was drawn as a chess board"
        );
    }
    let draughts = board(8, with(64, Glyph::WhiteDraughtsMan)).layout_with(&metrics, &chrome);
    assert!(draughts
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::CheckeredFrame));
}
