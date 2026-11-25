use super::*;

/// Paints a screen on the black and white Clara and hands back its layout
/// with the pixels.
fn painted(screen: &Screen) -> (Layout, Surface) {
    let metrics = CLARA_BW_METRICS;
    let chrome = Chrome::measuring(true);
    let layout = screen.layout_with(&metrics, &chrome);
    let mut surface = Surface::new(metrics.width as usize, metrics.height as usize);
    render_with(screen, &metrics, &chrome, &mut surface, None);
    (layout, surface)
}

fn pixel(surface: &Surface, x: i32, y: i32) -> u8 {
    surface.pixels[y as usize * surface.width + x as usize]
}

/// A point on the stroke's line a fifth of the way up from the lower left
/// corner, clear of a letter or digit set in the middle of the square.
fn on_the_stroke(rect: Rect) -> (i32, i32) {
    (rect.x + rect.width / 5, rect.y + rect.height * 4 / 5)
}

/// A small crossword: the first square in the shaded word, the second not,
/// both numbered, so either can be struck.
fn crossword(first: CellTone, second: CellTone) -> Screen {
    let cell = |action: u32, letter: &str, corner: u8, selected: bool, tone: CellTone| {
        let mut cell = Cell::new(ActionId(action), letter)
            .with_selected(selected)
            .with_tone(tone);
        cell.corner = Some(corner);
        cell
    };
    Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 2,
            square: true,
            cells: vec![
                cell(10, "Q", 1, true, first),
                cell(11, "Z", 2, false, second),
                cell(12, "A", 3, true, CellTone::Plain),
                cell(13, "B", 4, false, CellTone::Plain),
            ],
        }],
    )
}

#[test]
fn a_struck_crossword_letter_keeps_its_square_letter_and_number_and_gains_a_stroke() {
    let (plain_layout, plain) = painted(&crossword(CellTone::Plain, CellTone::Plain));
    let (layout, struck) = painted(&crossword(CellTone::Struck, CellTone::Struck));
    // Nothing moves: every node of the grid sits where it did, so marking a
    // checked word is an ink change and never a layout change.
    assert_eq!(layout.nodes.len(), plain_layout.nodes.len());
    for (node, before) in layout.nodes.iter().zip(&plain_layout.nodes) {
        assert_eq!(node.rect, before.rect);
        assert_eq!(node.text_lines, before.text_lines);
    }
    for action in [10, 11] {
        let rect = layout.rect_of_action(ActionId(action)).expect("a square");
        let (x, y) = on_the_stroke(rect);
        assert_eq!(
            pixel(&struck, x, y),
            tone::INK,
            "square {action} is not struck"
        );
        assert_ne!(
            pixel(&plain, x, y),
            tone::INK,
            "square {action} was ink already"
        );
        // The upper left quarter holds the clue number, and the stroke runs
        // the other way, so it is untouched.
        for y in rect.y..rect.y + rect.height / 4 {
            for x in rect.x..rect.x + rect.width / 4 {
                assert_eq!(pixel(&struck, x, y), pixel(&plain, x, y));
            }
        }
    }
    // The squares that were not struck are the pixels they always were.
    for action in [12, 13] {
        let rect = layout.rect_of_action(ActionId(action)).expect("a square");
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                assert_eq!(pixel(&struck, x, y), pixel(&plain, x, y));
            }
        }
    }
}

#[test]
fn a_struck_tone_on_a_word_game_tile_draws_the_plain_tile() {
    let tiles = |tone: CellTone| {
        Screen::new(
            1,
            vec![Node::Grid {
                id: NodeId(1),
                columns: 4,
                square: true,
                cells: (0..4)
                    .map(|index| Cell::new(ActionId(20 + index), "I").with_tone(tone))
                    .collect(),
            }],
        )
    };
    assert_eq!(
        painted(&tiles(CellTone::Struck)).1.pixels,
        painted(&tiles(CellTone::Plain)).1.pixels
    );
}

/// One row of three squares on a sudoku-sized board: an entered digit, the
/// same digit marked wrong, and a wrong digit in the selected square.
fn digits(second: PencilMarkKind, third: PencilMarkKind) -> Screen {
    let mark = |column: u8, kind: PencilMarkKind, selected: bool| PencilMark {
        column,
        row: 0,
        kind,
        action: Some(ActionId(30 + u32::from(column))),
        selected,
        peer: false,
    };
    Screen::new(
        1,
        vec![Node::PencilBoard {
            id: NodeId(1),
            board: PencilBoard {
                columns: 3,
                rows: 1,
                cell_tenth_mm: 80,
                marks: vec![
                    mark(
                        0,
                        PencilMarkKind::Digit {
                            value: 5,
                            given: false,
                        },
                        false,
                    ),
                    mark(1, second, false),
                    mark(2, third, true),
                ],
                edges: Vec::new(),
            },
        }],
    )
}

#[test]
fn a_wrong_digit_is_struck_through_in_ink_and_reversed_out_of_the_selected_square() {
    let entered = PencilMarkKind::Digit {
        value: 5,
        given: false,
    };
    let wrong = PencilMarkKind::WrongDigit(5);
    let (plain_layout, plain) = painted(&digits(entered, entered));
    let (layout, struck) = painted(&digits(wrong, wrong));
    // Laid out as the entries were, and named as wrong where a square is
    // read out.
    assert_eq!(layout.nodes.len(), plain_layout.nodes.len());
    for (node, before) in layout.nodes.iter().zip(&plain_layout.nodes) {
        assert_eq!(node.rect, before.rect);
        if before.text_lines == ["Row 1, column 2, digit 5"] {
            assert_eq!(node.text_lines, ["Row 1, column 2, digit 5, wrong"]);
        } else if before.text_lines != ["Row 1, column 3, digit 5"] {
            assert_eq!(node.text_lines, before.text_lines);
        }
    }
    let rect = |action: u32| layout.rect_of_action(ActionId(action)).expect("a square");
    let (x, y) = on_the_stroke(rect(31));
    assert_eq!(pixel(&struck, x, y), tone::INK);
    assert_eq!(pixel(&plain, x, y), tone::PAPER);
    // The selected square is the board's one filled control, so its stroke
    // is paper, as its digit is.
    let (x, y) = on_the_stroke(rect(32));
    assert_eq!(pixel(&struck, x, y), tone::PAPER);
    assert_ne!(pixel(&plain, x, y), tone::PAPER);
    // The digit is still there, in the grey of an entry, and the square
    // beside it is untouched.
    let first = rect(30);
    for y in first.y..first.y + first.height {
        for x in first.x..first.x + first.width {
            assert_eq!(pixel(&struck, x, y), pixel(&plain, x, y));
        }
    }
    let middle = rect(31);
    assert!(
        layout.nodes.iter().any(|node| {
            node.kind == LayoutKind::PencilWrong(false)
                && node.text_lines == ["5"]
                && middle.contains(node.rect.x, node.rect.y)
        }),
        "the wrong digit is not set as a struck entry"
    );
}
