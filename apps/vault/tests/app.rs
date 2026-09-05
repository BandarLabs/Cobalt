#[path = "../src/md.rs"]
mod md;
#[path = "../src/model.rs"]
mod model;
use kobo_sdk::{action_id, Glyph, ScreenBuilder};
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
fn wiki_links_create_backlinks() {
    let notes = vec![
        Note {
            path: "Welcome.md".into(),
            body: "See [[Alpha]] and [[Projects/Alpha|the project]].".into(),
        },
        Note {
            path: "Projects/Alpha.md".into(),
            body: "Started from [[Welcome]].\n\n---\n\nStill one note.".into(),
        },
    ];
    let to_alpha = backlinks(&notes, "Projects/Alpha.md");
    assert_eq!(to_alpha.len(), 1);
    assert_eq!(to_alpha[0].0, 0);
    let to_welcome = backlinks(&notes, "Welcome.md");
    assert_eq!(to_welcome.len(), 1);
    assert_eq!(to_welcome[0].0, 1);
    assert!(to_welcome[0].1.contains("[[Welcome]]"));
}
#[test]
fn empty_or_missing_index_is_an_empty_vault() {
    assert_eq!(INDEX_KEY, "vault-index-v1");
    assert!(decode_index("").is_empty());
    assert!(decode_index("   \n").is_empty());
}
#[test]
fn thematic_breaks_survive_the_index() {
    let notes = vec![
        Note {
            path: "Welcome.md".into(),
            body: "# Welcome\n\nHome note of the fixture vault.".into(),
        },
        Note {
            path: "Projects/Alpha.md".into(),
            body: "Before.\n\n---\n\nAfter [[Welcome]].".into(),
        },
    ];
    let packed = encode_index(&notes);
    let restored = decode_index(&packed);
    assert_eq!(restored, notes);
    assert!(restored[1].body.contains("---"));
    assert_eq!(backlinks(&restored, "Welcome.md").len(), 1);
}
#[test]
fn opened_note_renders_fixture_body() {
    let note = Note {
        path: "Welcome.md".into(),
        body: "# Welcome\n\nThis is the home note of the fixture vault.".into(),
    };
    let rendered = note.rendered();
    assert!(rendered.contains("Welcome"));
    assert!(rendered.contains("home note of the fixture vault"));
    let screen = ScreenBuilder::new("vault-note")
        .top_bar(note.title())
        .reading(true)
        .text(rendered)
        .build();
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
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
#[test]
fn empty_vault_names_the_companion() {
    let screen = ScreenBuilder::new("vault-home")
        .top_bar("Vault")
        .splash(
            Some(Glyph::Note),
            "No vault yet",
            "Run kobo vault init, then kobo vault push ~/Notes.",
        )
        .build();
    assert!(screen
        .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
        .issues
        .is_empty());
}
