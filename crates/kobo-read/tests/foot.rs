//! The place at the foot of a page, held against the layout that draws it.
//!
//! Run in a process of its own because it installs the real faces, and a face
//! once installed is the face every other test in the same process would be
//! measured in. The reader measures a page in the application and the runtime
//! draws it; this is the check that the two still agree once a line has been
//! taken off the foot of every page, at every size of type a reader can
//! choose and at the smallest and largest sizes the interface can be set to.

use kobo_doc::{Block, Contents, Document};
use kobo_read::{action, Memory, Reader};
use kobo_ui::{Chrome, DisplayMetrics, LayoutIssueKind, LayoutKind, Screen, TextScale};

/// Two chapters of paragraphs of every length from a line to most of a page,
/// so that pages end part-way through a paragraph, on a short one, and under
/// a heading.
fn novel() -> Document {
    let sentence = "The path follows the water past a low stone wall, and a wooden bench faces the bend where the reeds move. ";
    let chapter = |from: usize| {
        (from..from + 30).map(move |n| Block::Paragraph(sentence.repeat(1 + (n * 7) % 9)))
    };
    let mut blocks = vec![Block::Heading {
        level: 1,
        text: "The Weir".into(),
    }];
    blocks.extend(chapter(0));
    let second = blocks.len();
    blocks.push(Block::Heading {
        level: 1,
        text: "The Mill, and What Was Found Under Its Wheel in the Dry Summer".into(),
    });
    blocks.extend(chapter(3));
    Document {
        contents: vec![
            Contents {
                title: "The Weir".into(),
                block: 0,
                depth: 0,
            },
            Contents {
                title: "The Mill".into(),
                block: second,
                depth: 0,
            },
        ],
        blocks,
        ..Document::default()
    }
}

fn opened(reading: TextScale, interface: TextScale, full_page: bool) -> (Reader, DisplayMetrics) {
    let metrics = DisplayMetrics {
        text_scale: interface,
        ..kobo_ui::CLARA_BW_METRICS
    };
    let memory = Memory {
        scale: reading,
        full_page,
        ..Memory::default()
    };
    (
        in_a_callback(&metrics, || Reader::open(novel(), memory, &metrics)),
        metrics,
    )
}

/// Runs `body` the way the SDK runs every callback of an application: with
/// the interface's own type size in force, which is the size a heading and
/// the place line are measured at while the book is being broken into pages.
fn in_a_callback<T>(metrics: &DisplayMetrics, body: impl FnOnce() -> T) -> T {
    kobo_ui::with_text_scale(metrics.text_scale, body)
}

/// The lowest point any of the book's own words reach on a drawn page.
fn words_end(layout: &kobo_ui::Layout) -> i32 {
    layout
        .nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind,
                LayoutKind::Text | LayoutKind::RichText(_) | LayoutKind::Heading(_)
            )
        })
        .map(|node| node.rect.y + node.rect.height)
        .max()
        .unwrap_or(0)
}

/// The place line as it was drawn: where it is, and what it says.
fn place(layout: &kobo_ui::Layout) -> Option<(kobo_ui::Rect, Vec<String>)> {
    layout
        .nodes
        .iter()
        .rev()
        .find(|node| node.kind == LayoutKind::Secondary)
        .map(|node| (node.rect, node.text_lines.clone()))
}

fn lost_nothing(screen: &Screen, metrics: &DisplayMetrics) -> Result<(), String> {
    let lost: Vec<_> = screen
        .diagnostics(metrics, &Chrome::with_back(true))
        .issues
        .into_iter()
        .filter(|issue| {
            matches!(
                issue.kind,
                LayoutIssueKind::ContentOverflow { .. }
                    | LayoutIssueKind::Clipped
                    | LayoutIssueKind::TextOverflow
            )
        })
        .map(|issue| format!("{:?}", issue.kind))
        .collect();
    if lost.is_empty() {
        Ok(())
    } else {
        Err(lost.join(", "))
    }
}

#[test]
fn a_page_that_keeps_its_bar_ends_above_the_place_and_the_place_never_moves() {
    let _ = kobo_text::install(kobo_ui::CLARA_BW_METRICS);
    for interface in [TextScale::Smallest, TextScale::Default, TextScale::Largest] {
        for reading in TextScale::STEPS {
            let (mut reader, metrics) = opened(reading, interface, false);
            let at = format!(
                "type at {}%, interface at {}%",
                reading.percent(),
                interface.percent()
            );
            assert!(
                reader.page_count() > 4,
                "{at}: too short a book to prove much"
            );
            let floor = metrics.height - metrics.screen_margin();
            let mut stands_at = None;
            for page in 1..=reader.page_count() {
                let screen = reader.screen("A Novel");
                if let Err(lost) = lost_nothing(&screen, &metrics) {
                    panic!("{at}: page {page} was measured for more than it shows: {lost}");
                }
                let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
                let (line, said) =
                    place(&layout).unwrap_or_else(|| panic!("{at}: page {page} says no place"));
                assert_eq!(said.len(), 1, "{at}: the place wrapped on page {page}");
                assert!(
                    said[0].starts_with(&format!("Page {page} of ")),
                    "{at}: page {page} is called {:?}",
                    said[0]
                );
                assert!(
                    words_end(&layout) <= line.y,
                    "{at}: the words of page {page} run into the place under them"
                );
                assert!(
                    line.y + line.height <= floor,
                    "{at}: the place on page {page} hangs past the foot"
                );
                assert_eq!(
                    *stands_at.get_or_insert(line.y),
                    line.y,
                    "{at}: the place stands somewhere else on page {page}"
                );
                in_a_callback(&metrics, || reader.act(action::FORWARD, &metrics));
            }
        }
    }
}

#[test]
fn a_page_given_the_whole_panel_carries_no_place_and_holds_at_least_as_much() {
    let _ = kobo_text::install(kobo_ui::CLARA_BW_METRICS);
    for reading in TextScale::STEPS {
        let (mut whole, metrics) = opened(reading, TextScale::Default, true);
        let (kept, _) = opened(reading, TextScale::Default, false);
        assert!(
            whole.page_count() <= kept.page_count(),
            "a page with a bar and a place held more than one with neither"
        );
        for page in 1..=whole.page_count() {
            let screen = whole.screen("A Novel");
            if let Err(lost) = lost_nothing(&screen, &metrics) {
                panic!("page {page} of the whole page lost {lost}");
            }
            let layout = screen.layout_with(&metrics, &Chrome::with_back(true));
            assert!(place(&layout).is_none(), "a whole page said its place");
            in_a_callback(&metrics, || whole.act(action::FORWARD, &metrics));
        }
    }
}

#[test]
fn pinning_the_bar_from_the_controls_brings_the_place_and_giving_it_up_takes_it_away() {
    let _ = kobo_text::install(kobo_ui::CLARA_BW_METRICS);
    let (mut reader, metrics) = opened(TextScale::Default, TextScale::Default, true);
    let said = |reader: &Reader| {
        place(
            &reader
                .screen("A Novel")
                .layout_with(&metrics, &Chrome::with_back(true)),
        )
        .map(|(_, said)| said.join(" "))
    };
    assert_eq!(said(&reader), None);
    reader.act(action::CONTROLS, &metrics);
    reader.act(action::FULL_PAGE, &metrics);
    let line = said(&reader).expect("the page that keeps its bar says its place");
    assert!(
        line.starts_with("Page 1 of ") && line.ends_with("left in the chapter"),
        "{line}"
    );
    reader.act(action::CONTROLS, &metrics);
    reader.act(action::FULL_PAGE, &metrics);
    assert_eq!(said(&reader), None);
}
