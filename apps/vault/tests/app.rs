#[path = "../src/md.rs"]
mod md;
#[path = "../src/model.rs"]
mod model;
use kobo_sdk::{action_id, ScreenBuilder};
use kobo_ui::{Chrome, CLARA_BW_METRICS};
use model::*;
#[test]
fn markdown_extensions_survive_the_renderer() {
    let text = md::render("# Note\n\n- [x] done\n\n| a | b |\n|---|---|\n| 1 | 2 |");
    assert!(text.contains("Note"));
    assert!(text.contains("done"));
}
#[test]
fn search_caps_hits_and_backlinks_find_normal_links() {
    let notes = vec![
        Note {
            path: "one.md".into(),
            body: "links [two](two.md) #work".into(),
        },
        Note {
            path: "two.md".into(),
            body: "needle".into(),
        },
    ];
    assert_eq!(backlinks(&notes, "two.md").len(), 1);
    assert_eq!(search(&notes, "NEEDLE")[0].0, 1);
    assert_eq!(notes[0].title(), "one");
    assert!(notes[0].tags().contains(&"work".to_owned()));
    assert!(notes[1].rendered().contains("needle"));
}
#[test]
fn clara_bw_rows_fit() {
    let screen = ScreenBuilder::new("vault-home")
        .top_bar("Vault")
        .grid(1, false, [("browse", "Browse"), ("search", "Search")])
        .build();
    let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
    for a in ["browse", "search"] {
        assert!(
            layout.rect_of_action(action_id(a)).expect("row").height
                >= CLARA_BW_METRICS.touch_target_minimum()
        );
    }
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}
