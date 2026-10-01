//! In-process app/renderer coverage. These are not interactive simulator tests.
use super::*;
use kobo_sdk::{AppRunner, DiagnosticSeverity};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};

fn entities(count: usize) -> Vec<ha::Entity> {
    (0..count)
        .map(|n| ha::Entity {
            id: format!("light.room_{n:02}"),
            name: format!("Room {n:02} ceiling light"),
            state: "off".into(),
        })
        .collect()
}

fn assert_fits(screen: &Screen, metrics: DisplayMetrics) {
    let errors: Vec<_> = screen
        .diagnostics(&metrics, &Chrome::measuring(true))
        .issues
        .into_iter()
        .filter(|issue| issue.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(errors.is_empty(), "{:?}: {errors:#?}", screen.id);
}

#[test]
fn every_discovered_device_is_reachable_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(HomePanel::default(), metrics);
        let context = runner.context();
        let mut app = HomePanel {
            view: View::Add,
            entities: entities(100),
            ..HomePanel::default()
        };
        let pages = app.picker_pages(&context);
        assert_eq!(
            pages.iter().flatten().copied().collect::<Vec<_>>(),
            (0..100).collect::<Vec<_>>()
        );
        for (page, indices) in pages.iter().enumerate() {
            app.picker_page = page;
            let screen = app.add(&context);
            assert_fits(&screen, metrics);
            let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
            for &index in indices {
                let action = action_id(&format!("entity.light.room_{index:02}"));
                assert!(
                    layout.rect_of_action(action).is_some(),
                    "device {index} on page {page}"
                );
            }
        }
    }
}

#[test]
fn all_wall_tiles_and_error_notices_fit_at_every_text_size() {
    for text_scale in TextScale::STEPS {
        let metrics = DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        };
        let runner = AppRunner::with_metrics(HomePanel::default(), metrics);
        let context = runner.context();
        for wall in [false, true] {
            let mut app = HomePanel {
                view: View::Grid,
                wall,
                tiles: (0..12).map(|n| format!("light.room_{n:02}")).collect(),
                banner: Some(
                    "Home Assistant did not answer. Check the address and that it is running."
                        .into(),
                ),
                ..HomePanel::default()
            };
            let pages = app.grid_pages(&context);
            assert_eq!(
                pages.iter().flatten().copied().collect::<Vec<_>>(),
                (0..12).collect::<Vec<_>>()
            );
            for (page, indices) in pages.iter().enumerate() {
                app.grid_page = page;
                let screen = app.grid(&context);
                assert_fits(&screen, metrics);
                let layout = screen.layout_with(&metrics, &Chrome::measuring(true));
                for &index in indices {
                    assert!(layout
                        .rect_of_action(action_id(&format!("tile.light.room_{index:02}")))
                        .is_some());
                }
            }
        }
    }
}

#[test]
fn picker_navigation_clamps_resets_search_and_keeps_back_behavior() {
    let mut runner = AppRunner::new(HomePanel {
        view: View::Add,
        entities: entities(20),
        ..HomePanel::default()
    });
    let last = runner.app().picker_pages(&runner.context()).len() - 1;
    for _ in 0..50 {
        runner.action(action_id("page-next"));
    }
    assert_eq!(runner.app().picker_page, last);
    runner.action(action_id(SEARCH));
    runner.app_mut().keyboard = Keyboard::with_text("room_19");
    runner.action(action_id("kb.enter"));
    assert_eq!(runner.app().picker_page, 0);
    assert_eq!(runner.app().picker_rows().len(), 1);
    runner.action(action_id("entity.light.room_19"));
    assert!(runner.app().view == View::Grid);
    assert_eq!(runner.app().tiles, ["light.room_19"]);
    runner.app_mut().view = View::Add;
    runner.action(ActionId::BACK);
    assert!(runner.app().view == View::Grid);
    for _ in 0..50 {
        runner.action(action_id("page-previous"));
    }
    assert_eq!(runner.app().grid_page, 0);
}

#[test]
fn capture_review_pages_when_requested() {
    let Ok(output) = std::env::var("COBALT_REVIEW_OUT") else {
        return;
    };
    let output = std::path::PathBuf::from(output);
    std::fs::create_dir_all(&output).unwrap();
    let runner = AppRunner::new(HomePanel::default());
    let context = runner.context();
    let mut app = HomePanel {
        view: View::Add,
        entities: entities(20),
        ..HomePanel::default()
    };
    let mut screens = vec![("picker-first", app.add(&context))];
    app.picker_page = app.picker_pages(&context).len() - 1;
    screens.push(("picker-last", app.add(&context)));
    app.view = View::Grid;
    app.tiles = (0..12).map(|n| format!("light.room_{n:02}")).collect();
    app.wall = true;
    screens.push(("wall-first", app.grid(&context)));
    app.grid_page = app.grid_pages(&context).len() - 1;
    screens.push(("wall-last", app.grid(&context)));
    for (name, screen) in screens {
        let chrome = Chrome::for_screen(&screen, false, Chrome::measuring(true).status);
        let screen = kobo_ui::ensure_way_back(screen, &chrome, "Home Panel");
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
        std::fs::write(output.join(format!("{name}.png")), png).unwrap();
        std::fs::write(
            output.join(format!("{name}.diagnostics.txt")),
            format!("{:#?}", screen.diagnostics(&CLARA_BW_METRICS, &chrome)),
        )
        .unwrap();
    }
}
