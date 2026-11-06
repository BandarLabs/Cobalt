use super::*;

/// A reading face whose upright letters are ten pixels wide and whose italic
/// ones are six, so which cut a run was measured in is plain from its width.
struct Leaning;

impl Typesetter for Leaning {
    fn measure(&self, text: &str, _size: FontSize, _face: Face) -> (i32, i32) {
        (10 * i32::try_from(text.chars().count()).unwrap_or(0), 30)
    }

    fn line_height(&self, _size: FontSize, _face: Face) -> i32 {
        30
    }

    fn draw(
        &self,
        _text: &str,
        x: i32,
        y: i32,
        _size: FontSize,
        _face: Face,
        plot: &mut dyn FnMut(i32, i32, u8),
    ) {
        plot(x, y, 255);
    }

    fn has_italic(&self, face: Face) -> bool {
        face == Face::Reading
    }

    fn measure_italic(&self, text: &str, _size: FontSize, _face: Face) -> (i32, i32) {
        (6 * i32::try_from(text.chars().count()).unwrap_or(0), 30)
    }

    fn cap_height(&self, _size: FontSize, _face: Face) -> Option<i32> {
        Some(20)
    }

    fn ascent(&self, _size: FontSize, _face: Face) -> Option<i32> {
        Some(24)
    }

    /// An initial's letters are half as wide as its capitals are tall.
    fn measure_initial(&self, text: &str, _face: Face, cap: i32) -> Option<i32> {
        Some(cap / 2 * i32::try_from(text.chars().count()).unwrap_or(0))
    }
}

/// A handle of its own for each use, because tests run side by side and the
/// faces they install share one table: a test that dropped a shared handle
/// would take the face out from under another one mid-layout.
fn leaning() -> FontHandle {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9_100);
    let handle = FontHandle(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    put_book_typesetter(handle, Box::new(Leaning));
    handle
}

/// A paragraph set entirely in emphasis, as a poem's title or a book's
/// stressed sentence is.
fn emphasised(text: &str) -> Node {
    Node::RichText {
        id: NodeId(1),
        text: text.to_owned(),
        spans: vec![RichTextSpan {
            start: 0,
            end: text.len(),
            presentation: TextPresentation {
                emphasis: true,
                ..TextPresentation::default()
            },
        }],
        links: Vec::new(),
        presentation: ParagraphPresentation::default(),
        selection: None,
        formulae: Vec::new(),
    }
}

/// The widths of the runs a paragraph was set as, line by line in order.
fn run_widths(typography: Typography, text: &str, width: i32) -> Vec<i32> {
    let face = leaning();
    let metrics = DisplayMetrics {
        width,
        ..CLARA_BW_METRICS
    };
    let screen = Screen::new(1, vec![emphasised(text)])
        .with_reading(true)
        .with_reading_font(Some(face))
        .with_typography(typography);
    let widths = screen
        .layout_with(&metrics, &Chrome::with_back(false))
        .nodes
        .iter()
        .filter(|node| {
            matches!(node.kind, LayoutKind::RichText(style) if style.emphasis)
                && !node.text_lines.is_empty()
        })
        .map(|node| node.rect.width)
        .collect();
    drop_book_typesetter(face);
    widths
}

#[test]
fn emphasis_is_measured_in_the_italic_it_is_drawn_in() {
    // Fourteen letters and two spaces: 160 pixels upright, 96 in italic. The
    // column holds 120, so upright it takes two lines and in italic one.
    let text = "aaaa bbbb cccc";
    let margins = 2 * CLARA_BW_METRICS.screen_margin();
    let italic = run_widths(Typography::Fine, text, 120 + margins);
    assert_eq!(italic, vec![6 * 14], "one line, measured in the italic");
}

#[test]
fn an_application_that_measured_upright_keeps_its_upright_measure() {
    let text = "aaaa bbbb cccc";
    let margins = 2 * CLARA_BW_METRICS.screen_margin();
    let upright = run_widths(Typography::Quiet, text, 120 + margins);
    assert_eq!(upright, vec![10 * 9, 10 * 4], "two lines, measured upright");
}

/// Lays one plain paragraph out with the given presentation, in a column
/// `column` pixels wide, and returns its nodes.
fn paragraph(
    typography: Typography,
    text: &str,
    presentation: ParagraphPresentation,
    column: i32,
) -> Vec<LayoutNode> {
    let face = leaning();
    let metrics = DisplayMetrics {
        width: column + 2 * CLARA_BW_METRICS.screen_margin(),
        ..CLARA_BW_METRICS
    };
    let node = Node::RichText {
        id: NodeId(1),
        text: text.to_owned(),
        spans: Vec::new(),
        links: Vec::new(),
        presentation,
        selection: None,
        formulae: Vec::new(),
    };
    let nodes = Screen::new(1, vec![node])
        .with_reading(true)
        .with_reading_font(Some(face))
        .with_typography(typography)
        .layout_with(&metrics, &Chrome::with_back(false))
        .nodes;
    drop_book_typesetter(face);
    nodes
}

/// Where each line of a paragraph starts, from its column's edge.
fn line_starts(nodes: &[LayoutNode]) -> Vec<(i32, String)> {
    let edge = CLARA_BW_METRICS.screen_margin();
    nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::RichText(_)) && !node.text_lines.is_empty())
        .map(|node| (node.rect.x - edge, node.text_lines[0].clone()))
        .collect()
}

#[test]
fn a_block_indent_moves_every_line_and_narrows_the_measure() {
    // The em is the width of "M", ten pixels here, so two ems is twenty. The
    // column is 120, which leaves 100 beside the indent: two words a line.
    let nodes = paragraph(
        Typography::Fine,
        "aaaa bbbb cccc dddd",
        ParagraphPresentation {
            start_indent_em: 200,
            ..ParagraphPresentation::default()
        },
        120,
    );
    assert_eq!(
        line_starts(&nodes),
        vec![(20, "aaaa bbbb".to_owned()), (20, "cccc dddd".to_owned())]
    );
}

#[test]
fn an_initial_is_set_down_into_its_lines_and_they_are_set_beside_it() {
    // Three lines of thirty, capitals twenty tall: the initial's capitals
    // reach from the first line's to the third's baseline, 2 x 30 + 20 = 80
    // tall and so 40 wide, and a word space after it makes 50.
    let nodes = paragraph(
        Typography::Fine,
        "Abbb cccc dddd eeee ffff",
        ParagraphPresentation {
            initial_lines: 3,
            ..ParagraphPresentation::default()
        },
        100,
    );
    let initial = nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Initial { .. }))
        .expect("an initial");
    assert_eq!(initial.text_lines, vec!["A".to_owned()]);
    let LayoutKind::Initial { cap, baseline } = initial.kind else {
        unreachable!()
    };
    assert_eq!(cap, 80);
    assert_eq!(
        baseline,
        2 * 30 + 24,
        "on the third baseline, from its own top"
    );
    assert_eq!(
        line_starts(&nodes),
        vec![
            (50, "bbb".to_owned()),
            (50, "cccc".to_owned()),
            (50, "dddd".to_owned()),
            (0, "eeee ffff".to_owned()),
        ]
    );
}

#[test]
fn an_application_on_an_older_protocol_opens_its_paragraphs_as_it_measured_them() {
    let nodes = paragraph(
        Typography::Quiet,
        "Abbb cccc",
        ParagraphPresentation {
            initial_lines: 3,
            ..ParagraphPresentation::default()
        },
        100,
    );
    assert!(!nodes
        .iter()
        .any(|node| matches!(node.kind, LayoutKind::Initial { .. })));
    assert_eq!(line_starts(&nodes), vec![(0, "Abbb cccc".to_owned())]);
}

#[test]
fn a_poem_is_centred_on_its_longest_line_and_set_from_the_margin_when_one_would_wrap() {
    let face = leaning();
    let indent = with_reading_font(Some(face), || {
        let short: [&[&str]; 2] = [&["aaaa bbbb", "cc"], &["dddddd"]];
        let long: [&[&str]; 1] = [&["aaaa bbbb cccc dddd eeee ffff"]];
        (
            // The longest line is ninety pixels in a column of two hundred,
            // so it stands in fifty-five: five and a half ems of ten.
            verse_indent(&short, 200, ParagraphPresentation::PLAIN, Face::Reading),
            verse_indent(&long, 200, ParagraphPresentation::PLAIN, Face::Reading),
        )
    });
    drop_book_typesetter(face);
    assert_eq!(indent, (550, 0));
}

#[test]
fn italic_around_a_formula_is_measured_in_the_italic_and_the_formula_as_its_picture() {
    let face = leaning();
    let text = "aa xx bb";
    let width = with_reading_font(Some(face), || {
        with_typography(Typography::Fine, || {
            measure_rich_range(
                text,
                0,
                text.len(),
                FontSize::Body,
                Face::Reading,
                &[RichTextSpan {
                    start: 0,
                    end: text.len(),
                    presentation: TextPresentation {
                        emphasis: true,
                        ..TextPresentation::default()
                    },
                }],
                &[InlineFormula {
                    start: 3,
                    end: 5,
                    handle: PictureHandle(1),
                    source: (40, 20),
                }],
                30,
            )
        })
    });
    drop_book_typesetter(face);
    // "aa " and " bb" are three italic letters each, six pixels a letter,
    // and the formula is its forty-pixel picture.
    assert_eq!(width, 6 * 3 + 40 + 6 * 3);
}

#[test]
fn an_initial_keeps_its_accent_with_its_letter() {
    let face = leaning();
    let initial = with_reading_font(Some(face), || {
        with_typography(Typography::Fine, || {
            initial_letter(
                "A\u{301}nd so",
                ParagraphPresentation {
                    initial_lines: 2,
                    ..ParagraphPresentation::PLAIN
                },
                Face::Reading,
                30,
                &[],
            )
        })
    });
    drop_book_typesetter(face);
    assert_eq!(initial.map(|initial| initial.end), Some("A\u{301}".len()));
}

#[test]
fn holding_the_word_an_initial_begins_selects_the_whole_word() {
    let face = leaning();
    let metrics = DisplayMetrics {
        width: 300 + 2 * CLARA_BW_METRICS.screen_margin(),
        ..CLARA_BW_METRICS
    };
    let node = Node::RichText {
        id: NodeId(1),
        text: "Hello world".to_owned(),
        spans: Vec::new(),
        links: Vec::new(),
        presentation: ParagraphPresentation {
            initial_lines: 2,
            ..ParagraphPresentation::PLAIN
        },
        selection: Some(TextSelection {
            context: 7,
            offset: 0,
        }),
        formulae: Vec::new(),
    };
    let layout = Screen::new(1, vec![node])
        .with_reading(true)
        .with_reading_font(Some(face))
        .with_typography(Typography::Fine)
        .layout_with(&metrics, &Chrome::with_back(false));
    drop_book_typesetter(face);
    let initial = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Initial { .. }))
        .expect("an initial");
    let hello = layout
        .text_hits
        .iter()
        .find(|(_, hit)| hit.start == 0)
        .expect("a hit for the opening word");
    assert_eq!(hello.1.end, 5, "the whole of Hello");
    assert_eq!(hello.0.x, initial.rect.x, "from the initial's own edge");
}
