//! In-process navigation/renderer coverage for the selected Stats tab.
use super::*;
use kobo_sdk::{AppRunner, Node};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn selected(screen: &Screen) -> usize {
    screen
        .nodes
        .iter()
        .find_map(|node| match node {
            Node::Tabs { selected, .. } => Some(*selected),
            _ => None,
        })
        .expect("visible tabs")
}

#[test]
fn every_tab_selects_its_matching_destination() {
    let mut runner = AppRunner::new(Habits {
        loaded: true,
        ..Habits::default()
    });
    for _ in 0..2 {
        for (index, (name, _)) in Page::all().iter().enumerate() {
            runner.action(action_id(name));
            assert_eq!(selected(&runner.app().screen()), index, "{name}");
        }
    }
    runner.action(action_id("settings"));
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().page, Page::Stats);
    assert_eq!(selected(&runner.app().screen()), 3);
    runner.action(ActionId::BACK);
    assert_eq!(selected(&runner.app().screen()), 0);
}

#[test]
fn stats_selection_and_targets_fit_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let mut runner = AppRunner::with_metrics(
            Habits {
                loaded: true,
                ..Habits::default()
            },
            metrics,
        );
        runner.action(action_id("stats"));
        let screen = runner.app().screen();
        assert_eq!(selected(&screen), 3);
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        assert!(screen.diagnostics(&metrics, &chrome).issues.is_empty());
        let layout = screen.layout_with(&metrics, &chrome);
        for (name, _) in Page::all() {
            let target = layout.rect_of_action(action_id(name)).expect("tab target");
            assert!(target.height >= metrics.touch_target_minimum());
        }
    }
}

#[test]
fn capture_review_page_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let mut runner = AppRunner::new(Habits {
        loaded: true,
        items: vec![
            Habit::new("Read a chapter".into()),
            Habit::new("Walk around the neighborhood".into()),
            Habit::new("Make tea and reflect on the day".into()),
        ],
        ..Habits::default()
    });
    runner.action(action_id("stats"));
    let screen = runner.app().screen();
    let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(screen, &chrome, "Habits");
    let mut surface = kobo_ui::Surface::new(1072, 1448);
    kobo_ui::render_all(
        &screen,
        &CLARA_BW_METRICS,
        &chrome,
        &kobo_ui::PictureCache::default(),
        &mut surface,
        None,
    );
    let png = kobo_image::encode_png_grey(1072, 1448, &surface.pixels).unwrap();
    std::fs::write(output.join("stats.png"), png).unwrap();
    std::fs::write(
        output.join("stats.diagnostics.txt"),
        format!("{:#?}", screen.diagnostics(&CLARA_BW_METRICS, &chrome)),
    )
    .unwrap();
}
