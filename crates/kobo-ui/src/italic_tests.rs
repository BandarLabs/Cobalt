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
}

const LEANING: FontHandle = FontHandle(91);

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
    put_book_typesetter(LEANING, Box::new(Leaning));
    let metrics = DisplayMetrics {
        width,
        ..CLARA_BW_METRICS
    };
    let screen = Screen::new(1, vec![emphasised(text)])
        .with_reading(true)
        .with_reading_font(Some(LEANING))
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
    drop_book_typesetter(LEANING);
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
