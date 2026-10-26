use super::*;

#[test]
fn a_finger_on_a_button_finds_the_button_and_not_the_page() {
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(7),
            label: "Read".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    );
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let button = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(_, _, _)))
        .expect("the button was laid out")
        .rect;

    let inside = layout
        .pressed_control(button.x + button.width / 2, button.y + button.height / 2)
        .expect("a finger in the middle of the button is on the button");
    assert_eq!(inside, button);
}

#[test]
fn a_finger_on_bare_text_has_nothing_to_invert() {
    let screen = Screen::new(
        1,
        vec![Node::Text {
            id: NodeId(1),
            text: "Once upon a time".into(),
            links: Vec::new(),
        }],
    );
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    let text = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Text))
        .expect("the text was laid out")
        .rect;

    // Tapping prose may well turn the page, but there is no control there,
    // and inverting a paragraph would look like a fault rather than
    // feedback.
    assert_eq!(
        layout.pressed_control(text.x + text.width / 2, text.y + text.height / 2),
        None
    );
}

/// A screen with one outlined button, rendered, and the button's rect.
fn rendered_button(emphasis: Emphasis) -> (Surface, Rect) {
    let screen = Screen::new(
        1,
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(1),
            label: "Add".to_owned(),
            state: ControlState::Enabled,
            emphasis,
        }],
    );
    let rect = screen
        .layout_for(&CLARA_BW_METRICS)
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("a button")
        .rect;
    let mut surface = Surface::new(
        usize::try_from(CLARA_BW_METRICS.width).expect("a panel"),
        usize::try_from(CLARA_BW_METRICS.height).expect("a panel"),
    );
    surface.fill_rect(
        Rect {
            x: 0,
            y: 0,
            width: CLARA_BW_METRICS.width,
            height: CLARA_BW_METRICS.height,
        },
        tone::PAPER,
    );
    render(&screen, &mut surface, None);
    (surface, rect)
}

/// A dev aid: renders a shelf and a pair of buttons to a PGM so the
/// weights can be looked at without a reader on the desk. Run with
/// `cargo test -p kobo-ui control_sheet -- --ignored`.
#[test]
#[ignore = "writes a sheet to look at rather than asserting anything"]
fn control_sheet() {
    let screen = Screen::new(
        1,
        vec![
            Node::Rows {
                id: NodeId(1),
                rows: vec![
                    Row::new(ActionId(1), "Scroll.in", "feeds.feedburner.com", Glyph::Rss),
                    Row::new(
                        ActionId(2),
                        "Humor, Satire, and Cartoons",
                        "newyorker.com",
                        Glyph::Rss,
                    ),
                    Row::new(
                        ActionId(3),
                        "Culture: TV, Movies, Music",
                        "newyorker.com",
                        Glyph::Rss,
                    ),
                ],
            },
            Node::Spacer {
                id: NodeId(2),
                space: Space::Medium,
            },
            Node::Button {
                id: NodeId(3),
                action: ActionId(4),
                label: "Add a feed".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Normal,
            },
            Node::Button {
                id: NodeId(4),
                action: ActionId(5),
                label: "Save".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            },
        ],
    );
    let width = CLARA_BW_METRICS.width;
    let height = 700;
    let mut surface = Surface::new(
        usize::try_from(width).unwrap(),
        usize::try_from(height).unwrap(),
    );
    surface.clear(tone::PAPER);
    render(&screen, &mut surface, None);
    let path = std::env::temp_dir().join("cobalt-control-sheet.pgm");
    let mut out = format!("P5\n{width} {height}\n255\n").into_bytes();
    out.extend_from_slice(&surface.pixels);
    std::fs::write(&path, out).unwrap();
    println!("written to {}", path.display());
}

#[test]
fn a_button_has_its_corners_taken_off() {
    // Square corners on a full-width outlined rectangle is what an HTML
    // form looked like in 1996, and it is most of why these read as
    // wireframes rather than as controls.
    for emphasis in [Emphasis::Normal, Emphasis::Primary] {
        let (surface, rect) = rendered_button(emphasis);
        let at = |x: i32, y: i32| {
            surface.pixels[usize::try_from(y * CLARA_BW_METRICS.width + x).expect("inside")]
        };
        assert_eq!(
            at(rect.x, rect.y),
            tone::PAPER,
            "{emphasis:?} kept a square corner"
        );
        assert_ne!(
            at(rect.x + rect.width / 2, rect.y),
            tone::PAPER,
            "{emphasis:?} has no top edge"
        );
    }
}

#[test]
fn a_buttons_outline_is_heavier_than_a_rule() {
    // A rule separates two things and wants to be quiet. An outline says
    // where a finger may go and wants to be found. At the same weight the
    // button read as one more line on the page.
    let (surface, rect) = rendered_button(Emphasis::Normal);
    let column = rect.x + rect.width / 2;
    let ink = (rect.y..rect.y + rect.height)
        .take_while(|y| {
            surface.pixels[usize::try_from(y * CLARA_BW_METRICS.width + column).expect("inside")]
                != tone::PAPER
        })
        .count();
    let expected = usize::try_from(CLARA_BW_METRICS.button_border()).expect("a border");
    assert_eq!(ink, expected, "the top edge is {ink} pixels");
}

#[test]
fn an_outlined_button_is_not_filled_in() {
    // The border is the difference between the shape and the same shape
    // inset, so it must leave the middle of the control alone.
    let (surface, rect) = rendered_button(Emphasis::Normal);
    let middle = rect.y + rect.height / 2;
    let at = |x: i32| {
        surface.pixels[usize::try_from(middle * CLARA_BW_METRICS.width + x).expect("inside")]
    };
    assert_eq!(at(rect.x + rect.width / 4), tone::PAPER);
    assert_ne!(at(rect.x), tone::PAPER, "the left edge went missing");
}

#[test]
fn inverting_a_rectangle_twice_leaves_the_picture_as_it_was() {
    let mut surface = Surface::new(20, 10);
    surface.fill_rect(
        Rect {
            x: 2,
            y: 2,
            width: 5,
            height: 5,
        },
        30,
    );
    let before = surface.pixels.clone();
    let rect = Rect {
        x: 1,
        y: 1,
        width: 8,
        height: 8,
    };

    surface.invert_rect(rect);
    assert_ne!(surface.pixels, before, "inverting has to change something");
    surface.invert_rect(rect);
    assert_eq!(
        surface.pixels, before,
        "releasing a control must restore it exactly"
    );
}

#[test]
fn a_press_mark_stays_inside_the_control_it_marks() {
    // The failure this replaced: a row's hit rectangle is the full width of
    // the panel, so a tap turned an eighth of the page solid black between
    // the rules above and below it.
    let metrics = CLARA_BW_METRICS;
    let mut surface = Surface::new(200, 60);
    let before = surface.pixels.clone();
    let rect = Rect {
        x: 0,
        y: 10,
        width: 200,
        height: 40,
    };
    surface.invert_press(rect, &metrics);

    let marked = |x: usize, y: usize| surface.pixels[y * 200 + x] != before[y * 200 + x];
    assert!(!marked(0, 10), "the mark reached the corner of the row");
    assert!(!marked(100, 10), "the mark reached the top edge of the row");
    assert!(!marked(0, 30), "the mark reached the side of the panel");
    assert!(marked(100, 30), "the mark did not cover what was touched");
}

#[test]
fn a_press_mark_has_its_corners_taken_off() {
    // Asserted by measuring the mark rather than by predicting where its
    // corner lands: the inset and the radius are both clamped against the
    // control's own size, so a test that guessed a coordinate would be
    // testing its own arithmetic.
    let metrics = CLARA_BW_METRICS;
    let mut surface = Surface::new(200, 60);
    let before = surface.pixels.clone();
    surface.invert_press(
        Rect {
            x: 0,
            y: 0,
            width: 200,
            height: 60,
        },
        &metrics,
    );
    let run = |y: usize| {
        (0..200)
            .filter(|x| surface.pixels[y * 200 + x] != before[y * 200 + x])
            .count()
    };
    let rows: Vec<usize> = (0..60).map(run).collect();
    let first = rows.iter().position(|&n| n > 0).expect("a mark");
    assert!(
        rows[first] < rows[30],
        "the mark's first row is {} wide against {} in the middle, so its \
             corners are square",
        rows[first],
        rows[30]
    );
    assert!(rows[first] > 0, "the mark has no top edge at all");
}

#[test]
fn a_press_mark_undoes_itself_exactly() {
    // The mark is removed by drawing it again, which is only true if its
    // shape comes from the rectangle and the metrics and nothing else.
    let metrics = CLARA_BW_METRICS;
    let mut surface = Surface::new(200, 60);
    for (i, pixel) in surface.pixels.iter_mut().enumerate() {
        *pixel = u8::try_from(i % 251).unwrap_or(0);
    }
    let before = surface.pixels.clone();
    let rect = Rect {
        x: 3,
        y: 4,
        width: 150,
        height: 44,
    };
    surface.invert_press(rect, &metrics);
    assert_ne!(surface.pixels, before, "the press was never drawn");
    surface.invert_press(rect, &metrics);
    assert_eq!(surface.pixels, before, "releasing left the row marked");
}

#[test]
fn a_small_control_still_shows_it_was_touched() {
    // A checkbox is smaller than twice the inset. Taken literally the mark
    // would be nothing at all, which is the state this whole mechanism
    // exists to avoid.
    let metrics = CLARA_BW_METRICS;
    let mut surface = Surface::new(40, 40);
    let rect = Rect {
        x: 8,
        y: 8,
        width: 24,
        height: 24,
    };
    let before = surface.pixels.clone();
    surface.invert_press(rect, &metrics);
    let marked = surface
        .pixels
        .iter()
        .zip(&before)
        .filter(|(now, then)| now != then)
        .count();
    assert!(
        marked >= 24 * 24 / 4,
        "a small control's press mark covered only {marked} pixels"
    );
}

#[test]
fn inverting_off_the_edge_touches_nothing_outside_the_surface() {
    let mut surface = Surface::new(8, 8);
    let before = surface.pixels.clone();
    surface.invert_rect(Rect {
        x: -40,
        y: -40,
        width: 10,
        height: 10,
    });
    assert_eq!(surface.pixels, before);
}

#[test]
fn a_section_header_never_ends_a_page_without_its_first_row() {
    let area = CLARA_BW_METRICS.prose_area(true, true);
    // Enough rows to overflow, with a section opening partway down, placed
    // by binary search at every position so no single lucky offset is
    // being asserted.
    let plain: Vec<(&str, &str)> = (0..40)
        .map(|_| ("A row with an ordinary length of title", "and a summary"))
        .collect();
    for opener in 1..plain.len() {
        let rows: Vec<(Option<&str>, &str, &str)> = plain
            .iter()
            .enumerate()
            .map(|(index, (title, summary))| {
                (
                    (index == opener).then_some("Everything else"),
                    *title,
                    *summary,
                )
            })
            .collect();
        let pages = paginate_rows_in_sections(&rows, &CLARA_BW_METRICS, area);
        let page = pages
            .iter()
            .find(|page| page.contains(&opener))
            .expect("every row lands on a page");
        let heights: i32 = page
            .iter()
            .map(|index| {
                let (section, title, summary) = rows[*index];
                let text_width = row_text_width(&CLARA_BW_METRICS, area);
                let body = wrap_text(title, text_width, FontSize::Body).len() as i32
                    * FontSize::Body.line_height()
                    + wrap_text(summary, text_width, FontSize::Caption).len() as i32
                        * FontSize::Caption.line_height()
                    + CLARA_BW_METRICS.space(Space::Small) * 2;
                max(CLARA_BW_METRICS.touch_target_default(), body)
                    + if section.is_some() {
                        section_height(&CLARA_BW_METRICS)
                    } else {
                        0
                    }
            })
            .sum();
        let separators = (page.len() as i32 - 1) * (area.gap * 2);
        assert!(
            heights + separators <= area.height || page.len() == 1,
            "a page carrying a section header and its row overflowed"
        );
    }
}

#[test]
fn an_action_bar_marks_nothing_and_a_nav_bar_is_asked_to() {
    let actions = Screen::new(1, Vec::new()).with_nav_bar(NavBar::actions(
        NodeId(9),
        vec![
            BarAction::new(ActionId(1), "Back"),
            BarAction::new(ActionId(2), "Next"),
        ],
    ));
    let layout = actions.layout_for(&CLARA_BW_METRICS);
    assert!(
        !layout
            .nodes
            .iter()
            .any(|node| matches!(node.kind, LayoutKind::NavDestinationSelected(..))),
        "none of these is a place the reader could be standing"
    );
    assert!(
        !actions
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::NavBarWithoutSelection),
        "a bar of verbs has nothing to mark and must not be nagged about it"
    );

    let unmarked = Screen::new(1, Vec::new()).with_nav_bar(NavBar::new(
        NodeId(9),
        vec![
            BarAction::new(ActionId(1), "Stories"),
            BarAction::new(ActionId(2), "Saved"),
        ],
        None,
    ));
    assert!(
        unmarked
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::NavBarWithoutSelection),
        "a bar of places that marks none of them is a bar that lies"
    );
}

/// Every composition lint, each with a screen that fires it and one that
/// does not. The pairs matter more than the cases: a lint that fires on
/// everything is noise, and noise is turned off.
#[test]
fn state_written_into_a_label_is_caught_where_a_state_field_exists() {
    let fires = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: vec![Tile::new(ActionId(1), "Moby-Dick (kept)", Glyph::Book)],
            shape: TileShape::Portrait,
        }],
    );
    assert!(
        fires
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::StateInLabel),
        "this is the exact string the shelf drew for a year"
    );

    let quiet = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            tiles: vec![
                Tile::new(ActionId(1), "Moby-Dick", Glyph::Book).with_state(TileState::Held),
                Tile::new(ActionId(2), "Ulysses (1922)", Glyph::Book),
                Tile::new(ActionId(3), "Emma (annotated edition)", Glyph::Book),
            ],
            shape: TileShape::Portrait,
        }],
    );
    assert!(
        !quiet
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::StateInLabel),
        "a year, an edition and a real state field are all legitimate"
    );
}

#[test]
fn a_second_way_back_is_caught() {
    let back = |owns: bool, extra: Vec<Node>| {
        Screen::new(1, extra)
            .with_own_back(owns)
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::AmbiguousBack)
    };
    let button = |label: &str| {
        vec![Node::Button {
            id: NodeId(1),
            action: ActionId(1),
            label: label.to_owned(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }]
    };
    assert!(
        back(true, button("Back to the results")),
        "the runtime already drew Back; this is the second one"
    );
    assert!(
        !back(true, button("Read")),
        "a verb that is not a retreat is not a second way out"
    );
    assert!(
        !back(false, button("Back to the results")),
        "a screen that did not take Back may certainly draw its own"
    );
}

#[test]
fn a_second_primary_action_is_caught() {
    let primary = |emphasis: Emphasis| Node::Button {
        id: NodeId(2),
        action: ActionId(2),
        label: "Delete".to_owned(),
        state: ControlState::Enabled,
        emphasis,
    };
    let first = Node::Button {
        id: NodeId(1),
        action: ActionId(1),
        label: "Read".to_owned(),
        state: ControlState::Enabled,
        emphasis: Emphasis::Primary,
    };
    let fired = |emphasis: Emphasis| {
        Screen::new(1, vec![first.clone(), primary(emphasis)])
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::MultiplePrimaryActions)
    };
    assert!(fired(Emphasis::Primary), "two primaries is no decision");
    assert!(!fired(Emphasis::Normal), "one primary is the whole point");
}

#[test]
fn a_section_header_with_nothing_under_it_is_caught() {
    let section = |id: u32, title: &str| Node::Section {
        id: NodeId(id),
        title: title.to_owned(),
        value: None,
        link: None,
    };
    let orphan = Screen::new(1, vec![section(1, "Details")]);
    assert!(
        orphan
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::OrphanedSection),
        "a header with its content overleaf is the classic broken page"
    );

    let filled = Screen::new(
        1,
        vec![
            section(1, "Details"),
            Node::Text {
                id: NodeId(2),
                text: "Published in 1851.".to_owned(),
                links: Vec::new(),
            },
        ],
    );
    assert!(
        !filled
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::OrphanedSection),
        "a header that keeps its first line is exactly right"
    );
}

#[test]
fn a_spinner_that_knows_its_own_total_is_caught() {
    let activity = |transferred: Option<(u64, Option<u64>)>| Node::Activity {
        id: NodeId(1),
        label: "Downloading".to_owned(),
        progress: None,
        cancel: None,
        transferred,
        failure: None,
    };
    let fired = |transferred| {
        Screen::new(1, vec![activity(transferred)])
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| issue.kind == LayoutIssueKind::IndeterminateWithKnownTotal)
    };
    assert!(
        fired(Some((1024, Some(8192)))),
        "it has counted the work and is still refusing to say"
    );
    assert!(
        !fired(Some((1024, None))),
        "a server that admitted no total leaves nothing to draw a bar from"
    );
    assert!(!fired(None), "a plain wait is a plain wait");
}

#[test]
fn a_screen_that_spends_every_ink_is_caught() {
    let over_budget = Screen::new(
        1,
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Shelf".to_owned(),
                level: 1,
            },
            Node::Secondary {
                id: NodeId(2),
                text: "Nine books".to_owned(),
            },
            Node::Divider { id: NodeId(3) },
            Node::Card {
                id: NodeId(4),
                children: vec![Node::Text {
                    id: NodeId(5),
                    text: "A card is the surface tone.".to_owned(),
                    links: Vec::new(),
                }],
            },
            Node::Button {
                id: NodeId(6),
                action: ActionId(1),
                label: "Read".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            },
        ],
    );
    let issue = over_budget
        .validate(&CLARA_BW_METRICS)
        .into_iter()
        .find(|issue| matches!(issue.kind, LayoutIssueKind::ToneBudget { .. }));
    assert_eq!(
        issue.map(|issue| issue.kind),
        Some(LayoutIssueKind::ToneBudget { used: 5 }),
        "ink, muted, hairline, surface and inverted is all five"
    );

    let within_budget = Screen::new(
        1,
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Shelf".to_owned(),
                level: 1,
            },
            Node::Secondary {
                id: NodeId(2),
                text: "Nine books".to_owned(),
            },
            Node::Divider { id: NodeId(3) },
            Node::Button {
                id: NodeId(4),
                action: ActionId(1),
                label: "Read".to_owned(),
                state: ControlState::Enabled,
                emphasis: Emphasis::Primary,
            },
        ],
    );
    assert!(
        !within_budget
            .validate(&CLARA_BW_METRICS)
            .iter()
            .any(|issue| matches!(issue.kind, LayoutIssueKind::ToneBudget { .. })),
        "four inks is the budget, not a warning"
    );
}

#[test]
fn an_action_bar_stops_at_three_verbs() {
    let bar = NavBar::actions(
        NodeId(1),
        (1..=5)
            .map(|index| BarAction::new(ActionId(index), format!("Do {index}")))
            .collect(),
    );
    assert_eq!(bar.visible(&CLARA_BW_METRICS).len(), MAX_ACTION_BAR_ACTIONS);
}

#[test]
fn byte_counts_read_the_way_a_person_says_them() {
    assert_eq!(byte_size(0), "0 B");
    assert_eq!(byte_size(512), "512 B");
    assert_eq!(byte_size(1024), "1.0 KB");
    assert_eq!(byte_size(4_404_019), "4.2 MB");
    assert_eq!(byte_size(11_534_336), "11 MB");
}

#[test]
fn a_transfer_without_a_total_reports_bytes_and_draws_no_bar() {
    let screen = Screen::new(
        1,
        vec![Node::Activity {
            id: NodeId(1),
            label: "Downloading".into(),
            progress: None,
            cancel: None,
            transferred: Some((4_404_019, None)),
            failure: None,
        }],
    );
    let layout = screen.layout_for(&CLARA_BW_METRICS);
    assert!(
        !layout
            .nodes
            .iter()
            .any(|node| node.kind == LayoutKind::ActivityProgress),
        "a bar with no denominator invents its own position"
    );
    let bytes = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::ActivityBytes)
        .expect("the count is the whole report when there is no bar");
    assert_eq!(bytes.text_lines, vec!["4.2 MB".to_string()]);
}

#[test]
fn a_transfer_with_a_total_says_how_far_through_it_is() {
    let layout = Screen::new(
        1,
        vec![Node::Activity {
            id: NodeId(1),
            label: "Downloading".into(),
            progress: Some(Percent::new(38)),
            cancel: None,
            transferred: Some((4_404_019, Some(11_534_336))),
            failure: None,
        }],
    )
    .layout_for(&CLARA_BW_METRICS);
    let bytes = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::ActivityBytes)
        .expect("a byte caption");
    assert_eq!(bytes.text_lines, vec!["4.2 MB of 11 MB".to_string()]);
}

#[test]
fn a_failed_transfer_keeps_what_was_around_it() {
    let layout = Screen::new(
        1,
        vec![
            Node::Heading {
                id: NodeId(1),
                text: "Moby Dick".into(),
                level: 1,
            },
            Node::Activity {
                id: NodeId(2),
                label: "Downloading".into(),
                progress: None,
                cancel: None,
                transferred: Some((512, None)),
                failure: Some(TransferFailure {
                    reason: "The connection was reset".into(),
                    resumable: true,
                }),
            },
        ],
    )
    .layout_for(&CLARA_BW_METRICS);
    assert!(
        layout
            .nodes
            .iter()
            .any(|node| node.text_lines.iter().any(|line| line == "Moby Dick")),
        "a failure must not take the screen away from the reader"
    );
    let failure = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::ActivityFailure)
        .expect("the reason");
    assert_eq!(
        failure.text_lines,
        vec!["The connection was reset".to_string()]
    );
}

#[test]
fn a_ranked_list_pages_with_the_column_its_digits_really_take() {
    // The layout engine sizes the lead column to what sits in it, so a
    // ranked row's title is wider than a marked row's by the difference
    // between digits and a mark. Paginated against a mark, a headline that
    // is drawn on one line is measured as one line too many, and the page
    // comes back a row short with a row's worth of white under it.
    let metrics = CLARA_BW_METRICS;
    let area = metrics.prose_area(true, true);
    assert!(
        row_rank_column(&metrics, 30) < row_mark_column(&metrics),
        "digits took at least as much room as a mark"
    );

    let marked = row_title_width_beside(
        &metrics,
        area,
        "40 points",
        false,
        row_mark_column(&metrics),
    );
    let ranked = row_title_width_beside(
        &metrics,
        area,
        "40 points",
        false,
        row_rank_column(&metrics, 30),
    );
    assert!(ranked > marked, "a ranked title was no wider");

    // A headline that straddles the two measures: it wraps one line
    // further beside a mark than beside digits. Built out of single
    // letters so the wrap can land between them, and long enough that the
    // row is taller than the finger-sized floor either way, which is what
    // makes the difference show up in the paging rather than be absorbed.
    let title = (1..)
        .map(|words| (0..words).map(|_| "n ").collect::<String>())
        .take(400)
        .find(|title| {
            let ranked_lines = wrap_text(title, ranked, FontSize::Body).len();
            let marked_lines = wrap_text(title, marked, FontSize::Body).len();
            ranked_lines >= 4 && marked_lines > ranked_lines
        })
        .expect("no headline straddled the two measures");

    let rows = (0..40)
        .map(|_| (title.as_str(), "someone, an hour ago", "40 points"))
        .collect::<Vec<_>>();
    let ranked_pages = paginate_ranked_rows_with_trailing(&rows, &metrics, area, 30);
    let marked_pages = paginate_rows_with_trailing(&rows, &metrics, area);
    assert!(
        ranked_pages[0].len() > marked_pages[0].len(),
        "the ranked page held no more: {} either way",
        ranked_pages[0].len()
    );
}
