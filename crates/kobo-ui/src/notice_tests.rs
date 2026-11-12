use super::*;

fn panels() -> Vec<(String, DisplayMetrics)> {
    let scales = [
        TextScale::Default,
        TextScale::ExtraLarge,
        TextScale::Largest,
    ];
    kobo_profile::SUPPORTED_PROFILES
        .iter()
        .flat_map(|profile| {
            scales.into_iter().flat_map(move |text_scale| {
                let portrait = DisplayMetrics {
                    width: i32::try_from(profile.width).expect("profile width fits layout"),
                    height: i32::try_from(profile.height).expect("profile height fits layout"),
                    pixels_per_inch: i32::from(profile.pixels_per_inch),
                    text_scale,
                };
                let landscape = DisplayMetrics {
                    width: portrait.height,
                    height: portrait.width,
                    ..portrait
                };
                [
                    (
                        format!("{} portrait at {text_scale:?}", profile.id),
                        portrait,
                    ),
                    (
                        format!("{} landscape at {text_scale:?}", profile.id),
                        landscape,
                    ),
                ]
            })
        })
        .collect()
}

const OPEN: ActionId = ActionId(41);
const APPS: ActionId = ActionId(42);

/// A launcher-shaped screen: a bar with a control, rows, and tabs at the foot.
fn underneath() -> Screen {
    let mut screen = Screen::new(
        1,
        vec![Node::Rows {
            id: NodeId(2),
            rows: (0..6)
                .map(|index| {
                    Row::new(
                        OPEN,
                        format!("Row {index}"),
                        "Underneath the card",
                        Glyph::Note,
                    )
                })
                .collect(),
        }],
    );
    screen.top_bar = Some(TopBar::new(NodeId(3), "Cobalt").action(ActionId(43), "Edit"));
    screen.nav_bar = Some(NavBar::new(
        NodeId(4),
        vec![
            BarAction::new(APPS, "Apps").with_glyph(Glyph::App),
            BarAction::new(ActionId(44), "Books").with_glyph(Glyph::Book),
        ],
        Some(0),
    ));
    screen
}

fn chrome(line: &str) -> Chrome {
    Chrome::with_back(false)
        .with_status(Status::standard(
            "08:13",
            Some(Percent::new(80)),
            false,
            Signal::Strong,
            false,
        ))
        .with_notice(Some(NoticeCard {
            glyph: Glyph::Book,
            heading: "Article arrived".to_owned(),
            line: line.to_owned(),
        }))
}

const LONG: &str = "How the river found its way to the sea, and what the town that grew up beside it learnt from listening to the water for three hundred years";

#[test]
fn a_tap_on_the_notice_is_the_notice_and_a_tap_anywhere_else_reaches_the_screen_underneath() {
    for (name, metrics) in panels() {
        let layout = underneath().layout_with(&metrics, &chrome(LONG));
        let card = notice_rect(&layout).unwrap_or_else(|| panic!("{name}: no card"));
        for (x, y) in [
            (card.x + 2, card.y + 2),
            (card.x + card.width / 2, card.y + card.height / 2),
            (card.x + card.width - 8, card.y + card.height - 8),
        ] {
            assert_eq!(layout.hit_test(x, y), Some(ActionId::NOTICE), "{name}");
        }
        let without = underneath().layout_with(
            &metrics,
            &Chrome {
                notice: None,
                ..chrome(LONG)
            },
        );
        // Every control the card does not cover answers exactly as it did.
        for node in &without.nodes {
            let Some(action) = node.kind.acts_on() else {
                continue;
            };
            let below = node.rect.y + node.rect.height - 2;
            let centre = node.rect.x + node.rect.width / 2;
            if card.contains(centre, below) {
                continue;
            }
            assert_eq!(
                layout.hit_test(centre, below),
                Some(action),
                "{name}: {node:?}"
            );
        }
    }
}

#[test]
fn the_notice_sits_under_the_status_band_and_never_reaches_the_tabs_at_the_foot() {
    for (name, metrics) in panels() {
        let layout = underneath().layout_with(&metrics, &chrome(LONG));
        let card = notice_rect(&layout).expect("a card");
        let band = layout
            .nodes
            .iter()
            .find(|node| node.kind == LayoutKind::StatusBand)
            .map(|node| node.rect)
            .expect("a status band");
        assert!(card.y >= band.y + band.height, "{name}: card over the band");
        let tabs = layout
            .nodes
            .iter()
            .filter(|node| node.kind.acts_on() == Some(APPS))
            .map(|node| node.rect.y)
            .min()
            .expect("tabs");
        assert!(card.y + card.height < tabs, "{name}: card reaches the tabs");
        assert!(
            card.x >= 0 && card.x + card.width <= metrics.width,
            "{name}"
        );
        assert!(
            card.height >= metrics.touch_target_default(),
            "{name}: card shorter than a target"
        );
        // A quiet card, not a takeover: never a quarter of the panel.
        assert!(card.height * 4 < metrics.height, "{name}: card is {card:?}");
    }
}

#[test]
fn the_notice_says_two_lines_at_most_and_ellipsises_the_title_rather_than_wrapping_it() {
    for (name, metrics) in panels() {
        let layout = underneath().layout_with(&metrics, &chrome(LONG));
        let words: Vec<&LayoutNode> = layout
            .nodes
            .iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    LayoutKind::NoticeHeading | LayoutKind::NoticeLine
                )
            })
            .collect();
        assert_eq!(words.len(), 2, "{name}");
        assert!(
            words.iter().all(|node| node.text_lines.len() == 1),
            "{name}"
        );
        assert_eq!(words[0].text_lines[0], "Article arrived", "{name}");
        assert!(words[1].text_lines[0].ends_with('\u{2026}'), "{name}");
        let card = notice_rect(&layout).expect("a card");
        for node in words {
            let size = if node.kind == LayoutKind::NoticeHeading {
                notice::HEADING
            } else {
                notice::LINE
            };
            let fits = with_text_scale(metrics.text_scale, || {
                measure_text(&node.text_lines[0], size).0
            });
            assert!(
                node.rect.x + node.rect.width <= card.x + card.width,
                "{name}"
            );
            assert!(fits <= node.rect.width + 1, "{name}: {:?}", node.text_lines);
        }
    }
}

#[test]
fn a_notice_with_only_a_heading_is_still_a_whole_target() {
    let metrics = CLARA_BW_METRICS;
    let layout = underneath().layout_with(&metrics, &chrome(""));
    let card = notice_rect(&layout).expect("a card");
    assert!(card.height >= metrics.touch_target_default());
    assert!(!layout
        .nodes
        .iter()
        .any(|node| node.kind == LayoutKind::NoticeLine));
}

#[test]
fn drawing_the_notice_changes_only_the_pixels_inside_its_card() {
    let metrics = CLARA_BW_METRICS;
    let size = (
        usize::try_from(metrics.width).expect("width"),
        usize::try_from(metrics.height).expect("height"),
    );
    let mut plain = Surface::new(size.0, size.1);
    let mut covered = Surface::new(size.0, size.1);
    let without = Chrome {
        notice: None,
        ..chrome(LONG)
    };
    render_all(&underneath(), &metrics, &without, &(), &mut plain, None);
    render_all(
        &underneath(),
        &metrics,
        &chrome(LONG),
        &(),
        &mut covered,
        None,
    );
    let card = notice_rect(&underneath().layout_with(&metrics, &chrome(LONG))).expect("a card");
    let mut inside = 0;
    for y in 0..size.1 {
        for x in 0..size.0 {
            let index = y * size.0 + x;
            if plain.pixels[index] == covered.pixels[index] {
                continue;
            }
            let (x, y) = (i32::try_from(x).expect("x"), i32::try_from(y).expect("y"));
            assert!(
                card.contains(x, y),
                "a pixel changed outside the card at {x},{y}"
            );
            inside += 1;
        }
    }
    assert!(inside > 0, "the card drew nothing");
}
