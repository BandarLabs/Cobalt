use crate::{action_id, Chrome, Context, Glyph, ScreenBuilder};
use kobo_ui::{DisplayMetrics, LayoutKind, CLARA_BW_METRICS};

/// The smallest and the largest panels: a six inch Clara and a ten inch Elipsa.
const PANELS: [(&str, DisplayMetrics); 2] = [
    ("clara", CLARA_BW_METRICS),
    (
        "elipsa",
        DisplayMetrics {
            width: 1404,
            height: 1872,
            pixels_per_inch: 227,
            ..CLARA_BW_METRICS
        },
    ),
];

fn chip_rects(screen: &crate::Screen, metrics: &DisplayMetrics) -> Vec<(i32, i32, bool)> {
    screen
        .layout_with(metrics, &Chrome::default())
        .nodes
        .iter()
        .filter_map(|node| match node.kind {
            LayoutKind::Chip(_, selected) => Some((node.rect.width, node.rect.y, selected)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_segmented_control_is_one_row_of_equal_segments_with_the_current_one_filled() {
    for (panel, metrics) in PANELS {
        let screen = ScreenBuilder::new("segments")
            .segmented(1, [("two", "2"), ("three", "3"), ("four", "4")])
            .build();
        let chips = chip_rects(&screen, &metrics);
        assert_eq!(chips.len(), 3, "{panel}");
        assert!(chips.iter().all(|&(_, y, _)| y == chips[0].1), "{panel}");
        // Equal to the pixel, apart from the rounding the last one takes.
        assert!(
            chips[..2].iter().all(|&(width, _, _)| width == chips[0].0),
            "{}",
            panel
        );
        assert!((chips[2].0 - chips[0].0).abs() < 3, "{panel}");
        let filled: Vec<bool> = chips.iter().map(|&(_, _, selected)| selected).collect();
        assert_eq!(filled, [false, true, false], "{panel}");
        assert!(screen
            .diagnostics(&metrics, &Chrome::default())
            .issues
            .is_empty());
    }
}

#[test]
fn every_segment_answers_its_own_action() {
    let metrics = Context::default().metrics();
    let screen = ScreenBuilder::new("segments")
        .segmented(0, [("any", "Any"), ("easy", "Easy"), ("hard", "Hard")])
        .build();
    let layout = screen.layout_with(&metrics, &Chrome::default());
    for name in ["any", "easy", "hard"] {
        assert!(layout.rect_of_action(action_id(name)).is_some(), "{name}");
    }
}

#[test]
fn labels_too_long_for_a_share_wrap_rather_than_truncate() {
    let metrics = Context::default().metrics();
    let long = "A label far too long to share a row with four others";
    let screen = ScreenBuilder::new("segments")
        .segmented(
            0,
            [
                ("a", long),
                ("b", long),
                ("c", long),
                ("d", long),
                ("e", long),
            ],
        )
        .build();
    let chips = chip_rects(&screen, &metrics);
    assert_eq!(chips.len(), 5);
    assert!(chips.iter().any(|&(_, y, _)| y != chips[0].1));
}

#[test]
fn chips_outside_a_band_keep_their_own_widths() {
    let metrics = Context::default().metrics();
    let screen = ScreenBuilder::new("tags")
        .chips([("a", "Poetry", false), ("b", "Sea", false)])
        .build();
    let chips = chip_rects(&screen, &metrics);
    assert_ne!(chips[0].0, chips[1].0);
}

#[test]
fn buttons_in_a_row_are_the_same_width() {
    for (panel, metrics) in PANELS {
        let screen = ScreenBuilder::new("row")
            .buttons([("solo", "Solo round"), ("about", "About")])
            .build();
        let layout = screen.layout_with(&metrics, &Chrome::default());
        let solo = layout.rect_of_action(action_id("solo")).unwrap();
        let about = layout.rect_of_action(action_id("about")).unwrap();
        assert!((solo.width - about.width).abs() < 3, "{panel}");
    }
}

#[test]
fn toggles_write_their_state_at_the_trailing_edge() {
    let screen = ScreenBuilder::new("settings")
        .toggles([
            ("sound", "Sound", "", Glyph::VolumeUp, true),
            ("wake", "Wake on schedule", "", Glyph::Clock, false),
        ])
        .build();
    let shown = format!("{screen:?}");
    assert!(shown.contains("\"On\""));
    assert!(shown.contains("\"Off\""));
}

#[test]
fn a_picker_names_its_value_and_ticks_it_in_the_menu() {
    let options = [
        ("newest", "Newest"),
        ("oldest", "Oldest"),
        ("title", "Title"),
    ];
    let shut = ScreenBuilder::new("list")
        .picker("sort", "Sort", false, 1, options)
        .build();
    let shown = format!("{shut:?}");
    assert!(shown.contains("Sort: Oldest"));
    assert!(!shown.contains("Newest"));
    let open = ScreenBuilder::new("list")
        .picker("sort", "Sort", true, 1, options)
        .build();
    let metrics = Context::default().metrics();
    let layout = open.layout_with(&metrics, &Chrome::default());
    for name in ["newest", "oldest", "title"] {
        assert!(layout.rect_of_action(action_id(name)).is_some(), "{name}");
    }
    assert!(open
        .diagnostics(&metrics, &Chrome::default())
        .issues
        .is_empty());
}

#[test]
fn a_command_never_wraps_inside_a_flag() {
    let screen = ScreenBuilder::new("setup")
        .command("kobo flashcards stage collection.cobfc --kobo-root /Volumes/KOBOeReader/with/a/longer/path")
        .build();
    for (panel, metrics) in PANELS {
        let layout = screen.layout_with(&metrics, &Chrome::default());
        let lines: Vec<String> = layout
            .nodes
            .iter()
            .flat_map(|node| node.text_lines.clone())
            .collect();
        assert!(
            lines.iter().all(|line| {
                let line = line.trim_end().trim_end_matches('\u{2060}');
                !line.ends_with('\u{2011}') && !line.ends_with('/')
            }),
            "{panel}: {lines:?}"
        );
    }
}

#[test]
fn a_command_wider_than_the_panel_still_lays_out() {
    let metrics = DisplayMetrics {
        width: 758,
        height: 1024,
        pixels_per_inch: 212,
        text_scale: kobo_ui::TextScale::Largest,
    };
    let screen = ScreenBuilder::new("setup")
        .command("kobo stream --interactive-session-with-a-very-long-flag-name -- /bin/sh")
        .build();
    let _ = screen.layout_with(&metrics, &Chrome::default());
}
