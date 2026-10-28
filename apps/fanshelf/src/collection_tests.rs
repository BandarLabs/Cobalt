use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{Chrome, DisplayMetrics, LayoutKind, Node, TextScale, CLARA_BW_METRICS};

pub(super) fn metrics() -> Vec<DisplayMetrics> {
    TextScale::STEPS
        .into_iter()
        .map(|text_scale| DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        })
        .collect()
}

pub(super) fn collection_app() -> Fanshelf {
    let works = (0..18)
        .map(|index| Work {
            id: (9000 + index).to_string(),
            title: format!(
                "A Map Made of Starlight: The Journey Beyond the Northern Mountains {index:02}"
            ),
            author: "Juniper Vale and Rowan Ink".into(),
            fandom: format!("The Long Road Home and Other Public Domain Adventures {index:02}"),
            chapters: 12,
            total_chapters: None,
            complete: false,
            download: DownloadState::Downloaded,
            last_checked: 1_789_617_600,
            ..Work::default()
        })
        .collect();
    let tags = (0..18)
        .map(|index| FollowedTag {
            name: format!("The Long Road Home and Other Public Domain Adventures {index:02}"),
            slug: format!("PublicDomain{index}"),
        })
        .collect();
    let feed = (0..18)
        .map(|index| FeedWork {
            id: (9100 + index).to_string(),
            title: format!(
                "A Map Made of Starlight: The Journey Beyond the Northern Mountains {index:02}"
            ),
            author: "Juniper Vale and Rowan Ink".into(),
            updated: "2026-09-02".into(),
        })
        .collect();
    Fanshelf {
        works,
        tags,
        feed,
        works_loaded: true,
        tags_loaded: true,
        demo: false,
        open_tag: Some(0),
        ..Fanshelf::default()
    }
}

pub(super) const COLLECTIONS: [(&str, View); 5] = [
    ("shelf", View::Shelf),
    ("fandoms", View::Fandoms),
    ("follow", View::Follow),
    ("feed", View::Feed),
    ("updates", View::Updates),
];

pub(super) fn emitted(commands: Vec<Command>) -> Option<Screen> {
    commands
        .into_iter()
        .rev()
        .find_map(|command| match command {
            Command::SetScreen(screen) => Some(screen),
            _ => None,
        })
}

pub(super) fn turn(runner: &mut AppRunner<Fanshelf>, screen: &mut Screen, action: ActionId) {
    if let Some(next) = emitted(runner.action(action)) {
        *screen = next;
    }
    assert_eq!(*screen, runner.app().screen(&runner.context()));
}

fn row_actions(screen: &Screen) -> Vec<ActionId> {
    screen
        .nodes
        .iter()
        .flat_map(|node| match node {
            Node::Rows { rows, .. } => rows.iter().map(|row| row.action).collect(),
            _ => Vec::new(),
        })
        .collect()
}

fn assert_reachable(screen: &Screen, metrics: DisplayMetrics) {
    let chrome = Chrome::for_screen(screen, false, Chrome::measuring(true).status);
    let screen = kobo_ui::ensure_way_back(screen.clone(), &chrome, "fanshelf");
    let diagnostics = screen.diagnostics(&metrics, &chrome);
    assert!(
        diagnostics.issues.is_empty(),
        "{}%: {:?}",
        metrics.text_scale.percent(),
        diagnostics.issues
    );
    let layout = screen.layout_with(&metrics, &chrome);
    let mut hit_rows = Vec::new();
    for node in &layout.nodes {
        let action = match node.kind {
            LayoutKind::Row(action) => {
                hit_rows.push(action);
                Some(action)
            }
            LayoutKind::Button(action, kobo_ui::ControlState::Enabled, _) => {
                assert_eq!(node.text_lines.len(), 1, "shelf navigation label wrapped");
                Some(action)
            }
            LayoutKind::BarAction(action)
            | LayoutKind::NavDestination(action, _)
            | LayoutKind::NavDestinationSelected(action, _) => Some(action),
            LayoutKind::Back => Some(ActionId::BACK),
            _ => None,
        };
        if let Some(action) = action {
            assert_eq!(
                layout.hit_test(
                    node.rect.x + node.rect.width / 2,
                    node.rect.y + node.rect.height / 2
                ),
                Some(action)
            );
        }
    }
    assert_eq!(
        hit_rows,
        row_actions(&screen),
        "a row lost its touch target"
    );
}

#[test]
fn collections_fit_and_every_item_is_reachable_at_all_nine_scales() {
    for metrics in metrics() {
        for (name, view) in COLLECTIONS {
            for notice in [None, Some("Updated EPUB saved. Reading position kept.")] {
                let mut app = collection_app();
                app.view = view;
                app.message = notice.map(str::to_owned);
                let mut runner = AppRunner::with_metrics(app, metrics);
                let mut screen = emitted(runner.start()).unwrap();
                let total = screen.page_turns.unwrap().position.unwrap().1;
                assert!(total > 1);
                let mut seen = Vec::new();
                for page in 1..=total {
                    assert_reachable(&screen, metrics);
                    assert_eq!(screen.page_turns.unwrap().position, Some((page, total)));
                    let actions = row_actions(&screen);
                    assert!(!actions.is_empty());
                    seen.extend(actions);
                    turn(&mut runner, &mut screen, action_id("page-next"));
                }
                let prefix = match view {
                    View::Shelf => "work",
                    View::Fandoms => "fandom",
                    View::Follow => "tag",
                    View::Feed => "feed",
                    View::Updates => "update",
                    _ => unreachable!(),
                };
                let expected = (0..18)
                    .map(|index| action_id(&format!("{prefix}-{index}")))
                    .collect::<Vec<_>>();
                assert_eq!(
                    seen, expected,
                    "{name}: skipped, repeated or reordered an item"
                );
                let last = screen.clone();
                for _ in 0..3 {
                    turn(&mut runner, &mut screen, action_id("page-next"));
                    assert_eq!(screen, last);
                }
                for page in (1..total).rev() {
                    turn(&mut runner, &mut screen, action_id("page-prev"));
                    assert_reachable(&screen, metrics);
                    assert_eq!(screen.page_turns.unwrap().position, Some((page, total)));
                }
                let first = screen.clone();
                for _ in 0..3 {
                    turn(&mut runner, &mut screen, action_id("page-prev"));
                    assert_eq!(screen, first);
                }
            }
        }
    }
}

#[test]
fn empty_short_and_shrinking_collections_have_no_stale_page() {
    for metrics in metrics() {
        for (_, view) in COLLECTIONS {
            let mut app = collection_app();
            app.view = view;
            app.shelf_page = usize::MAX;
            app.fandom_page = usize::MAX;
            app.tag_page = usize::MAX;
            app.feed_page = usize::MAX;
            app.updates_page = usize::MAX;
            let mut runner = AppRunner::with_metrics(app, metrics);
            let mut screen = emitted(runner.start()).unwrap();
            assert_reachable(&screen, metrics);
            let total = screen.page_turns.unwrap().position.unwrap().1;
            turn(&mut runner, &mut screen, action_id("page-prev"));
            assert_eq!(
                screen.page_turns.unwrap().position,
                Some((total - 1, total))
            );
            for count in [1, 0] {
                runner.app_mut().works.truncate(count);
                runner.app_mut().tags.truncate(count);
                runner.app_mut().feed.truncate(count);
                turn(&mut runner, &mut screen, action_id("page-next"));
                assert_reachable(&screen, metrics);
                assert_eq!(row_actions(&screen).len(), count);
                assert!(screen.page_turns.is_none());
                assert!(screen.nav_bar.is_none());
            }
        }
    }
}

#[test]
fn back_filters_and_repeated_collection_actions_preserve_the_expected_page() {
    for metrics in metrics() {
        let mut runner = AppRunner::with_metrics(collection_app(), metrics);
        let mut screen = emitted(runner.start()).unwrap();
        turn(&mut runner, &mut screen, action_id("page-next"));
        let shelf = screen.clone();
        let selected = row_actions(&screen)[0];
        turn(&mut runner, &mut screen, selected);
        assert_eq!(runner.app().view, View::Work);
        assert_eq!(
            selected,
            action_id(&format!("work-{}", runner.app().open.unwrap()))
        );
        turn(&mut runner, &mut screen, ActionId::BACK);
        assert_eq!(screen, shelf);
        turn(&mut runner, &mut screen, action_id("filter"));
        turn(&mut runner, &mut screen, action_id("page-next"));
        let fandom = row_actions(&screen)[0];
        turn(&mut runner, &mut screen, fandom);
        assert_eq!(runner.app().view, View::Shelf);
        assert_eq!(row_actions(&screen).len(), 1);
        assert_eq!(runner.app().shelf_page, 0);
        assert_reachable(&screen, metrics);
        for _ in 0..2 {
            turn(&mut runner, &mut screen, action_id("all"));
            assert_eq!(runner.app().visible().len(), 18);
            assert_eq!(runner.app().shelf_page, 0);
        }
        turn(&mut runner, &mut screen, action_id("follow"));
        turn(&mut runner, &mut screen, action_id("page-next"));
        let followed = screen.clone();
        let tag = row_actions(&screen)[0];
        turn(&mut runner, &mut screen, tag);
        assert_eq!(runner.app().view, View::Feed);
        assert_eq!(runner.app().feed_page, 0);
        turn(&mut runner, &mut screen, ActionId::BACK);
        assert_eq!(screen, followed);
        for _ in 0..2 {
            turn(&mut runner, &mut screen, action_id("updates"));
            assert_eq!(runner.app().updates_page, 0);
            assert_reachable(&screen, metrics);
            turn(&mut runner, &mut screen, ActionId::BACK);
            assert_eq!(runner.app().view, View::Shelf);
        }
    }
}
