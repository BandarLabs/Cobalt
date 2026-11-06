use super::*;

/// A reading face that kerns: every "AV" it measures together is four pixels
/// narrower than its letters, the way a real face tucks a V under an A.
/// Letters are otherwise ten pixels wide.
struct Kerning;

impl Typesetter for Kerning {
    fn measure(&self, text: &str, _size: FontSize, _face: Face) -> (i32, i32) {
        let letters = i32::try_from(text.chars().count()).unwrap_or(0);
        let pairs = i32::try_from(text.matches("AV").count()).unwrap_or(0);
        (10 * letters - 4 * pairs, 30)
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
}

fn kerning() -> FontHandle {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9_400);
    let handle = FontHandle(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    put_book_typesetter(handle, Box::new(Kerning));
    handle
}

/// "AVAT", the first A strong and the rest plain, so the strong run ends in
/// the middle of a kerning pair, laid out with a link over "AT" and the
/// paragraph open to selection.
fn laid_out() -> Layout {
    let face = kerning();
    let screen = Screen::new(
        1,
        vec![Node::RichText {
            id: NodeId(1),
            text: "AVAT".to_owned(),
            spans: vec![RichTextSpan {
                start: 0,
                end: 1,
                presentation: TextPresentation {
                    strong: true,
                    ..TextPresentation::default()
                },
            }],
            links: vec![TextLink {
                action: ActionId::from_name("link"),
                start: 2,
                end: 4,
            }],
            presentation: ParagraphPresentation::default(),
            selection: Some(TextSelection {
                context: 1,
                offset: 0,
            }),
            formulae: Vec::new(),
        }],
    )
    .with_reading(true)
    .with_reading_font(Some(face))
    .with_typography(Typography::Fine);
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::with_back(false));
    drop_book_typesetter(face);
    layout
}

#[test]
fn a_link_after_a_run_boundary_is_found_under_the_letters_as_they_were_drawn() {
    let layout = laid_out();
    let plain = layout
        .nodes
        .iter()
        .find(|node| node.text_lines.first().map(String::as_str) == Some("VAT"))
        .expect("the plain run is drawn on its own");
    let link = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::InlineLink(_)))
        .expect("the link has a target");
    // The plain run is drawn as "VAT" from its own edge, so "AT" starts one
    // unkerned V into it. Measuring "AV" across the whole line put the link
    // four pixels to the left of the letters it underlines.
    assert_eq!(link.rect.x, plain.rect.x + 10);
    assert_eq!(
        link.rect.x + link.rect.width,
        plain.rect.x + plain.rect.width
    );
}

#[test]
fn a_word_held_to_select_it_is_found_where_its_letters_were_drawn() {
    let layout = laid_out();
    let first = layout
        .nodes
        .iter()
        .find(|node| node.text_lines.first().map(String::as_str) == Some("A"))
        .expect("the strong run is drawn");
    let plain = layout
        .nodes
        .iter()
        .find(|node| node.text_lines.first().map(String::as_str) == Some("VAT"))
        .expect("the plain run is drawn");
    let (rect, _) = layout.text_hits.first().expect("the word can be held");
    assert_eq!(rect.x, first.rect.x);
    assert_eq!(rect.x + rect.width, plain.rect.x + plain.rect.width);
}
