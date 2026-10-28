use super::*;
use kobo_sdk::{AppRunner, Command};
use kobo_ui::{TextScale, CLARA_BW_METRICS};
use std::collections::BTreeSet;

fn populated(metrics: DisplayMetrics) -> AppRunner<Fieldbook> {
    let mut fixture = shelf::Shelf::decode(include_bytes!(
        "../../../scripts/fixtures/fieldbook/packs.v1"
    ))
    .unwrap();
    let first = fixture.packs.remove(0);
    AppRunner::with_metrics(
        Fieldbook {
            packs: (0..12)
                .map(|index| {
                    let mut pack = first.clone();
                    pack.title = format!("Regional field pack {}", index + 1);
                    pack
                })
                .collect(),
            outings: (0..24)
                .map(|index| Outing {
                    id: index,
                    location: format!("Marsh outing {}", index + 1),
                    date: "10/01/2026".into(),
                    start: "07:00".into(),
                })
                .collect(),
            open_outing: Some(23),
            sightings: (0..18)
                .map(|index| Sighting {
                    outing: 23,
                    code: format!("bird-{index}"),
                    common: format!("Review bird {}", index + 1),
                    scientific: format!("Avis review {}", index + 1),
                    count: 1,
                })
                .collect(),
            ..Fieldbook::default()
        },
        metrics,
    )
}
fn screen(runner: &AppRunner<Fieldbook>) -> Screen {
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
fn tap(runner: &mut AppRunner<Fieldbook>, name: &str) {
    let drawn = screen(runner);
    let metrics = runner.context().metrics();
    let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
    let layout = drawn.layout_with(&metrics, &chrome);
    let action = action_id(name);
    let rect = layout
        .rect_of_action(action)
        .unwrap_or_else(|| panic!("missing {name}"));
    let (x, y) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
    assert!(x >= 0 && x < metrics.width && y >= 0 && y < metrics.height);
    assert_eq!(layout.hit_test(x, y), Some(action));
    runner.action(action);
}
fn verify_list(runner: &mut AppRunner<Fieldbook>, prefix: &str, count: usize) {
    let mut seen = BTreeSet::new();
    let mut steps = 0;
    loop {
        let drawn = screen(runner);
        let metrics = runner.context().metrics();
        let chrome = Chrome::for_screen(&drawn, false, Chrome::measuring(true).status);
        let diagnostics = drawn.diagnostics(&metrics, &chrome);
        assert!(
            diagnostics.issues.is_empty(),
            "{metrics:?}: {:?}",
            diagnostics.issues
        );
        let layout = drawn.layout_with(&metrics, &chrome);
        for index in 0..count {
            let action = action_id(&format!("{prefix}{index}"));
            if let Some(rect) = layout.rect_of_action(action) {
                assert_eq!(
                    layout.hit_test(rect.x + rect.width / 2, rect.y + rect.height / 2),
                    Some(action)
                );
                seen.insert(index);
            }
        }
        let before = screen(runner);
        runner.page_turn(true);
        if screen(runner) == before {
            break;
        }
        steps += 1;
        assert!(steps < count + 2);
    }
    assert_eq!(seen, (0..count).collect());
    let last = screen(runner);
    runner.action(action_id("list-next"));
    runner.page_turn(true);
    assert_eq!(screen(runner), last);
}

#[test]
fn complete_outing_pack_sighting_and_life_lists_fit_every_text_size() {
    for text_scale in TextScale::STEPS {
        let mut runner = populated(DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        });
        verify_list(&mut runner, "outing-", 24);
        let page = runner.app().home_page;
        tap(&mut runner, "outing-0");
        assert_eq!(runner.app().open_outing, Some(0));
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
        assert_eq!(runner.app().home_page, page);
        tap(&mut runner, "packs");
        verify_list(&mut runner, "pack-", 12);
        tap(&mut runner, "pack-11");
        assert_eq!(runner.app().pack_pick, Some(11));
        assert_eq!(runner.app().view, View::Search);
        runner.action(ActionId::BACK);
        runner.app_mut().open_outing = Some(23);
        runner.app_mut().view = View::Sightings;
        verify_list(&mut runner, "sight-", 18);
        runner.app_mut().view = View::Life;
        verify_list(&mut runner, "life-", 18);
        tap(&mut runner, "life-17");
        assert_eq!(runner.app().view, View::Detail);
        assert_eq!(runner.app().detail.as_ref().unwrap().code, "bird-17");
    }
}

#[test]
fn sighting_removal_requires_confirmation_and_is_undoable() {
    for text_scale in TextScale::STEPS {
        let mut runner = populated(DisplayMetrics {
            text_scale,
            ..CLARA_BW_METRICS
        });
        runner.app_mut().view = View::Sightings;
        verify_list(&mut runner, "sight-", 18);
        tap(&mut runner, "sight-17");
        assert_eq!(runner.app().sightings.len(), 18);
        assert_eq!(runner.app().delete_candidate, Some(17));
        let shown = screen(&runner);
        let metrics = runner.context().metrics();
        let chrome = Chrome::for_screen(&shown, false, Chrome::measuring(true).status);
        assert!(!shown.diagnostics(&metrics, &chrome).has_errors());
        runner.action(action_id("sight-17"));
        assert_eq!(runner.app().sightings.len(), 18);
        runner.action(ActionId::BACK);
        assert!(runner.app().delete_candidate.is_none());
        assert_eq!(runner.app().view, View::Sightings);
        tap(&mut runner, "sight-17");
        tap(&mut runner, "keep-sighting");
        assert_eq!(runner.app().sightings.len(), 18);
        tap(&mut runner, "sight-17");
        tap(&mut runner, "delete-sighting");
        assert_eq!(runner.app().sightings.len(), 17);
        runner.action(action_id("delete-sighting"));
        assert_eq!(runner.app().sightings.len(), 17);
        tap(&mut runner, "undo");
        assert_eq!(runner.app().sightings.len(), 18);
        assert_eq!(runner.app().sightings[17].code, "bird-17");
        runner.action(action_id("undo"));
        assert_eq!(runner.app().sightings.len(), 18);
    }
}

#[test]
fn failure_notice_and_an_empty_collection_keep_navigation_reachable() {
    let mut runner = populated(DisplayMetrics {
        text_scale: TextScale::Largest,
        ..CLARA_BW_METRICS
    });
    runner.app_mut().save = SaveState::Failed;
    verify_list(&mut runner, "outing-", 24);
    tap(&mut runner, "packs");
    verify_list(&mut runner, "pack-", 12);
    runner.app_mut().packs.clear();
    tap(&mut runner, "life");
    runner.app_mut().sightings.clear();
    tap(&mut runner, "home");
    tap(&mut runner, "packs");
    tap(&mut runner, "life");
    tap(&mut runner, "home");
    assert_eq!(runner.app().view, View::Home);
}

#[test]
fn distinct_manual_species_remain_visible_and_countable() {
    let mut runner = populated(CLARA_BW_METRICS);
    runner.app_mut().sightings.clear();
    runner.app_mut().pack_pick = None;
    for name in ["Robin", "Crow", "robin"] {
        runner.app_mut().query = name.into();
        runner.action(action_id("log-manual"));
    }
    assert_eq!(runner.app().sightings.len(), 2);
    assert_eq!(runner.app().sightings[0].common, "Robin");
    assert_eq!(runner.app().sightings[0].count, 2);
    assert_eq!(runner.app().sightings[1].common, "Crow");
    assert_eq!(runner.app().sightings[1].count, 1);
    assert_eq!(runner.app().outing_totals(23), (2, 3));
    assert_eq!(runner.app().outing_species().len(), 2);
    assert_eq!(runner.app().life_totals().len(), 2);
    let export = runner.app().checklist_csv();
    assert!(export.contains("Robin,"));
    assert!(export.contains("Crow,"));
    assert!(same_species("AMRO", "Robin", "AMRO", "American Robin"));
    assert!(!same_species("", "Robin", "", "Crow"));
}

#[test]
fn existing_and_previously_merged_records_are_preserved_without_guessing() {
    let saved = b"23||Robin||7\n23|AMRO|American Robin|Turdus migratorius|3".to_vec();
    let mut app = Fieldbook {
        open_outing: Some(23),
        ..Fieldbook::default()
    };
    let mut context = Context::default();
    app.on_store(
        &mut context,
        StoreResult::Loaded {
            key: SIGHTINGS.into(),
            value: Some(saved.clone()),
        },
    );
    assert_eq!(app.sightings.len(), 2);
    assert_eq!(app.sightings[0].count, 7);
    app.persist(&mut context);
    let roundtrip = context
        .commands()
        .iter()
        .rev()
        .find_map(|command| match command {
            Command::Store(kobo_sdk::StoreRequest::Save { key, value }) if key == SIGHTINGS => {
                Some(value)
            }
            _ => None,
        })
        .expect("sighting save");
    assert_eq!(*roundtrip, saved);
    app.query = "Crow".into();
    app.log_manual(&mut context);
    assert_eq!(app.sightings.len(), 3);
    assert_eq!(app.sightings[0].common, "Robin");
    assert_eq!(app.sightings[0].count, 7);
    assert_eq!(app.sightings[2].common, "Crow");
    assert_eq!(app.sightings[2].count, 1);
}
