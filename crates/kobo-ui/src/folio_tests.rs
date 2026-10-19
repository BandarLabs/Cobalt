use super::*;

#[test]
fn folio_card_tile_keeps_its_body_label_caption_and_live_value() {
    let screen = Screen::new(
        1,
        vec![Node::TileGrid {
            id: NodeId(1),
            shape: TileShape::Card,
            tiles: vec![Tile::new(ActionId(7), "App Store", Glyph::Download)
                .with_caption("Updates")
                .with_value("3")],
        }],
    );
    let layout = screen.layout();
    assert!(layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::TileValue));
    assert!(layout.nodes.iter().any(|node| {
        node.kind == LayoutKind::TileLabel && node.rect.height >= FontSize::Body.line_height()
    }));
    assert!(screen.validate(&CLARA_BW_METRICS).is_empty());
}

#[test]
fn a_screen_that_hides_its_bar_still_gets_one_back_when_it_is_asked_for() {
    let chrome = Chrome::with_back(true);
    let asking = Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true);

    let hidden = ensure_way_back_revealed(asking.clone(), &chrome, "Birds", TopBarState::Hidden);
    assert!(
        hidden.top_bar.is_none(),
        "hidden means the bar is not drawn"
    );
    let shown = ensure_way_back_revealed(asking, &chrome, "Birds", TopBarState::Shown);
    assert!(
        shown.top_bar.is_some(),
        "asking for it back must produce a bar to leave from"
    );

    // Everything that did not opt in keeps the guarantee unchanged.
    let ordinary = Screen::new(1, Vec::new());
    assert!(
        ensure_way_back_revealed(ordinary, &chrome, "Birds", TopBarState::Hidden)
            .top_bar
            .is_some(),
        "a screen that never asked to hide its bar must never lose it"
    );
}

#[test]
fn the_way_back_the_shell_draws_can_actually_be_reached_by_a_finger() {
    // Composing and hit testing have to be done against the same screen.
    // When they were not, the Back the shell had drawn sat in a place no
    // touch resolved to, and every ordinary application lost its way out
    // while still showing one.
    let metrics = &CLARA_BW_METRICS;
    let chrome = Chrome::with_back(true);
    let drawn = Screen::new(1, Vec::new());

    let shown = ensure_way_back_revealed(drawn.clone(), &chrome, "Books", TopBarState::Shown);
    let layout = shown.layout_with(metrics, &chrome);
    let rect = layout
        .rect_of_action(ActionId::BACK)
        .expect("the drawn way back must have a rectangle");
    assert_eq!(
        layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
        Some(ActionId::BACK),
        "a touch in the middle of the drawn Back must resolve to Back"
    );

    // And the hidden case: nothing to hit, because nothing is drawn.
    let hiding = Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true);
    let hidden = ensure_way_back_revealed(hiding.clone(), &chrome, "Birds", TopBarState::Hidden);
    assert!(hidden
        .layout_with(metrics, &chrome)
        .rect_of_action(ActionId::BACK)
        .is_none());
    // Until it is asked for, and then it is reachable like any other.
    let revealed = ensure_way_back_revealed(hiding, &chrome, "Birds", TopBarState::Shown);
    let layout = revealed.layout_with(metrics, &chrome);
    let rect = layout
        .rect_of_action(ActionId::BACK)
        .expect("asking for the bar back must draw one");
    assert_eq!(
        layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
        Some(ActionId::BACK)
    );
}

#[test]
fn the_band_that_brings_the_bar_back_is_where_the_bar_would_be() {
    let metrics = &CLARA_BW_METRICS;
    let asking = Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true);
    let bar = metrics.top_bar_height();
    let bare = Chrome::with_back(true);

    assert_eq!(
        top_bar_touch(&asking, metrics, &bare, TopBarState::Hidden, bar / 2),
        Some(TopBarState::Shown),
        "touching where the bar would be asks for it back"
    );
    assert_eq!(
        top_bar_touch(&asking, metrics, &bare, TopBarState::Hidden, bar + 1),
        None,
        "below the band the touch is the screen's own"
    );
    assert_eq!(
        top_bar_touch(&asking, metrics, &bare, TopBarState::Shown, bar + 1),
        Some(TopBarState::Hidden),
        "carrying on elsewhere puts it away"
    );
    assert_eq!(
        top_bar_touch(&asking, metrics, &bare, TopBarState::Shown, bar / 2),
        None,
        "a touch on a bar that is showing belongs to the bar, so Back works"
    );

    let ordinary = Screen::new(1, Vec::new());
    assert_eq!(
        top_bar_touch(&ordinary, metrics, &bare, TopBarState::Hidden, bar / 2),
        None,
        "a screen that never asked must not have its top edge taken"
    );

    // With a status band the bar is not at the top of the panel, so the
    // band is not claimed and the bar is never hidden in the first place.
    let with_status = Chrome::with_back(true).with_status(Status::default());
    assert_eq!(
        top_bar_touch(&asking, metrics, &with_status, TopBarState::Hidden, bar / 2),
        None,
        "the top edge belongs to the status band when there is one"
    );
    assert!(
        ensure_way_back_revealed(
            Screen::new(1, Vec::new()).with_auto_hidden_top_bar(true),
            &with_status,
            "Birds",
            TopBarState::Hidden
        )
        .top_bar
        .is_some(),
        "a bar under a status band stays drawn rather than hiding out of reach"
    );
}

#[test]
fn folio_section_link_is_a_caption_sized_control() {
    let action = ActionId(8);
    let screen = Screen::new(
        1,
        vec![Node::Section {
            id: NodeId(1),
            title: "Featured".into(),
            value: None,
            link: Some(BarAction::new(action, "View all ↗")),
        }],
    );
    let layout = screen.layout();
    let link = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::SectionLink(action))
        .expect("section link is laid out");
    assert!(link.rect.height >= CLARA_BW_METRICS.touch_target_minimum());
}

#[test]
fn short_section_links_stay_in_bounds_and_reserve_their_touch_height() {
    let action = ActionId(8);
    let following = NodeId(2);
    let screen = Screen::new(
        1,
        vec![
            Node::Section {
                id: NodeId(1),
                title: "Featured".into(),
                value: None,
                link: Some(BarAction::new(action, "All")),
            },
            Node::Secondary {
                id: following,
                text: "Following content".into(),
            },
        ],
    );
    let layout = screen.layout();
    let link = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::SectionLink(action))
        .expect("section link");
    let next = layout
        .nodes
        .iter()
        .find(|node| node.id == following)
        .expect("following node");
    assert!(link.rect.x >= layout.content.x);
    assert!(
        link.rect.x.saturating_add(link.rect.width)
            <= layout.content.x.saturating_add(layout.content.width)
    );
    assert!(link.rect.y.saturating_add(link.rect.height) <= next.rect.y);
}

#[test]
fn row_holds_accelerate_the_visible_overflow_without_replacing_taps() {
    let open = ActionId(7);
    let menu = ActionId(8);
    let screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(1),
            rows: vec![Row::new(open, "Article", "Summary", Glyph::Note).with_menu(menu)],
        }],
    );
    let layout = screen.layout();
    let row = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Row(open))
        .expect("row");
    let x = row.rect.x + row.rect.width / 2;
    let y = row.rect.y + row.rect.height / 2;
    assert_eq!(layout.hit_test(x, y), Some(open));
    assert_eq!(layout.hit_hold(x, y), Some(menu));
    let overflow = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::RowMenu(menu))
        .expect("visible overflow");
    assert_eq!(
        layout.hit_test(
            overflow.rect.x + overflow.rect.width / 2,
            overflow.rect.y + overflow.rect.height / 2,
        ),
        Some(menu)
    );
}

#[test]
fn tile_holds_share_a_visible_details_action_and_leave_taps_immediate() {
    let open = ActionId(7);
    let details = ActionId(8);
    let screen = Screen::new(
        1,
        vec![
            Node::Section {
                id: NodeId(1),
                title: "Featured".into(),
                value: None,
                link: Some(BarAction::new(details, "Details")),
            },
            Node::TileGrid {
                id: NodeId(2),
                shape: TileShape::Card,
                tiles: vec![Tile::new(open, "Article", Glyph::Note).with_menu(details)],
            },
        ],
    );
    let layout = screen.layout();
    let tile = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Tile(open, ControlState::Enabled))
        .expect("tile");
    let x = tile.rect.x + tile.rect.width / 2;
    let y = tile.rect.y + tile.rect.height / 2;
    assert_eq!(layout.hit_test(x, y), Some(open));
    assert_eq!(layout.hit_hold(x, y), Some(details));
    assert!(layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::SectionLink(details)));
}

#[test]
fn folio_page_rail_is_passive_and_absent_for_one_page() {
    let rail = |of| {
        Screen::new(
            1,
            vec![Node::PageRail {
                id: NodeId(1),
                page: 1,
                of,
            }],
        )
        .layout()
    };
    assert!(rail(3)
        .nodes
        .iter()
        .any(|node| matches!(node.kind, LayoutKind::PageRail { page: 1, of: 3 })));
    assert!(!rail(1)
        .nodes
        .iter()
        .any(|node| matches!(node.kind, LayoutKind::PageRail { .. })));
}

#[test]
fn page_rail_reserves_a_gutter_for_the_collection_it_precedes() {
    let screen = Screen::new(
        1,
        vec![
            Node::PageRail {
                id: NodeId(1),
                page: 0,
                of: 2,
            },
            Node::TileGrid {
                id: NodeId(2),
                shape: TileShape::Card,
                tiles: vec![Tile::new(ActionId(2), "Focused", Glyph::App)],
            },
        ],
    );
    let layout = screen.layout();
    let rail = layout
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::PageRail { .. }))
        .expect("rail");
    assert!(layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Tile(..)))
        .all(|tile| tile.rect.x + tile.rect.width <= rail.rect.x));
}

#[test]
fn ordinary_buttons_are_centered_content_width_on_supported_panels_and_scales() {
    let panels = [
        CLARA_BW_METRICS,
        DisplayMetrics {
            width: 758,
            height: 1024,
            pixels_per_inch: 212,
            text_scale: TextScale::Default,
        },
    ];
    for mut metrics in panels {
        for scale in [TextScale::Default, TextScale::Large, TextScale::ExtraLarge] {
            metrics.text_scale = scale;
            let screen = Screen::new(
                1,
                vec![Node::Button {
                    id: NodeId(1),
                    action: ActionId(1),
                    label: "Try again".into(),
                    state: ControlState::Enabled,
                    emphasis: Emphasis::Normal,
                }],
            );
            let layout = screen.layout_for(&metrics);
            let button = layout
                .nodes
                .iter()
                .find(|node| matches!(node.kind, LayoutKind::Button(..)))
                .expect("button");
            assert!(button.rect.width >= metrics.touch_target_minimum());
            assert!(button.rect.height >= metrics.touch_target_minimum());
            assert!(button.rect.width < layout.content.width);
            assert_eq!(
                button.rect.x,
                layout.content.x + (layout.content.width - button.rect.width) / 2
            );
        }
    }
}

#[test]
fn primary_and_narrow_button_fallbacks_remain_full_width() {
    let primary = Node::Button {
        id: NodeId(1),
        action: ActionId(1),
        label: "Install update".into(),
        state: ControlState::Enabled,
        emphasis: Emphasis::Primary,
    };
    let narrow = Node::Button {
            id: NodeId(2),
            action: ActionId(2),
            label: "A deliberately long action label that must use the complete available width on this narrow panel".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        };
    let screen = Screen::new(1, vec![primary, narrow]);
    let layout = screen.layout();
    for node in layout
        .nodes
        .iter()
        .filter(|node| matches!(node.kind, LayoutKind::Button(..)))
    {
        assert_eq!(
            node.rect.width,
            CLARA_BW_METRICS.width - 2 * CLARA_BW_METRICS.screen_margin()
        );
    }
}

#[test]
fn legacy_screens_keep_pre_folio_button_and_tile_geometry() {
    let nodes = vec![
        Node::Button {
            id: NodeId(1),
            action: ActionId(1),
            label: "Back".into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        },
        Node::TileGrid {
            id: NodeId(2),
            shape: TileShape::Square,
            tiles: vec![Tile::new(ActionId(2), "Existing", Glyph::App)],
        },
    ];
    let modern = Screen::new(1, nodes.clone()).layout();
    let legacy = Screen::new(1, nodes).with_legacy_typography(true).layout();
    let modern_button = modern
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("modern button");
    let legacy_button = legacy
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("legacy button");
    assert!(legacy_button.rect.width > modern_button.rect.width);
    let modern_glyph = modern
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::TileGlyph(_)))
        .expect("modern glyph");
    let legacy_glyph = legacy
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::TileGlyph(_)))
        .expect("legacy glyph");
    assert!(legacy_glyph.rect.width > modern_glyph.rect.width);

    let long_label =
        "A deliberately long legacy action whose wrapping must keep the old pixel inset";
    let legacy_long = Screen::new(
        2,
        vec![Node::Button {
            id: NodeId(3),
            action: ActionId(3),
            label: long_label.into(),
            state: ControlState::Enabled,
            emphasis: Emphasis::Normal,
        }],
    )
    .with_legacy_typography(true)
    .layout();
    let legacy_long_button = legacy_long
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::Button(..)))
        .expect("long legacy button");
    assert_eq!(
        legacy_long_button.text_lines,
        wrap_text(
            long_label,
            legacy_long_button.rect.width - 32,
            FontSize::Body
        )
    );

    let legacy_splash = Screen::new(
        3,
        vec![Node::Splash {
            id: NodeId(4),
            glyph: Some(Glyph::App),
            title: "Existing app".into(),
            summary: String::new(),
        }],
    )
    .with_legacy_typography(true)
    .layout();
    let legacy_splash_glyph = legacy_splash
        .nodes
        .iter()
        .find(|node| matches!(node.kind, LayoutKind::SplashGlyph(_)))
        .expect("legacy splash glyph");
    assert_eq!(
        legacy_splash_glyph.rect.width,
        CLARA_BW_METRICS.tenth_mm(140)
    );
}

#[test]
fn legacy_section_values_keep_the_full_pre_folio_measure() {
    let value = "A deliberately long section value that needs more than half the available measure";
    let layout = Screen::new(
        1,
        vec![Node::Section {
            id: NodeId(1),
            title: "Status".into(),
            value: Some(value.into()),
            link: None,
        }],
    )
    .with_legacy_typography(true)
    .layout();
    let section = layout
        .nodes
        .iter()
        .find(|node| node.kind == LayoutKind::Section)
        .expect("legacy section");
    let expected = with_legacy_typography(true, || {
        one_line(value, section.rect.width, FontSize::Caption)
    });
    let half_width = with_legacy_typography(true, || {
        one_line(value, section.rect.width / 2, FontSize::Caption)
    });
    assert_ne!(
        expected, half_width,
        "fixture must distinguish the measures"
    );
    assert_eq!(section.text_lines.get(1), Some(&expected));
}

#[test]
fn legacy_two_line_tile_labels_render_with_caption_metrics() {
    let width = usize::try_from(CLARA_BW_METRICS.width).expect("positive width");
    let height = usize::try_from(CLARA_BW_METRICS.height).expect("positive height");
    let clip = Rect {
        x: 0,
        y: 0,
        width: CLARA_BW_METRICS.width,
        height: CLARA_BW_METRICS.height,
    };
    for shape in [TileShape::Square, TileShape::Portrait] {
        let screen = Screen::new(
            1,
            vec![Node::TileGrid {
                id: NodeId(1),
                shape,
                tiles: vec![Tile::new(
                    ActionId(1),
                    "A deliberately long existing destination label",
                    Glyph::App,
                )],
            }],
        )
        .with_legacy_typography(true);
        let layout = screen.layout();
        let label = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::TileLabel)
            .expect("legacy tile label");
        assert_eq!(label.text_lines.len(), 2);

        let mut rendered = Surface::new(width, height);
        render_with(
            &screen,
            &CLARA_BW_METRICS,
            &Chrome::default(),
            &mut rendered,
            None,
        );
        let mut expected = Surface::new(width, height);
        with_legacy_typography(true, || {
            draw_centered(
                &mut expected,
                &label.text_lines,
                label.rect,
                FontSize::Caption,
                tone::INK,
                clip,
            );
        });

        for y in label.rect.y..label.rect.y + label.rect.height {
            for x in label.rect.x..label.rect.x + label.rect.width {
                let index = usize::try_from(y * CLARA_BW_METRICS.width + x).expect("inside");
                assert_eq!(
                    rendered.pixels[index], expected.pixels[index],
                    "{shape:?} legacy tile label differs at ({x}, {y})"
                );
            }
        }
    }
}

#[test]
fn legacy_nav_glyph_is_half_a_touch_target() {
    let rect = Rect {
        x: 20,
        y: 10,
        width: 120,
        height: 100,
    };
    let clip = Rect {
        x: 0,
        y: 0,
        width: 160,
        height: 120,
    };
    let mut rendered = Surface::new(160, 120);
    let mut expected = Surface::new(160, 120);
    with_legacy_typography(true, || {
        draw_nav_label(
            &mut rendered,
            &[],
            rect,
            &CLARA_BW_METRICS,
            false,
            Some(Glyph::App),
            clip,
        );
        let line = FontSize::Caption.line_height();
        let gap = CLARA_BW_METRICS.space(Space::Tight);
        let side = min(
            CLARA_BW_METRICS.touch_target_minimum() / 2,
            max(0, rect.height - line - gap * 2),
        );
        let block = side + gap + line;
        let top = rect.y + max(0, rect.height - block) / 2;
        draw_vector(
            &mut expected,
            &vector::shapes(Glyph::App),
            Rect {
                x: rect.x + (rect.width - side) / 2,
                y: top,
                width: side,
                height: side,
            },
            clip,
            tone::INK,
        );
    });
    assert_eq!(rendered, expected);
}
