//! Lists paged under a banner that carries its retry, and lists whose rows
//! carry a button or a count, measured the way they are drawn on a Clara at
//! every text size held both ways.
//!
//! These are the two measures that kept retries off the banners that explain
//! them and counts out of the rows they belong to: a banner's action takes a
//! line under its words, and a row's button and badge each keep a column of
//! the title's width.

use crate::{
    ActionId, AppRunner, BannerLevel, BannerShape, Chrome, Context, KoboApp, RowEdge, Screen,
    ScreenBuilder,
};
use kobo_ui::{
    DiagnosticSeverity, DisplayMetrics, Glyph, Orientation, TextScale, CLARA_BW_METRICS,
};

struct Shows;
impl KoboApp for Shows {
    fn on_start(&mut self, _context: &mut Context) {}
    fn on_action(&mut self, _context: &mut Context, _action: ActionId) {}
}

const TEXT: &str = "Subscriptions were not saved. The reader may be out of space.";

const TITLES: [&str; 5] = [
    "Feeds",
    "The Wren, the Robin and the Long Winter: notes from a garden",
    "Hacker News",
    "A long subscription title that wraps onto a second line beside its count",
    "Short",
];

const LEVELS: [BannerLevel; 4] = [
    BannerLevel::Info,
    BannerLevel::Attention,
    BannerLevel::Success,
    BannerLevel::Error,
];

/// Every size of a Clara, held both ways, with a context measuring there.
fn claras() -> Vec<(String, DisplayMetrics, Context)> {
    let mut out = Vec::new();
    for text_scale in TextScale::STEPS {
        for orientation in [Orientation::Portrait, Orientation::Landscape] {
            let metrics = DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            }
            .oriented(orientation);
            let context = AppRunner::with_metrics(Shows, metrics).context();
            out.push((format!("{text_scale:?} {orientation:?}"), metrics, context));
        }
    }
    out
}

fn errors(screen: &Screen, metrics: &DisplayMetrics) -> Vec<String> {
    screen
        .diagnostics(metrics, &Chrome::measuring(true))
        .issues
        .iter()
        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
        .map(|issue| format!("{issue:?}"))
        .collect()
}

fn rows() -> Vec<(&'static str, &'static str)> {
    (0..24)
        .map(|index| (TITLES[index % TITLES.len()], "12 unread · Updated 10:40"))
        .collect()
}

/// One page of the list as a screen draws it: the banner over the rows, the
/// page turns and position, and a bottom action under them.
fn page_under_banner(
    level: BannerLevel,
    action: bool,
    rows: &[(&str, &str)],
    page: &[usize],
    at: (usize, usize),
) -> Screen {
    let mut screen = ScreenBuilder::new("feeds")
        .top_bar("Feeds")
        .banner(level, TEXT);
    if action {
        screen = screen.banner_action("retry-subscriptions", "Retry saving");
    }
    screen
        .rows(page.iter().map(|&index| {
            (
                format!("feed-{index}"),
                rows[index].0,
                rows[index].1,
                Glyph::Book,
            )
        }))
        .page_turns("previous", "next")
        .page_position(
            u16::try_from(at.0 + 1).unwrap(),
            u16::try_from(at.1).unwrap(),
        )
        .bottom_action("add", "Add feed")
        .build()
}

#[test]
fn a_list_paged_below_a_banner_of_any_level_with_its_action_is_drawn_whole_on_a_clara_at_every_size_held_either_way(
) {
    let rows = rows();
    for (at, metrics, context) in claras() {
        for level in LEVELS {
            for action in [false, true] {
                let mut shape = BannerShape::new(level, TEXT);
                if action {
                    shape = shape.with_action();
                }
                let pages = context.paginate_rows_below_banner(&rows, true, Some(shape));
                let mut seen = Vec::new();
                for (index, page) in pages.iter().enumerate() {
                    let screen =
                        page_under_banner(level, action, &rows, page, (index, pages.len()));
                    let found = errors(&screen, &metrics);
                    assert!(
                        found.is_empty(),
                        "{at} {level:?} action {action}: {found:?}"
                    );
                    seen.extend(page.iter().copied());
                }
                assert_eq!(seen, (0..rows.len()).collect::<Vec<_>>(), "{at}");
            }
        }
    }
}

#[test]
fn the_words_alone_leave_no_room_for_an_error_banners_action_somewhere_on_a_clara() {
    // Why `paginate_rows_below_banner` exists: paged under the words of the
    // banner alone, the list overflows once the mark and the action line are
    // drawn.
    let rows = rows();
    let overflowed = claras().into_iter().any(|(_, metrics, context)| {
        let pages = context.paginate_rows_below_notice(&rows, true, Some(TEXT));
        pages.iter().enumerate().any(|(index, page)| {
            let screen =
                page_under_banner(BannerLevel::Error, true, &rows, page, (index, pages.len()));
            !errors(&screen, &metrics).is_empty()
        })
    });
    assert!(overflowed);
}

#[test]
fn the_notice_paginators_measure_exactly_as_they_did_for_plain_words() {
    let rows = rows();
    for (at, _, context) in claras() {
        assert_eq!(
            context.paginate_rows_below_notice(&rows, true, Some(TEXT)),
            context.paginate_rows_below_banner(
                &rows,
                true,
                Some(BannerShape::new(BannerLevel::Attention, TEXT))
            ),
            "{at}"
        );
        assert_eq!(
            context.paginate_rows_with_menu_below_notice(&rows, false, None),
            context.paginate_rows_with_menu_below_banner(&rows, false, None),
            "{at}"
        );
    }
}

/// The feed shelf's shape: every row has a menu, rows with something unread
/// carry a count, and a few carry a button.
fn edged(index: usize) -> RowEdge<'static> {
    match index % 4 {
        1 => RowEdge::new().with_menu(),
        2 => RowEdge::new().with_menu().with_button("Update"),
        _ => RowEdge::new().with_menu().with_badge(),
    }
}

#[test]
fn rows_with_counts_buttons_and_menus_under_a_banner_with_an_action_are_drawn_whole_on_a_clara_at_every_size_held_either_way(
) {
    let titles = rows();
    for (at, metrics, context) in claras() {
        let rows: Vec<(String, &str, RowEdge)> = titles
            .iter()
            .enumerate()
            .map(|(index, (title, summary))| {
                let edge = edged(index);
                (
                    context.clamped_row_with_edge(title, 2, true, edge),
                    *summary,
                    edge,
                )
            })
            .collect();
        let measured: Vec<(&str, &str, RowEdge)> = rows
            .iter()
            .map(|(title, summary, edge)| (title.as_str(), *summary, *edge))
            .collect();
        let shape = BannerShape::new(BannerLevel::Error, TEXT).with_action();
        let pages = context.paginate_rows_with_edges_below_banner(&measured, true, Some(shape));
        let mut seen = Vec::new();
        for (number, page) in pages.iter().enumerate() {
            let mut screen = ScreenBuilder::new("feeds")
                .top_bar("Feeds")
                .banner(BannerLevel::Error, TEXT)
                .banner_action("retry-subscriptions", "Retry saving")
                .rows_with_menu(page.iter().map(|&index| {
                    (
                        format!("feed-{index}"),
                        measured[index].0,
                        measured[index].1,
                        Glyph::Book,
                        format!("menu-{index}"),
                    )
                }));
            for &index in page {
                let edge = measured[index].2;
                if edge.badge {
                    screen = screen.row_badge(format!("feed-{index}"), 12);
                }
                if let Some(label) = edge.button {
                    screen = screen.row_button(
                        format!("feed-{index}"),
                        format!("update-{index}"),
                        label,
                    );
                }
            }
            let screen = screen
                .page_turns("previous", "next")
                .page_position(
                    u16::try_from(number + 1).unwrap(),
                    u16::try_from(pages.len()).unwrap(),
                )
                .bottom_action("add", "Add feed")
                .build();
            let found = errors(&screen, &metrics);
            assert!(found.is_empty(), "{at}: {found:?}");
            seen.extend(page.iter().copied());
        }
        assert_eq!(seen, (0..measured.len()).collect::<Vec<_>>(), "{at}");

        // The same rows, under the same banner, measured from a built prefix.
        let prefix = ScreenBuilder::new("feeds")
            .top_bar("Feeds")
            .banner(BannerLevel::Error, TEXT)
            .banner_action("retry-subscriptions", "Retry saving")
            .bottom_action("add", "Add feed")
            .build();
        assert_eq!(
            context.paginate_rows_with_edges_under(
                &measured,
                true,
                crate::Position::AtTheFoot,
                &prefix
            ),
            pages,
            "{at}"
        );
    }
}

#[test]
fn a_title_clamped_beside_a_row_button_is_drawn_on_no_more_lines_than_it_was_clamped_to() {
    const LONG: &str =
        "The Wren, the Robin and the Long Winter: notes from a garden kept through three hard years";
    // The second title and label are the case that once came out a line
    // long: on a Clara held sideways at the largest size, "Download again"
    // is wider than a third of the readable measure and narrower than a
    // third of the row.
    for (at, metrics, context) in claras() {
        for (title, label, lines) in [
            (LONG, "Download again", 2),
            (
                "A Walk by the River and Back Again by the Longer Path",
                "Download again",
                2,
            ),
            (LONG, "Install", 1),
            (LONG, "Update", 2),
        ] {
            let edge = RowEdge::new().with_button(label);
            let clamped = context.clamped_row_with_edge(title, lines, false, edge);
            let screen = ScreenBuilder::new("library")
                .top_bar("Library")
                .rows([(
                    "book",
                    clamped.as_str(),
                    "Saved for offline reading",
                    Glyph::Book,
                )])
                .row_button("book", "again", label)
                .build();
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            let drawn = layout
                .nodes
                .iter()
                .find(|node| node.kind == kobo_ui::LayoutKind::RowTitle)
                .map(|node| node.text_lines.len())
                .unwrap();
            assert!(
                drawn <= lines,
                "{at} {label}: {drawn} lines for {clamped:?}"
            );
        }
    }
}
