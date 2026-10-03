use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{Chrome, DisplayMetrics, TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn screen(runner: &AppRunner<Grimoire>) -> Screen {
    let mut context = runner.context();
    runner.app().show(&mut context);
    context
        .commands()
        .iter()
        .find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen.clone()),
            _ => None,
        })
        .expect("shown screen")
}
fn tap(runner: &mut AppRunner<Grimoire>, name: &str) {
    let shown = screen(runner);
    let metrics = runner.context().metrics();
    let chrome = Chrome::for_screen(&shown, false, Chrome::measuring(true).status);
    let layout = shown.layout_with(&metrics, &chrome);
    let action = action_id(name);
    let rect = layout
        .rect_of_action(action)
        .unwrap_or_else(|| panic!("missing {name}"));
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert!(x >= 0 && x < metrics.width && y >= 0 && y < metrics.height);
    assert_eq!(layout.hit_test(x, y), Some(action));
    runner.action(action);
}

#[test]
fn every_bookmark_is_reachable_and_reading_returns_to_the_same_page() {
    for text_scale in TextScale::STEPS {
        let mut runner = AppRunner::with_metrics(
            Grimoire {
                bookmarks: (0..25).collect(),
                view: View::Compendium,
                page: 3,
                ..Grimoire::default()
            },
            DisplayMetrics {
                text_scale,
                ..CLARA_BW_METRICS
            },
        );
        runner.action(action_id("bookmarks"));
        assert_eq!(runner.app().bookmarks_page, 0);
        let mut seen = BTreeSet::new();
        loop {
            let shown = screen(&runner);
            let metrics = runner.context().metrics();
            let chrome = Chrome::for_screen(&shown, false, Chrome::measuring(true).status);
            let diagnostics = shown.diagnostics(&metrics, &chrome);
            assert!(
                diagnostics.issues.is_empty(),
                "{text_scale:?}: {:?}",
                diagnostics.issues
            );
            let layout = shown.layout_with(&metrics, &chrome);
            for index in 0..25 {
                let action = action_id(&format!("entry-{index}"));
                if let Some(rect) = layout.rect_of_action(action) {
                    assert_eq!(
                        layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                        Some(action)
                    );
                    seen.insert(index);
                }
            }
            let page = runner.app().bookmarks_page;
            runner.page_turn(true);
            if runner.app().bookmarks_page == page {
                break;
            }
            assert!(runner.app().bookmarks_page < 25);
        }
        assert_eq!(seen, (0..25).collect());
        let last = runner.app().bookmarks_page;
        runner.action(action_id("bookmarks-next"));
        assert_eq!(runner.app().bookmarks_page, last);
        tap(&mut runner, "entry-24");
        assert_eq!(runner.app().view, View::Detail);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Bookmarks);
        assert_eq!(runner.app().bookmarks_page, last);
        assert_eq!(runner.app().page, 3);
        for _ in 0..=last {
            runner.page_turn(false);
        }
        assert_eq!(runner.app().bookmarks_page, 0);
        tap(&mut runner, "back");
        assert_eq!(runner.app().view, View::Home);
    }
}

#[test]
fn a_shortened_or_invalid_bookmark_list_stays_safe() {
    let mut runner = AppRunner::new(Grimoire {
        bookmarks: (0..25).collect(),
        bookmarks_page: 20,
        view: View::Bookmarks,
        bookmarks_open: true,
        ..Grimoire::default()
    });
    runner.app_mut().bookmarks.truncate(1);
    runner.action(action_id("bookmarks-previous"));
    assert_eq!(runner.app().bookmarks_page, 0);
    tap(&mut runner, "entry-0");
    runner.action(ActionId::BACK);
    assert_eq!(runner.app().view, View::Bookmarks);
    runner.app_mut().bookmarks = vec![usize::MAX];
    runner.action(action_id("bookmarks-next"));
    assert_eq!(runner.app().bookmarks, vec![usize::MAX]);
    let shown = screen(&runner);
    assert!(format!("{shown:?}").contains("No bookmarks"));
    tap(&mut runner, "back");
    assert_eq!(runner.app().view, View::Home);
}
