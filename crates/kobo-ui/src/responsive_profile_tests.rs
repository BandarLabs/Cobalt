use super::*;

fn panels() -> Vec<(String, DisplayMetrics)> {
    kobo_profile::SUPPORTED_PROFILES
        .iter()
        .flat_map(|profile| {
            let portrait = DisplayMetrics {
                width: i32::try_from(profile.width).expect("profile width fits layout"),
                height: i32::try_from(profile.height).expect("profile height fits layout"),
                pixels_per_inch: i32::from(profile.pixels_per_inch),
                text_scale: TextScale::Default,
            };
            let landscape = DisplayMetrics {
                width: portrait.height,
                height: portrait.width,
                ..portrait
            };
            [
                (format!("{} portrait", profile.id), portrait),
                (format!("{} landscape", profile.id), landscape),
            ]
        })
        .collect()
}

fn navigation() -> NavBar {
    NavBar::new(
        NodeId(90),
        [
            ("Home", Glyph::App),
            ("Library", Glyph::Reader),
            ("Search", Glyph::Search),
            ("Settings", Glyph::Settings),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (label, glyph))| {
            BarAction::new(ActionId(100 + index as u32), label).with_glyph(glyph)
        })
        .collect(),
        Some(0),
    )
}

fn showcase() -> Screen {
    Screen::new(
            1,
            vec![
                Node::Heading {
                    id: NodeId(1),
                    text: "A responsive Folio screen".into(),
                    level: 1,
                },
                Node::Text {
                    id: NodeId(2),
                    text: "Interface prose keeps a readable measure instead of stretching from bezel to bezel on a large or rotated reader.".into(),
                    links: Vec::new(),
                },
                Node::Secondary {
                    id: NodeId(3),
                    text: "Profile-driven typography and spacing".into(),
                },
                Node::Section {
                    id: NodeId(4),
                    title: "Controls".into(),
                    value: Some("Responsive".into()),
                    link: None,
                },
                Node::Field {
                    id: NodeId(5),
                    action: ActionId(5),
                    value: "Search Kobo".into(),
                    placeholder: "Search".into(),
                    clear: Some(ActionId(6)),
                    label: None,
                    hint: None,
                    error: None,
                    kind: FieldKind::Text,
                },
                Node::Button {
                    id: NodeId(6),
                    action: ActionId(7),
                    label: "Continue".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
                Node::Progress {
                    id: NodeId(7),
                    value: Percent::new(55),
                },
            ],
        )
        .with_top_bar(
            TopBar::new(NodeId(80), "Folio")
                .action(ActionId(80), "Display settings and reading preferences"),
        )
        .with_nav_bar(navigation())
}

fn assert_safe_layout(name: &str, metrics: &DisplayMetrics, screen: &Screen) {
    let layout = screen.layout_with(metrics, &Chrome::with_back(true));
    for node in &layout.nodes {
        assert!(
            node.rect.x >= 0
                && node.rect.y >= 0
                && node.rect.x.saturating_add(node.rect.width) <= metrics.width
                && node.rect.y.saturating_add(node.rect.height) <= metrics.height,
            "{name}: {:?} is outside {metrics:?}: {:?}",
            node.kind,
            node.rect
        );
        if node.kind.acts_on().is_some() {
            assert!(
                node.rect.width >= metrics.touch_target_minimum()
                    && node.rect.height >= metrics.touch_target_minimum(),
                "{name}: {:?} is smaller than a touch target: {:?}",
                node.kind,
                node.rect
            );
        }
        if matches!(
            node.kind,
            LayoutKind::Heading(_)
                | LayoutKind::Text
                | LayoutKind::Secondary
                | LayoutKind::Quote(..)
                | LayoutKind::FactLabel
                | LayoutKind::FactValue
                | LayoutKind::PagedList
                | LayoutKind::Banner(_)
                | LayoutKind::SplashTitle
                | LayoutKind::SplashText
                | LayoutKind::ActivityLabel
                | LayoutKind::ActivityBytes
                | LayoutKind::ActivityFailure
        ) {
            assert!(
                node.rect.width <= metrics.readable_width(),
                "{name}: {:?} uses an unreadable {}px measure",
                node.kind,
                node.rect.width
            );
        }
    }

    let controls = layout
        .nodes
        .iter()
        .filter(|node| node.kind.acts_on().is_some())
        .collect::<Vec<_>>();
    for (index, left) in controls.iter().enumerate() {
        for right in &controls[index + 1..] {
            if left.id == right.id {
                continue;
            }
            assert!(
                left.rect.intersection(right.rect).is_none(),
                "{name}: {:?} overlaps {:?}",
                left.kind,
                right.kind
            );
        }
    }
}

/// A mark on a chosen square has to stay visible.
///
/// The glyph in a selected cell used to be drawn in paper, which is right
/// over ink and wrong over everything else the toolkit fills a cell with.
/// A tic-tac-toe board marking its winning line came out as three empty
/// squares.
#[test]
fn a_glyph_in_a_chosen_cell_is_still_drawn_in_ink() {
    let metrics = CLARA_BW_METRICS;
    let cells = (0..4).map(|index| {
        Cell::new(ActionId(index + 1), " ")
            .with_glyph(Glyph::Circle)
            .with_selected(index < 2)
    });
    let screen = Screen::new(
        1,
        vec![Node::Grid {
            id: NodeId(1),
            columns: 2,
            square: true,
            cells: cells.collect(),
        }],
    );
    let layout = screen.layout_with(&metrics, &Chrome::default());
    assert!(
        layout
            .nodes
            .iter()
            .all(|node| !matches!(node.kind, LayoutKind::InlineGlyph(_, true))),
        "a board glyph was inverted over a light cell"
    );
    let mut surface = Surface::new(
        usize::try_from(metrics.width).expect("width"),
        usize::try_from(metrics.height).expect("height"),
    );
    render_with(&screen, &metrics, &Chrome::default(), &mut surface, None);
    let chosen = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::InlineGlyph(..)))
        .expect("a cell with a mark in it");
    let mut inked = 0;
    for y in chosen.rect.y..chosen.rect.y + chosen.rect.height {
        for x in chosen.rect.x..chosen.rect.x + chosen.rect.width {
            let at = usize::try_from(y).expect("row")
                * usize::try_from(metrics.width).expect("width")
                + usize::try_from(x).expect("column");
            if surface.pixels[at] < tone::MUTED {
                inked += 1;
            }
        }
    }
    assert!(inked > 0, "the mark on a chosen square was invisible");
}

#[test]
fn every_supported_profile_and_orientation_has_safe_responsive_primitives() {
    for (name, metrics) in panels() {
        let screen = showcase();
        assert_safe_layout(&name, &metrics, &screen);

        let mut surface = Surface::new(
            usize::try_from(metrics.width).expect("positive profile width"),
            usize::try_from(metrics.height).expect("positive profile height"),
        );
        render_with(
            &screen,
            &metrics,
            &Chrome::with_back(true),
            &mut surface,
            None,
        );
        assert!(surface.pixels.iter().any(|pixel| *pixel != tone::PAPER));

        let dialog = Screen::new(2, Vec::new()).with_overlay(Overlay::modal(
            NodeId(20),
            "Responsive dialog",
            vec![
                Node::Text {
                    id: NodeId(21),
                    text: "Dialog copy keeps a readable line length on every supported panel."
                        .into(),
                    links: Vec::new(),
                },
                Node::Button {
                    id: NodeId(22),
                    action: ActionId(22),
                    label: "Done".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Primary,
                },
            ],
        ));
        assert_safe_layout(&format!("{name} dialog"), &metrics, &dialog);
        let overlay = dialog
            .layout_for(&metrics)
            .nodes
            .into_iter()
            .find(|node| node.kind == LayoutKind::Overlay)
            .expect("dialog overlay");
        assert!(overlay.rect.width <= metrics.readable_width());
    }
}

#[test]
fn tile_columns_follow_the_allocated_width_on_every_profile() {
    for (name, metrics) in panels() {
        for shape in [TileShape::Square, TileShape::Portrait, TileShape::Card] {
            let screen = Screen::new(
                1,
                vec![Node::TileGrid {
                    id: NodeId(1),
                    tiles: (0..20)
                        .map(|index| {
                            Tile::new(ActionId(index + 1), format!("Tile {index}"), Glyph::App)
                        })
                        .collect(),
                    shape,
                }],
            );
            let layout = screen.layout_for(&metrics);
            let tiles = layout
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
                .collect::<Vec<_>>();
            assert!(!tiles.is_empty(), "{name} {shape:?}: no tiles");
            let first_y = tiles[0].rect.y;
            let first_row = tiles
                .iter()
                .take_while(|tile| tile.rect.y == first_y)
                .count();
            assert_eq!(
                first_row,
                metrics.grid_columns_for_width(shape, metrics.content_width()),
                "{name} {shape:?}: wrong column count"
            );
            assert_safe_layout(&format!("{name} {shape:?} tiles"), &metrics, &screen);
        }

        let half = metrics.content_width() / 2 - metrics.space(Space::Small) / 2;
        assert!(
            metrics.grid_columns_for_width(TileShape::Card, half)
                <= metrics.grid_columns(TileShape::Card),
            "{name}: a half-width band gained tile columns"
        );
    }
}

#[test]
fn legacy_flow_keeps_the_uncapped_panel_width() {
    let (_, metrics) = panels()
        .into_iter()
        .max_by_key(|(_, metrics)| metrics.width_tenth_mm())
        .expect("supported profiles");
    let screen = Screen::new(
        1,
        vec![
            Node::Text {
                id: NodeId(1),
                text: "Legacy prose uses the full content width.".into(),
                links: Vec::new(),
            },
            Node::Section {
                id: NodeId(2),
                title: "Legacy section".into(),
                value: Some("Full width".into()),
                link: None,
            },
            Node::Band {
                id: NodeId(3),
                align: BandAlign::Top,
                slots: vec![BandSlot::fill(vec![Node::Secondary {
                    id: NodeId(4),
                    text: "Legacy band".into(),
                }])],
            },
        ],
    );
    let content_width = metrics.content_width();
    assert_eq!(
        metrics.prose_area(false, false).width,
        content_width.min(metrics.readable_width())
    );
    assert_eq!(
        with_legacy_typography(true, || metrics.prose_area(false, false).width),
        content_width
    );
    let narrow_area = ProseArea {
        width: content_width / 2,
        height: 0,
        gap: 0,
        face: Face::Text,
    };
    let modern_columns = paginate_tiles(100, &metrics, TileShape::Card, narrow_area)[0].len();
    let legacy_columns = with_legacy_typography(true, || {
        paginate_tiles(100, &metrics, TileShape::Card, narrow_area)[0].len()
    });
    assert_eq!(
        modern_columns,
        metrics.grid_columns_for_width(TileShape::Card, narrow_area.width)
    );
    assert_eq!(legacy_columns, metrics.grid_columns(TileShape::Card));
    assert!(legacy_columns > modern_columns);

    let modern = screen.layout_for(&metrics);
    let legacy = screen.with_legacy_typography(true).layout_for(&metrics);

    for id in [NodeId(1), NodeId(2), NodeId(3)] {
        let rect = legacy
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("legacy node {id:?}"));
        assert_eq!(rect.rect.width, content_width, "legacy node {id:?}");
    }
    assert_eq!(
        modern
            .nodes
            .iter()
            .find(|node| node.id == NodeId(1))
            .expect("modern text")
            .rect
            .width,
        content_width.min(metrics.readable_width())
    );
    for id in [NodeId(2), NodeId(3)] {
        assert_eq!(
            modern
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap_or_else(|| panic!("modern node {id:?}"))
                .rect
                .width,
            content_width.min(metrics.control_width()),
            "modern node {id:?}"
        );
    }
}

#[test]
fn square_boards_keep_their_geometry_and_trailing_controls_at_larger_sizes() {
    for (name, base) in panels() {
        for text_scale in TextScale::STEPS {
            let metrics = DisplayMetrics { text_scale, ..base };
            let screen = Screen::new(
                1,
                vec![
                    Node::Heading {
                        id: NodeId(1),
                        text: "O to play".into(),
                        level: 1,
                    },
                    Node::Grid {
                        id: NodeId(2),
                        columns: 3,
                        square: true,
                        cells: (1..=9).map(|id| Cell::new(ActionId(id), "O")).collect(),
                    },
                    Node::Grid {
                        id: NodeId(3),
                        columns: 2,
                        square: false,
                        cells: vec![
                            Cell::new(ActionId(10), "Reset game"),
                            Cell::new(ActionId(11), "How to play"),
                        ],
                    },
                ],
            )
            .with_top_bar(TopBar::new(NodeId(4), "Tic-tac-toe"));
            let diagnostics = screen.diagnostics(&metrics, &Chrome::measuring(true));
            assert!(
                !diagnostics.has_errors(),
                "{name} {text_scale:?}: {:?}",
                diagnostics.issues
            );
            let cells = diagnostics
                .layout
                .nodes
                .iter()
                .filter(|node| node.id == NodeId(2) && matches!(node.kind, LayoutKind::Cell(..)))
                .collect::<Vec<_>>();
            assert_eq!(cells.len(), 9);
            for cell in &cells {
                assert_eq!(cell.rect.width, cell.rect.height);
                assert!(cell.rect.width >= metrics.touch_target_minimum());
            }
            assert_eq!(cells[0].rect.y, cells[2].rect.y);
            assert!(cells[3].rect.y > cells[2].rect.y);
            for action in 1..=11 {
                assert!(diagnostics
                    .layout
                    .nodes
                    .iter()
                    .any(|node| node.kind.acts_on() == Some(ActionId(action))));
            }
        }
    }
}

#[test]
fn long_document_regions_yield_to_trailing_controls() {
    for (name, metrics) in panels() {
        let screen = Screen::new(
            1,
            vec![
                Node::Text {
                    id: NodeId(1),
                    text: "A long document line ".repeat(500),
                    links: Vec::new(),
                },
                Node::Grid {
                    id: NodeId(2),
                    columns: 3,
                    square: false,
                    cells: ["Previous", "Continue", "Close"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| Cell::new(ActionId(index as u32 + 1), label))
                        .collect(),
                },
            ],
        );
        let diagnostics = screen.diagnostics(&metrics, &Chrome::with_back(false));
        assert!(
            diagnostics.issues.iter().any(|issue| {
                issue.node == Some(NodeId(1))
                    && issue.severity == DiagnosticSeverity::Error
                    && issue.kind == LayoutIssueKind::TextOverflow
            }),
            "{name}: truncated document text was not reported"
        );
        assert!(
            !diagnostics
                .issues
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::InteractiveOffscreen),
            "{name}: {:?}",
            diagnostics.issues
        );
        let controls = diagnostics
            .layout
            .nodes
            .iter()
            .filter(|node| matches!(node.kind, LayoutKind::Cell(..)))
            .collect::<Vec<_>>();
        assert_eq!(controls.len(), 3, "{name}");
        for control in controls {
            assert!(
                rect_is_inside(control.rect, diagnostics.layout.content),
                "{name}: {:?} is outside {:?}",
                control.rect,
                diagnostics.layout.content
            );
        }
    }
}

#[test]
fn section_action_labels_have_one_render_source_on_every_panel() {
    for (name, metrics) in panels() {
        let labels = ["Details", "View all ↗"];
        let screen = Screen::new(
            1,
            labels
                .iter()
                .enumerate()
                .map(|(index, label)| Node::Section {
                    id: NodeId(index as u32 + 1),
                    title: if index == 0 {
                        "Continue".into()
                    } else {
                        "Featured".into()
                    },
                    value: None,
                    link: Some(BarAction::new(ActionId(index as u32 + 1), *label)),
                })
                .collect(),
        );
        let chrome = Chrome::with_back(false);
        let diagnostics = screen.diagnostics(&metrics, &chrome);
        assert!(
            diagnostics
                .issues
                .iter()
                .all(|issue| issue.severity != DiagnosticSeverity::Error),
            "{name}: {:?}",
            diagnostics.issues
        );
        for label in labels {
            let sources = diagnostics
                .layout
                .nodes
                .iter()
                .flat_map(|node| &node.text_lines)
                .filter(|line| line.as_str() == label)
                .count();
            assert_eq!(sources, 1, "{name}: {label:?} has {sources} render sources");
        }
        assert!(
            diagnostics
                .layout
                .nodes
                .iter()
                .filter(|node| node.kind == LayoutKind::Section)
                .all(|node| node.text_lines.len() == 1),
            "{name}: a section body still carries its action label"
        );

        let mut surface = Surface::new(
            usize::try_from(metrics.width).expect("positive profile width"),
            usize::try_from(metrics.height).expect("positive profile height"),
        );
        render_with(&screen, &metrics, &chrome, &mut surface, None);
        assert!(surface.pixels.iter().any(|pixel| *pixel != tone::PAPER));
        if metrics == CLARA_BW_METRICS {
            for (index, label) in labels.into_iter().enumerate() {
                let id = NodeId(index as u32 + 1);
                let section = diagnostics
                    .layout
                    .nodes
                    .iter()
                    .find(|node| node.id == id && node.kind == LayoutKind::Section)
                    .expect("section");
                let link = diagnostics
                    .layout
                    .nodes
                    .iter()
                    .find(|node| matches!(node.kind, LayoutKind::SectionLink(_)) && node.id == id)
                    .expect("section link");
                let label_width = measure_text(label, FontSize::Caption).0;
                let label_top =
                    link.rect.y + (link.rect.height - FontSize::Caption.line_height()) / 2;
                let label_left = link.rect.x + link.rect.width - label_width;
                for y in section.rect.y..label_top {
                    for x in label_left..link.rect.x + link.rect.width {
                        let offset = usize::try_from(y).expect("visible y") * surface.width
                            + usize::try_from(x).expect("visible x");
                        assert_eq!(
                            surface.pixels[offset],
                            tone::PAPER,
                            "{label:?} was also painted above its control"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn rich_document_overflow_is_reported_without_sacrificing_controls() {
    for (name, metrics) in panels() {
        let styled = |text: String, presentation| {
            let strong = text.find("styled").expect("strong run");
            let emphasis = text.find("document").expect("emphasis run");
            Node::RichText {
                id: NodeId(1),
                text,
                spans: vec![
                    RichTextSpan {
                        start: strong,
                        end: strong + "styled".len(),
                        presentation: TextPresentation {
                            strong: true,
                            ..TextPresentation::default()
                        },
                    },
                    RichTextSpan {
                        start: emphasis,
                        end: emphasis + "document".len(),
                        presentation: TextPresentation {
                            emphasis: true,
                            ..TextPresentation::default()
                        },
                    },
                ],
                links: Vec::new(),
                presentation,
                selection: None,
                formulae: Vec::new(),
            }
        };
        let presentation = ParagraphPresentation {
            line_height_percent: 120,
            margin_before_em: 25,
            margin_after_em: 25,
            first_line_indent_em: 100,
            ..ParagraphPresentation::default()
        };
        let screen = Screen::new(
            1,
            vec![
                styled("A long styled document line ".repeat(500), presentation),
                Node::Grid {
                    id: NodeId(2),
                    columns: 3,
                    square: false,
                    cells: ["Previous", "Continue", "Close"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, label)| Cell::new(ActionId(index as u32 + 1), label))
                        .collect(),
                },
            ],
        );
        let diagnostics = screen.diagnostics(&metrics, &Chrome::with_back(false));
        assert!(
            diagnostics.issues.iter().any(|issue| {
                issue.node == Some(NodeId(1))
                    && issue.severity == DiagnosticSeverity::Error
                    && issue.kind == LayoutIssueKind::TextOverflow
            }),
            "{name}: truncated rich text was not reported: {:?}",
            diagnostics.issues
        );
        assert!(
            !diagnostics.issues.iter().any(|issue| matches!(
                issue.kind,
                LayoutIssueKind::InteractiveOffscreen | LayoutIssueKind::Clipped
            )),
            "{name}: {:?}",
            diagnostics.issues
        );
        let bounded = diagnostics
            .layout
            .nodes
            .iter()
            .filter(|node| node.id == NodeId(1) || matches!(node.kind, LayoutKind::Cell(..)));
        for node in bounded {
            assert!(
                rect_is_inside(node.rect, diagnostics.layout.content),
                "{name}: {:?} is outside {:?}",
                node.rect,
                diagnostics.layout.content
            );
        }

        let fitting = Screen::new(
            2,
            vec![styled(
                "A styled document line remains fully visible.".into(),
                presentation,
            )],
        )
        .diagnostics(&metrics, &Chrome::with_back(false));
        assert!(
            !fitting
                .issues
                .iter()
                .any(|issue| issue.kind == LayoutIssueKind::TextOverflow),
            "{name}: fully visible rich text reported overflow: {:?}",
            fitting.issues
        );
    }
}
