/// A stacked value is written with the heading it sat under.
#[test]
fn a_stacked_value_keeps_the_heading_it_sat_under() {
    let labels = vec!["BigToM".to_owned(), "Hi-ToM".to_owned()];
    assert_eq!(
        super::stacked_cell(&labels, 1, 0, "0.803").as_deref(),
        Some("BigToM: 0.803"),
        "a bare number says nothing about which column it came from"
    );
    assert_eq!(
        super::stacked_cell(&labels, 0, 0, "BigToM"),
        None,
        "the heading row is read as written"
    );
    assert_eq!(
        super::stacked_cell(&labels, 1, 9, "0.803"),
        None,
        "a column with no heading has nothing to say"
    );
}

/// The row that names columns is words; the rows under it are numbers.
#[test]
fn a_row_of_numbers_is_not_mistaken_for_a_row_of_headings() {
    let words = ["Scaffold builder", "BigToM", "Hi-ToM", "Avg."]
        .map(str::to_owned)
        .to_vec();
    assert!(
        super::row_names_the_columns(&words),
        "a row of names names the columns"
    );
    let numbers = ["6", "0.803", "0.842", "+0.387"]
        .map(str::to_owned)
        .to_vec();
    assert!(
        !super::row_names_the_columns(&numbers),
        "a table whose top row is already data has no headings to give"
    );
}

/// A narrow column keeps the digits it asked for.
///
/// The eight columns of a paper's results table each wanted less than a
/// sentence; squeezed in proportion they all fell under their own content
/// width and the table stacked. What a column asked for is the most it can
/// need, so no column is ever given less than that.
#[test]
fn a_narrow_column_is_never_squeezed_under_what_it_asked_for() {
    let wants = [351, 23, 125, 121, 132, 104, 238, 121];
    let (widths, stacked) = super::table_column_widths(&wants, 1000, 142);
    assert!(!stacked, "every column fits in 1000: {widths:?}");
    for (column, (width, want)) in widths.iter().zip(&wants).enumerate() {
        assert!(
            *width >= (*want).min(142),
            "column {column} wanted {want} and was given {width}"
        );
    }
}

/// The room left over goes to the column that wanted it.
#[test]
fn the_spare_room_goes_to_the_widest_column() {
    let (widths, stacked) = super::table_column_widths(&[400, 40, 40], 300, 100);
    assert!(!stacked, "three columns fitting should not stack");
    assert_eq!(
        widths.iter().sum::<i32>(),
        300,
        "the room is all shared out"
    );
    assert!(
        widths[0] > widths[1] && widths[0] > widths[2],
        "the sentence should take the slack, not the numbers: {widths:?}"
    );
    assert_eq!(
        (widths[1], widths[2]),
        (40, 40),
        "a column four characters wide needs no more than four characters"
    );
}

/// A table stacks only when even the least widths do not fit.
#[test]
fn a_table_stacks_only_when_the_least_widths_do_not_fit() {
    let (_, roomy) = super::table_column_widths(&[300, 300, 300], 330, 100);
    assert!(!roomy, "three columns of 100 fit in 330");
    let (_, cramped) = super::table_column_widths(&[300, 300, 300], 290, 100);
    assert!(cramped, "three columns of 100 do not fit in 290");
}
use super::*;

#[test]
fn reading_quotes_validate_in_their_reading_face_and_scale() {
    let screen = Screen::new(
        1,
        vec![Node::Quote {
            id: NodeId(1),
            depth: 1,
            role: QuoteRole::Body,
            text: "A good walk leaves room to notice small details along the river. ".repeat(3),
            fold: None,
        }],
    )
    .with_reading(true)
    .with_text_scale(Some(TextScale::Default));
    let metrics = DisplayMetrics {
        text_scale: TextScale::Largest,
        ..CLARA_BW_METRICS
    };
    let _environment = environment::TextEnvironment::enter(&screen, &metrics);
    let layout = screen.layout_with(&metrics, &Chrome::default());
    let mut issues = Vec::new();
    validate_layout_nodes(&layout, &metrics, &mut issues);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn publisher_styled_text_keeps_alignment_and_inline_emphasis() {
    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: "ordinary strong".into(),
            spans: vec![RichTextSpan {
                start: 9,
                end: 15,
                presentation: TextPresentation {
                    strong: true,
                    emphasis: true,
                    ..TextPresentation::default()
                },
            }],
            links: Vec::new(),
            presentation: ParagraphPresentation {
                alignment: ParagraphAlignment::Center,
                line_height_percent: 140,
                ..ParagraphPresentation::default()
            },
            selection: None,
            formulae: Vec::new(),
        }],
    )
    .with_reading(true);
    let layout = screen.layout();
    assert!(layout.nodes.iter().any(|node| matches!(
        node.kind,
        LayoutKind::RichText(TextPresentation {
            strong: true,
            emphasis: true,
            ..
        })
    )));
    let first = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::RichText(_)))
        .expect("rich run");
    assert!(first.rect.x > CLARA_BW_METRICS.screen_margin());
}

#[test]
fn a_held_unicode_word_resolves_to_stable_document_offsets() {
    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: "naïve café".into(),
            spans: Vec::new(),
            links: Vec::new(),
            presentation: ParagraphPresentation::default(),
            selection: Some(TextSelection {
                context: 19,
                offset: 100,
            }),
            formulae: Vec::new(),
        }],
    )
    .with_reading(true)
    .with_hold(ActionId(7));
    let layout = screen.layout();
    let (rect, expected) = layout.text_hits.last().expect("selectable word");
    assert_eq!(
        layout.hit_text(rect.x + rect.width / 2, rect.y + rect.height / 2),
        Some(*expected)
    );
    assert_eq!(
        *expected,
        TextHit {
            context: 19,
            start: 107,
            end: 112,
        }
    );
}

#[test]
fn a_cell_glyph_and_a_bottom_action_glyph_both_reach_the_layout() {
    // Both builders hand a glyph to a layout function that used to ignore
    // it: the cell payload had no glyph at all, and layout_bottom_action
    // read only the label. A builder whose argument is silently dropped is
    // worse than no builder, because the call site looks correct.
    let cells = vec![
        Cell::new(ActionId(11), "Back 30 sec").with_glyph(Glyph::Rewind30),
        Cell::new(ActionId(12), "Play").with_glyph(Glyph::Play),
    ];
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 2,
            square: false,
            cells,
        }],
    )
    .with_bottom_action(BottomAction::new(
        NodeId(2),
        BarAction::new(ActionId(20), "Bluetooth audio output").with_glyph(Glyph::Bluetooth),
    ));
    let layout = screen.layout();
    let drawn: Vec<Glyph> = layout
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::InlineGlyph(glyph, _) => Some(glyph),
            _ => None,
        })
        .collect();
    assert_eq!(
        drawn,
        vec![Glyph::Rewind30, Glyph::Play, Glyph::Bluetooth],
        "a glyph was accepted by a builder and never drawn"
    );
    // The mark is decoration. Every one of them must sit inside a control
    // that owns the tap, or a reader who hits the icon gets nothing.
    for node in layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::InlineGlyph(..)))
    {
        assert!(
            layout.nodes.iter().any(|other| {
                matches!(other.kind, LayoutKind::Cell(..) | LayoutKind::Button(..))
                    && other.rect.intersection(node.rect) == Some(node.rect)
            }),
            "a glyph at {:?} sits outside every control",
            node.rect
        );
    }
}

#[test]
fn an_empty_terminal_waiting_for_output_is_not_hidden_content() {
    let screen = Screen::new(
        1,
        vec![Node::Terminal {
            id: NodeId(1),
            rows: vec![],
            cursor: None,
        }],
    );
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
    let (columns, rows) = terminal_grid_for(&screen, &CLARA_BW_METRICS);
    assert!(
        columns > 0 && rows > 0,
        "empty terminals must still negotiate a grid"
    );
}

/// A ten-key row on a narrow panel turns each cell taller than it is
/// wide -- the same rectangle a Sudoku digit or a tic-tac-toe mark draws
/// in -- and a rule that read the cell's own shape once took that as "this
/// is a mark" and grew it to heading size, so the qwertyuiop row of a real
/// keyboard came out bold and oversized while the row below it, nine keys
/// wide and so a little shorter than it is tall, did not.
#[test]
fn a_keyboard_key_never_becomes_a_board_mark_however_narrow_its_row() {
    let cells = ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"]
        .into_iter()
        .enumerate()
        .map(|(index, letter)| Cell::new(ActionId(index as u32 + 1), letter))
        .collect();
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 10,
            square: false,
            cells,
        }],
    );
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let labels: Vec<_> = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::CellLabel(_)))
        .collect();
    assert_eq!(labels.len(), 10, "every key should have drawn a label");
    for node in labels {
        assert_eq!(
            node.kind,
            LayoutKind::CellLabel(false),
            "a keyboard key at {:?} was drawn as though it were a board mark",
            node.rect
        );
    }
}

/// The rule the test above guards against still has to fire for what it
/// was written for: a short mark on an actual board.
#[test]
fn short_board_marks_can_fit_below_the_current_caption_scale_without_shrinking_words() {
    let edge = with_text_scale(TextScale::Default, || FontSize::Caption.line_height());
    with_text_scale(TextScale::Largest, || {
        let mut node = LayoutNode {
            id: NodeId(1),
            rect: Rect {
                x: 0,
                y: 0,
                width: edge,
                height: edge,
            },
            kind: LayoutKind::CellLabel(true),
            text_lines: vec!["8".into()],
        };
        let (size, scale) = board_label_style(&node);
        assert_ne!(scale, TextScale::Largest);
        assert!(with_text_scale(scale, || size.line_height() <= edge));
        node.text_lines = vec!["Long label".into()];
        assert_eq!(board_label_style(&node).1, TextScale::Largest);
    });
}

#[test]
fn board_labels_use_the_largest_semantic_size_that_fits_the_square() {
    for scale in TextScale::STEPS {
        for ppi in [212, 300] {
            let metrics = DisplayMetrics {
                pixels_per_inch: ppi,
                text_scale: scale,
                ..CLARA_BW_METRICS
            };
            let _environment =
                environment::TextEnvironment::enter(&Screen::new(1, vec![]), &metrics);
            {
                for label in ["1", "9", "4\u{0332}", "□", "·"] {
                    let edge = metrics.touch_target_minimum();
                    let node = LayoutNode {
                        id: NodeId(1),
                        rect: Rect {
                            x: 0,
                            y: 0,
                            width: edge,
                            height: edge,
                        },
                        kind: LayoutKind::CellLabel(true),
                        text_lines: vec![label.into()],
                    };
                    let size = board_label_size(&node);
                    assert!(
                        measure_text(label, size).0 <= edge,
                        "{scale:?} {ppi} {label}"
                    );
                    assert!(size.line_height() <= edge);
                    assert_eq!(layout_text_style(&node).unwrap().0, size);
                }
            }
        }
    }
}

#[test]
fn a_mark_on_a_board_still_grows_to_heading_size() {
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 3,
            square: true,
            cells: vec![Cell::new(ActionId(1), "X")],
        }],
    );
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let label = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::CellLabel(_)))
        .expect("the mark drew a label");
    assert_eq!(label.kind, LayoutKind::CellLabel(true));
}

#[test]
fn wrapping_and_measurement_are_deterministic() {
    assert_eq!(measure_text("AB", FontSize::Body), (36, 21));
    assert_eq!(
        wrap_text("one two three", 90, FontSize::Body),
        vec!["one", "two", "three"]
    );
}

#[test]
fn text_scale_has_stable_names_and_wire_values() {
    assert_eq!(TextScale::from_name("large"), Some(TextScale::Large));
    assert_eq!(TextScale::from_name("140%"), Some(TextScale::ExtraLarge));
    assert_eq!(TextScale::from_wire(1), Some(TextScale::Large));
    assert_eq!(TextScale::from_wire(9), None);
    assert_eq!(TextScale::ExtraLarge.percent(), 140);
}

#[test]
fn validation_reports_content_that_layout_would_hide() {
    let nodes = (0..80)
        .map(|index| Node::Text {
            id: NodeId(index + 1),
            text: "A paragraph that occupies a real line.".into(),
            links: Vec::new(),
        })
        .collect();
    let issues = Screen::new(1, nodes).validate(&CLARA_BW_METRICS);
    assert!(issues.iter().any(|issue| matches!(
        issue.kind,
        LayoutIssueKind::ContentOverflow { hidden_nodes } if hidden_nodes > 0
    )));
}

#[test]
fn validation_reports_truncation_and_clamps_grid_targets() {
    let screen = Screen::new(
        1,
        vec![
            Node::Choice {
                id: NodeId(1),
                prompt: "Choose".into(),
                options: (0..=MAX_CHOICE_OPTIONS)
                    .map(|index| BarAction::new(ActionId(index as u32 + 1), "Option"))
                    .collect(),
                selected: None,
                freeform: None,
            },
            Node::Grid {
                id: NodeId(2),
                columns: MAX_COLUMNS,
                square: false,
                cells: vec![Cell::new(ActionId(20), "1")],
            },
        ],
    );
    let issues = screen.validate(&PANELS[1].1);
    assert!(issues.iter().any(|issue| matches!(
        issue.kind,
        LayoutIssueKind::CollectionTruncated {
            collection: "choice options",
            ..
        }
    )));
    assert!(!issues
        .iter()
        .any(|issue| matches!(issue.kind, LayoutIssueKind::TouchTargetTooSmall { .. })));
}

/// A formula set into a sentence is drawn where its words were, at the
/// size it was handed over, and the words underneath it are not drawn
/// twice.
#[test]
fn a_formula_in_a_sentence_is_drawn_in_place_of_the_words_it_stands_for() {
    let text = "the constant K_G is small.";
    let start = text.find("K_G").expect("the formula's words");
    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: text.to_owned(),
            spans: Vec::new(),
            links: Vec::new(),
            presentation: ParagraphPresentation::default(),
            selection: None,
            formulae: vec![InlineFormula {
                start,
                end: start + "K_G".len(),
                handle: PictureHandle(3),
                source: (60, 30),
            }],
        }],
    );
    let layout = screen.layout();
    let picture = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Picture(PictureHandle(3))))
        .expect("the formula should have been laid out");
    assert_eq!((picture.rect.width, picture.rect.height), (60, 30));
    // The words are still in the paragraph -- a search and a selection
    // read them -- but nothing draws them, or they would show through.
    let drawn: String = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RichText(_) | LayoutKind::Text))
        .flat_map(|node| node.text_lines.iter())
        .cloned()
        .collect();
    assert!(!drawn.contains("K_G"), "the words were drawn too: {drawn}");
    assert!(
        drawn.contains("the constant"),
        "the sentence was lost: {drawn}"
    );
    assert!(
        drawn.contains("is small."),
        "the sentence was lost: {drawn}"
    );
}

/// A link that follows a formula on the same line is where the reader can
/// see it, not where the words the formula stands for used to end.
///
/// A formula takes the width of its picture rather than the width of its
/// TeX, and a tap target measured against the written form is out of place
/// by the difference -- so a reader taps a citation and the page does
/// nothing, or follows the reference beside it.
#[test]
fn a_link_after_a_formula_is_where_the_formula_leaves_it() {
    let text = "at K_G see the note.";
    let start = text.find("K_G").expect("the formula's words");
    let note = text.find("the note").expect("the link's words");
    let link = TextLink {
        action: ActionId(7),
        start: note,
        end: note + "the note".len(),
    };
    let target = |formulae: Vec<InlineFormula>| {
        let screen = Screen::new(
            1,
            vec![Node::RichText {
                id: NodeId(1),
                text: text.to_owned(),
                spans: Vec::new(),
                links: vec![link],
                presentation: ParagraphPresentation::default(),
                selection: None,
                formulae,
            }],
        );
        screen
            .layout()
            .nodes
            .iter()
            .find(|node| matches!(node.kind, LayoutKind::InlineLink(ActionId(7))))
            .expect("the link should have been laid out")
            .rect
            .x
    };

    // Far wider than the three characters it is set in place of, so the
    // words after it are pushed along and the mistake is visible.
    let written = target(Vec::new());
    let drawn = target(vec![InlineFormula {
        start,
        end: start + "K_G".len(),
        handle: PictureHandle(3),
        source: (240, 30),
    }]);
    assert!(
        drawn > written,
        "the formula did not move the words after it, so this proves nothing"
    );

    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: text.to_owned(),
            spans: Vec::new(),
            links: vec![link],
            presentation: ParagraphPresentation::default(),
            selection: None,
            formulae: vec![InlineFormula {
                start,
                end: start + "K_G".len(),
                handle: PictureHandle(3),
                source: (240, 30),
            }],
        }],
    );
    let layout = screen.layout();
    let words = layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RichText(_)))
        .find(|node| node.text_lines.iter().any(|line| line.contains("the note")))
        .expect("the words after the formula should have been drawn");
    let target = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::InlineLink(ActionId(7))))
        .expect("the link should have been laid out")
        .rect;
    assert!(
        target.x >= words.rect.x,
        "the link was left behind the words it covers: {target:?} against {:?}",
        words.rect
    );
    assert!(
        target.x < words.rect.x.saturating_add(words.rect.width),
        "the link fell past the words it covers: {target:?} against {:?}",
        words.rect
    );
}

/// A formula whose offsets land inside a character lays the paragraph out
/// anyway instead of taking the application down.
///
/// A paper is written in a great many characters that take more than one
/// byte, and every offset here indexes bytes. Panicking on a bad one does
/// not fail politely on a reader: the process dies, the panel goes back to
/// the stock software, and the sentence somebody was reading goes with it.
#[test]
fn a_formula_offset_inside_a_character_does_not_take_the_page_down() {
    // Every one of these is two bytes, so every odd offset is inside a
    // character and none of them are anywhere a formula should begin.
    let text = "\u{3c0}\u{3c0}\u{3c0} and \u{3c0}\u{3c0}\u{3c0}";
    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: text.to_owned(),
            spans: Vec::new(),
            links: Vec::new(),
            presentation: ParagraphPresentation::default(),
            selection: None,
            formulae: vec![InlineFormula {
                start: 1,
                end: 5,
                handle: PictureHandle(4),
                source: (40, 20),
            }],
        }],
    );
    let layout = screen.layout();
    assert!(
        !layout.nodes.is_empty(),
        "the paragraph was dropped entirely"
    );
    let drawn: String = layout
        .nodes
        .iter()
        .flat_map(|node| node.text_lines.iter())
        .cloned()
        .collect();
    assert!(drawn.contains("and"), "the sentence was lost: {drawn}");
}

/// A formula takes the width of its picture rather than the width of the
/// words it stands for, and lines have to break on the width that is
/// actually drawn or the paragraph runs into the margin.
#[test]
fn a_wide_formula_pushes_the_line_it_will_not_fit_on() {
    let text = "aaa bbb X ccc ddd";
    let start = text.find('X').expect("the formula's words");
    let paragraph = |source| {
        Screen::new(
            1,
            vec![Node::RichText {
                id: NodeId(1),
                text: text.to_owned(),
                spans: Vec::new(),
                links: Vec::new(),
                presentation: ParagraphPresentation::default(),
                selection: None,
                formulae: vec![InlineFormula {
                    start,
                    end: start + 1,
                    handle: PictureHandle(3),
                    source,
                }],
            }],
        )
        .layout()
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RichText(_) | LayoutKind::Text))
        .map(|node| node.rect.y)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
    };
    assert!(
        paragraph((4_000, 30)) > paragraph((20, 30)),
        "a formula wider than the page did not break the line"
    );
}

/// An illustration wants an edge and a formula does not, and the two
/// have to be distinguishable after layout, because that is the only
/// place the renderer looks.
#[test]
fn a_picture_asked_for_without_a_frame_lays_out_without_one() {
    let picture = |framed| {
        Screen::new(
            1,
            vec![Node::Picture {
                id: NodeId(1),
                handle: PictureHandle(7),
                source: (10, 10),
                max_height_tenths_mm: 100,
                framed,
                bleed: false,
            }],
        )
        .layout()
        .nodes
        .into_iter()
        .find_map(|node| match node.kind {
            LayoutKind::Picture(_) | LayoutKind::FramedPicture(_) => Some(node.kind),
            _ => None,
        })
        .expect("the picture should have been laid out")
    };
    assert!(matches!(picture(true), LayoutKind::FramedPicture(_)));
    assert!(matches!(picture(false), LayoutKind::Picture(_)));
}

#[test]
fn large_pictures_fit_remaining_space_without_hiding_controls() {
    for (width, height, pixels_per_inch) in [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
    {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics {
                width,
                height,
                pixels_per_inch,
                text_scale,
            };
            let screen = Screen::new(
                1,
                vec![
                    Node::Picture {
                        id: NodeId(1),
                        handle: PictureHandle(1),
                        source: (1072, 1448),
                        max_height_tenths_mm: 5000,
                        framed: false,
                        bleed: false,
                    },
                    Node::Grid {
                        id: NodeId(2),
                        columns: 1,
                        square: false,
                        cells: vec![Cell::new(ActionId(2), "Open")],
                    },
                ],
            )
            .with_top_bar(TopBar::new(NodeId(3), "Photograph"));
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                diagnostics.issues.is_empty(),
                "{metrics:?}: {:?}",
                diagnostics.issues
            );
            let picture = diagnostics
                .layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Picture(_)))
                .expect("picture");
            assert!(picture.rect.width > 0 && picture.rect.height > 0);
            assert!(
                (i64::from(picture.rect.width) * 1448 - i64::from(picture.rect.height) * 1072)
                    .abs()
                    < 3000,
                "aspect ratio"
            );
            assert!(diagnostics.layout.rect_of_action(ActionId(2)).is_some());
        }
    }
}

#[test]
fn an_empty_game_grid_is_not_hidden_content() {
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 8,
            square: true,
            cells: vec![],
        }],
    );
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::measuring(true))
        .issues
        .is_empty());
}

#[test]
fn validation_can_distinguish_a_missing_picture_from_layout() {
    let screen = Screen::new(
        1,
        vec![Node::Picture {
            id: NodeId(1),
            handle: PictureHandle(7),
            source: (10, 10),
            max_height_tenths_mm: 100,
            framed: true,
            bleed: false,
        }],
    );
    let diagnostics = screen.diagnostics_with_pictures(
        &CLARA_BW_METRICS,
        &Chrome::default(),
        &PictureCache::default(),
    );
    assert!(diagnostics.issues.iter().any(|issue| matches!(
        issue.kind,
        LayoutIssueKind::MissingPicture(PictureHandle(7))
    )));
}

/// Panels this SDK is expected to reach eventually. None of them is
/// supported by the hardware gate yet; they exist here so the design
/// system is exercised against real densities rather than one device.
pub(super) const PANELS: [(&str, DisplayMetrics); 5] = [
    ("clara-bw", CLARA_BW_METRICS),
    (
        "nia",
        DisplayMetrics {
            width: 758,
            height: 1024,
            pixels_per_inch: 212,
            text_scale: TextScale::Default,
        },
    ),
    (
        "libra-2",
        DisplayMetrics {
            width: 1264,
            height: 1680,
            pixels_per_inch: 300,
            text_scale: TextScale::Default,
        },
    ),
    (
        "sage",
        DisplayMetrics {
            width: 1440,
            height: 1920,
            pixels_per_inch: 300,
            text_scale: TextScale::Default,
        },
    ),
    (
        "elipsa",
        DisplayMetrics {
            width: 1404,
            height: 1872,
            pixels_per_inch: 227,
            text_scale: TextScale::Default,
        },
    ),
];

/// The whole point of measuring in millimetres: a touch target is the same
/// physical size everywhere, even though the pixel count differs a lot.
#[test]
fn a_touch_target_is_seven_millimetres_on_every_panel() {
    for (name, metrics) in PANELS {
        let pixels = metrics.touch_target_minimum();
        let tenths = pixels * 254 / metrics.pixels_per_inch;
        assert!(
            (69..=71).contains(&tenths),
            "{name}: {pixels}px is {tenths} tenths of a millimetre, not 70"
        );
    }
    // Concretely: the same seven millimetres is 83 pixels on a 300 pixel
    // per inch panel and 58 on a 212 one. A shared pixel constant could
    // not be right for both.
    assert_eq!(CLARA_BW_METRICS.touch_target_minimum(), 83);
    assert_eq!(PANELS[1].1.touch_target_minimum(), 58);
}

#[test]
fn column_counts_follow_physical_width_rather_than_resolution() {
    // The Nia has far fewer pixels than the Clara but is the same physical
    // width, so it must get the same layout.
    let clara = CLARA_BW_METRICS;
    let nia = PANELS[1].1;
    assert!((clara.width_tenth_mm() - nia.width_tenth_mm()).abs() <= 20);
    assert_eq!(clara.max_grid_columns(), nia.max_grid_columns());
    assert_eq!(clara.max_grid_columns(), 2);

    // A ten inch panel is wide enough for a third column.
    assert_eq!(PANELS[4].1.max_grid_columns(), 3);

    for (name, metrics) in PANELS {
        let columns = metrics.max_grid_columns();
        assert!((1..=4).contains(&columns), "{name} asked for {columns}");
        let column_width = metrics.width_tenth_mm() / columns as i32;
        assert!(
            column_width >= 450,
            "{name}: {column_width} tenths per column is too narrow to read"
        );
    }
}

#[test]
fn every_panel_gets_a_usable_navigation_bar_and_visible_rules() {
    for (name, metrics) in PANELS {
        let destinations = metrics.max_nav_destinations();
        assert!(
            (MIN_NAV_DESTINATIONS..=5).contains(&destinations),
            "{name} allowed {destinations} destinations"
        );
        // Every destination has to remain at least a finger wide.
        let usable = metrics.width - 2 * metrics.screen_margin();
        assert!(usable / destinations as i32 >= metrics.touch_target_minimum());
        // A rule has to survive rounding to at least one whole pixel.
        assert!(metrics.rule_thickness() >= 1, "{name} rule vanished");
    }
}

#[test]
fn the_spacing_scale_is_ordered_and_never_negative() {
    for (name, metrics) in PANELS {
        let steps = [Space::Tight, Space::Small, Space::Medium, Space::Large]
            .map(|space| metrics.space(space));
        assert!(steps[0] > 0, "{name} tight spacing vanished");
        assert!(
            steps.windows(2).all(|pair| pair[0] < pair[1]),
            "{name} spacing is not ordered: {steps:?}"
        );
    }
}

#[test]
fn a_percentage_can_never_exceed_a_hundred() {
    assert_eq!(Percent::new(0).get(), 0);
    assert_eq!(Percent::new(100).get(), 100);
    assert_eq!(Percent::new(101).get(), 100);
    assert_eq!(Percent::new(u8::MAX).get(), 100);
}

#[test]
fn button_hit_testing_respects_touch_target() {
    let screen = Screen::new(
        7,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(2),
            label: "Increment".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    );
    let button = screen.layout().nodes[0].rect;
    assert!(button.height >= CLARA_BW_METRICS.touch_target_minimum());
    assert_eq!(
        screen.hit_test(button.x + 1, button.y + 1),
        Some(ActionId(2))
    );
    assert_eq!(screen.hit_test(0, 0), None);
}

#[test]
fn a_disabled_button_is_visible_but_not_tappable() {
    let screen = Screen::new(
        7,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(2),
            label: "Unavailable".into(),
            state: ControlState::Disabled,
            emphasis: Emphasis::Normal,
        }],
    );
    let layout = screen.layout();
    let button = &layout.nodes[0];
    assert_eq!(
        button.kind,
        LayoutKind::Button(ActionId(2), ControlState::Disabled, Emphasis::Normal)
    );
    assert_eq!(screen.hit_test(button.rect.x + 1, button.rect.y + 1), None);
}

#[test]
fn a_disabled_button_absorbs_the_tap_rather_than_turning_the_page() {
    // A greyed-out control answering with somebody else's action is worse
    // than one that does nothing at all.
    let screen = Screen::new(
        8,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(2),
            label: "Unavailable".into(),
            state: ControlState::Disabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_page_turns(ActionId(10), ActionId(11));
    let layout = screen.layout();
    let button = layout.nodes[0].rect;
    assert_eq!(screen.hit_test(button.x + 1, button.y + 1), None);
    // Content the button does not cover still turns the page.
    assert_eq!(
        screen.hit_test(
            layout.content.x + layout.content.width - 1,
            button.y + button.height + 1
        ),
        Some(ActionId(11))
    );
}

#[test]
fn renderer_writes_grayscale_pixels() {
    let screen = Screen::new(
        1,
        vec![Node::Heading {
            id: NodeId(1),
            text: "Hi".into(),
            level: 1,
        }],
    );
    let mut surface = Surface::new(128, 128);
    render(&screen, &mut surface, None);
    assert!(surface.pixels.contains(&tone::INK));
}

#[test]
fn dirty_render_leaves_other_pixels_untouched() {
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(1),
            label: "Go".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    );
    let metrics = DisplayMetrics {
        width: 300,
        height: 300,
        pixels_per_inch: 300,
        text_scale: TextScale::Default,
    };
    let mut surface = Surface::new(300, 300);
    surface.clear(77);
    // On the button's left edge, halfway down it: the corners are rounded
    // now, so a point near one is outside the shape and would prove
    // nothing either way.
    let rect = screen
        .layout_for(&metrics)
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("a button")
        .rect;
    let (x, y) = (rect.x, rect.y + rect.height / 2);
    render_with(
        &screen,
        &metrics,
        &Chrome::default(),
        &mut surface,
        Some(Rect {
            x,
            y,
            width: 1,
            height: 1,
        }),
    );
    let at =
        |x: i32, y: i32| surface.pixels[usize::try_from(y * metrics.width + x).expect("inside")];
    assert_eq!(at(x, y), tone::INK);
    assert_eq!(at(0, 0), 77);
}

#[test]
fn an_outlined_box_is_drawn_at_the_panel_rule_thickness() {
    // A one pixel outline is 0.08 millimetres on this panel, and at the
    // light tone an outline uses it is close to invisible. This is why
    // every ruled box looked washed out on the device while dividers,
    // which have always used the real rule thickness, looked right.
    let screen = Screen::new(
        1,
        vec![Node::Card {
            id: NodeId(1),
            children: vec![Node::Heading {
                id: NodeId(2),
                text: "Card".into(),
                level: 1,
            }],
        }],
    );
    let card = screen
        .layout()
        .nodes
        .into_iter()
        .find(|node| node.kind == LayoutKind::Card)
        .expect("the card was laid out");
    let stride = usize::try_from(CLARA_BW_METRICS.width).expect("a positive width");
    let mut surface = Surface::new(
        stride,
        usize::try_from(CLARA_BW_METRICS.height).expect("a positive height"),
    );
    surface.clear(tone::PAPER);
    render(&screen, &mut surface, None);

    let thickness = CLARA_BW_METRICS.rule_thickness();
    assert!(thickness > 1, "a rule thinner than this proves nothing");
    let column = usize::try_from(card.rect.x + card.rect.width / 2).expect("inside the panel");
    let mut drawn = 0;
    for offset in 0..thickness {
        let row = usize::try_from(card.rect.y + offset).expect("inside the panel");
        if surface.pixels[row * stride + column] == tone::RULE {
            drawn += 1;
        }
    }
    assert_eq!(
        drawn, thickness,
        "the top edge of a card is {drawn} pixels rather than {thickness}"
    );
    let below = usize::try_from(card.rect.y + thickness).expect("inside the panel");
    assert_eq!(
        surface.pixels[below * stride + column],
        tone::SURFACE,
        "the border ran past the rule thickness into the card itself"
    );
}

#[test]
fn frame_planner_matches_the_panel_waveform_rules() {
    let mut planner = FramePlanner::new(8, 4);
    let mut frame = Surface::new(8, 4);
    let first = planner.plan(&frame).expect("first frame refreshes");
    assert_eq!(first.waveform, PanelWaveform::Gc16);
    assert!(first.full);
    assert!(planner.commit(&frame, first));
    assert!(planner.plan(&frame).is_none(), "unchanged frame refreshes");

    frame.pixels[2 * 8 + 3] = tone::INK;
    let black_and_white = planner.plan(&frame).expect("one changed pixel");
    assert_eq!(black_and_white.waveform, PanelWaveform::Du);
    assert_eq!(
        black_and_white.region,
        Rect {
            x: 3,
            y: 2,
            width: 1,
            height: 1,
        }
    );
    assert!(planner.commit(&frame, black_and_white));

    frame.pixels[2 * 8 + 3] = tone::MUTED;
    let grey = planner.plan(&frame).expect("grey changed");
    assert_eq!(grey.waveform, PanelWaveform::Gl16);
    assert!(planner.commit(&frame, grey));

    frame.pixels[0] = tone::INK;
    let grey_outside_change = planner.plan(&frame).expect("black pixel changed");
    assert_eq!(grey_outside_change.waveform, PanelWaveform::Du);
}

#[test]
fn a_colour_picture_plans_a_colour_update_and_grey_over_it_does_not() {
    let mut planner = FramePlanner::new(8, 4);
    let mut frame = Surface::new(8, 4);
    let first = planner.plan(&frame).expect("first frame refreshes");
    assert!(planner.commit(&frame, first));
    assert!(frame.chroma.is_none(), "a grey frame has no colour plane");

    // One red pixel: the changed region is that pixel and it is colour.
    frame.blend_colour(3, 2, [200, 20, 20], 255);
    assert_eq!(frame.pixels[2 * 8 + 3], luma([200, 20, 20]));
    let colour = planner.plan(&frame).expect("colour changed");
    assert_eq!(colour.waveform, PanelWaveform::Colour);
    assert!(!colour.full);
    assert_eq!(
        colour.region,
        Rect {
            x: 3,
            y: 2,
            width: 1,
            height: 1
        }
    );
    assert!(planner.commit(&frame, colour));
    assert!(planner.plan(&frame).is_none(), "unchanged frame refreshes");

    // A different colour with the same luminance is still a change: the
    // grey plane alone would have missed it.
    let same_luma = [20, 200, 20];
    let candidate = [0u8, 255, 0];
    let green = if luma(candidate) == luma([200, 20, 20]) {
        candidate
    } else {
        same_luma
    };
    frame.blend_colour(3, 2, green, 255);
    let recoloured = planner.plan(&frame).expect("hue changed");
    assert_eq!(recoloured.waveform, PanelWaveform::Colour);
    assert!(planner.commit(&frame, recoloured));

    // Grey elsewhere is planned as grey: colour on the panel does not
    // make every later update a colour one.
    frame.fill_rect(
        Rect {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
        },
        tone::INK,
    );
    let ink = planner.plan(&frame).expect("ink changed");
    assert_eq!(ink.waveform, PanelWaveform::Du);
    assert!(planner.commit(&frame, ink));

    // Painting grey over the colour pixel is a grey update of that pixel,
    // and the frame no longer holds colour.
    frame.fill_rect(
        Rect {
            x: 3,
            y: 2,
            width: 1,
            height: 1,
        },
        tone::MUTED,
    );
    assert!(!frame.has_colour());
    let covered = planner.plan(&frame).expect("colour covered");
    assert_eq!(covered.waveform, PanelWaveform::Gl16);
    assert!(planner.commit(&frame, covered));

    // And a cleared frame drops the plane altogether.
    frame.clear(tone::PAPER);
    assert!(frame.chroma.is_none());
}

#[test]
fn a_cleaning_refresh_over_colour_is_planned_in_colour() {
    let mut planner = FramePlanner::new(2, 1);
    let mut frame = Surface::new(2, 1);
    let first = planner.plan(&frame).expect("first");
    assert!(planner.commit(&frame, first));
    frame.blend_colour(1, 0, [10, 90, 200], 255);
    let colour = planner.plan(&frame).expect("colour");
    assert!(planner.commit(&frame, colour));
    // Each flip repaints one of the two pixels, so the budget of eight
    // panels' worth is sixteen flips.
    for index in 0..=2 * PANEL_CLEAN_INTERVAL {
        frame.fill_rect(
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            if index % 2 == 0 {
                tone::INK
            } else {
                tone::PAPER
            },
        );
        let update = planner.plan(&frame).expect("update");
        assert!(planner.commit(&frame, update));
        if update.full {
            assert_eq!(update.waveform, PanelWaveform::Colour);
            assert_eq!(update.region.width, 2);
            return;
        }
        assert_eq!(update.waveform, PanelWaveform::Du);
    }
    panic!("the panel was never cleaned");
}

#[test]
fn colour_and_grey_planes_describe_one_picture() {
    let mut frame = Surface::new(4, 1);
    frame.blend_colour(0, 0, [255, 0, 0], 255);
    frame.blend_colour(1, 0, [0, 255, 0], 255);
    frame.fill_rect(
        Rect {
            x: 2,
            y: 0,
            width: 1,
            height: 1,
        },
        40,
    );
    frame.blend(3, 0, 0, 128);
    let chroma = frame.chroma.as_ref().expect("colour drawn");
    assert_eq!(&chroma[6..9], &[40, 40, 40]);
    assert_eq!(chroma[9], chroma[10]);
    assert_eq!(chroma[10], chroma[11]);
    for index in 0..4 {
        assert_eq!(
            frame.pixels[index],
            luma(frame.rgb_at(index).expect("inside")),
            "pixel {index}"
        );
    }
    frame.invert_rect(Rect {
        x: 0,
        y: 0,
        width: 4,
        height: 1,
    });
    assert_eq!(frame.rgb_at(0), Some([0, 255, 255]));
    assert_eq!(frame.rgb_at(2), Some([215, 215, 215]));
    assert_eq!(frame.pixels[2], 215);
    assert!(frame.region_has_colour(Rect {
        x: 0,
        y: 0,
        width: 2,
        height: 1
    }));
    assert!(!frame.region_has_colour(Rect {
        x: 2,
        y: 0,
        width: 2,
        height: 1
    }));
    let rows: Option<Vec<&[u8]>> = frame
        .colour_rows(Rect {
            x: 1,
            y: 0,
            width: 2,
            height: 1,
        })
        .map(Iterator::collect);
    assert_eq!(rows, Some(vec![&[255, 0, 255, 215, 215, 215][..]]));
    assert!(
        frame
            .colour_rows(Rect {
                x: 3,
                y: 0,
                width: 2,
                height: 1,
            })
            .is_none(),
        "a region past the edge is refused"
    );
    for v in [0_u8, 1, 17, 137, 254, 255] {
        assert_eq!(luma([v, v, v]), v, "grey survives the round trip");
    }
}

#[test]
fn frame_planner_cleans_after_eight_panels_worth_of_repainting() {
    // Eight full-panel repaints still clean on the eighth, which is what a
    // run of page turns does. That behaviour is the one being preserved.
    let mut planner = FramePlanner::new(2, 1);
    let mut frame = Surface::new(2, 1);
    let first = planner.plan(&frame).expect("first");
    assert!(planner.commit(&frame, first));
    for index in 0..PANEL_CLEAN_INTERVAL {
        let tone = if index % 2 == 0 {
            tone::INK
        } else {
            tone::PAPER
        };
        frame.pixels[0] = tone;
        frame.pixels[1] = tone;
        let partial = planner.plan(&frame).expect("partial");
        assert!(!partial.full, "repaint {index} should not flash");
        assert!(planner.commit(&frame, partial));
    }
    frame.pixels[0] = tone::MUTED;
    let cleaning = planner.plan(&frame).expect("cleaning refresh");
    assert_eq!(cleaning.waveform, PanelWaveform::Gc16);
    assert!(cleaning.full);
    assert_eq!(cleaning.region.width, 2);
    assert_eq!(cleaning.dirty, 0, "the budget resets when the panel clears");
}

#[test]
fn typing_does_not_flash_the_panel_every_few_keystrokes() {
    // The reported fault: entering one address made the panel go black and
    // back several times. A keystroke changes a word and a key, so on a
    // real panel it is a rounding error against the cleaning budget, and
    // far more than eight of them must fit before anything flashes.
    let mut planner = FramePlanner::new(64, 64);
    let mut frame = Surface::new(64, 64);
    frame.clear(tone::PAPER);
    let first = planner.plan(&frame).expect("first");
    assert!(planner.commit(&frame, first));
    let mut flashes = 0;
    for keystroke in 0..64 {
        // One character somewhere near the top, one key near the bottom.
        frame.pixels[keystroke % 64] = tone::INK;
        frame.pixels[63 * 64 + keystroke % 64] = tone::INK;
        let update = planner.plan(&frame).expect("keystroke");
        if update.full {
            flashes += 1;
        }
        assert!(planner.commit(&frame, update));
    }
    assert_eq!(
        flashes, 0,
        "64 keystrokes flashed the panel {flashes} times"
    );
}

#[test]
fn the_budget_is_spent_on_pixels_that_moved_not_on_the_box_around_them() {
    // A keystroke's changed box spans the panel, from the text at the top
    // to the key at the bottom. Charging the box rather than the pixels is
    // exactly what made typing flash, so the two must stay different.
    let mut planner = FramePlanner::new(32, 32);
    let mut frame = Surface::new(32, 32);
    frame.clear(tone::PAPER);
    let first = planner.plan(&frame).expect("first");
    assert!(planner.commit(&frame, first));
    frame.pixels[0] = tone::INK;
    frame.pixels[32 * 32 - 1] = tone::INK;
    let update = planner.plan(&frame).expect("two corners");
    assert_eq!(
        (update.region.width, update.region.height),
        (32, 32),
        "the controller is asked to repaint the whole box between them"
    );
    assert_eq!(update.dirty, 2, "but only two pixels are charged for");
}

#[test]
fn a_box_smaller_than_its_own_border_is_still_a_box() {
    // Nothing lays out a two pixel card, but a clamped thickness is what
    // stops one being filled solid rather than outlined, and the clamp is
    // cheaper to test than to reason about.
    let mut surface = Surface::new(8, 8);
    surface.clear(tone::PAPER);
    let rect = Rect {
        x: 1,
        y: 1,
        width: 2,
        height: 2,
    };
    stroke_clipped(
        &mut surface,
        rect,
        tone::INK,
        99,
        Rect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
    );
    assert_eq!(surface.pixels[8 + 1], tone::INK);
    assert_eq!(
        surface.pixels[0],
        tone::PAPER,
        "the border escaped its rect"
    );
    assert_eq!(surface.pixels[4 * 8 + 4], tone::PAPER);
}

#[test]
fn extreme_layout_values_are_bounded() {
    let mut node = Node::Spacer {
        id: NodeId(1),
        space: Space::Large,
    };
    for id in 2..40 {
        node = Node::Card {
            id: NodeId(id),
            children: vec![node],
        };
    }
    let screen = Screen::new(1, vec![node]);
    let layout = screen.layout();
    assert!(layout.nodes.len() <= MAX_LAYOUT_NODES);
    assert!(layout.nodes.len() <= MAX_LAYOUT_DEPTH + 2);
    let mut surface = Surface::new(128, 128);
    render(&screen, &mut surface, None);
    assert_eq!(surface.pixels.len(), 128 * 128);
}
