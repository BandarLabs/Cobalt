mod md;
mod model;
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, StoreResult};
use model::{backlinks, decode_index, search, Note, INDEX_KEY};
use std::process::ExitCode;
#[derive(Clone, Copy, Debug, PartialEq)]
enum View {
    Home,
    Browse,
    Note,
    Tags,
    Recent,
    Search,
    Backlinks,
}
struct Vault {
    notes: Vec<Note>,
    loaded: bool,
    view: View,
    current: usize,
    entry: TextEntry,
    results: Vec<(usize, String)>,
}
impl Default for Vault {
    fn default() -> Self {
        Self {
            notes: vec![],
            loaded: false,
            view: View::Home,
            current: 0,
            entry: TextEntry::new().opened_by("search"),
            results: vec![],
        }
    }
}
impl Vault {
    fn show(&self, cx: &mut Context) {
        cx.set_screen(self.screen());
    }
    #[allow(clippy::too_many_lines)]
    fn screen(&self) -> Screen {
        if self.entry.is_open() {
            return ScreenBuilder::new("vault-search")
                .top_bar("Search")
                .text_entry(&self.entry, "Find text", "Search")
                .build();
        }
        if !self.loaded {
            return ScreenBuilder::new("vault-home")
                .top_bar("Vault")
                .skeleton(4)
                .build();
        }
        match self.view {
            View::Home => {
                let s = ScreenBuilder::new("vault-home").top_bar("Vault");
                if self.notes.is_empty() {
                    s.splash(
                        Some(Glyph::Note),
                        "No vault yet",
                        "Run kobo vault init, then kobo vault push ~/Notes.",
                    )
                    .build()
                } else {
                    s.heading(format!("{} notes", self.notes.len()))
                        .rows([
                            ("browse", "Browse", "Folders and notes", Glyph::Folder),
                            ("tags", "Tags", "Notes by tag", Glyph::Note),
                            ("recent", "Recent", "Last pushed notes", Glyph::Clock),
                            ("search", "Search", "Find text in this vault", Glyph::Search),
                        ])
                        .build()
                }
            }
            View::Browse => {
                ScreenBuilder::new("vault-browse")
                    .top_bar("Browse")
                    .rows(self.notes.iter().enumerate().map(|(i, n)| {
                        (format!("note-{i}"), n.title(), n.path.clone(), Glyph::Note)
                    }))
                    .build()
            }
            View::Note => {
                let n = &self.notes[self.current];
                ScreenBuilder::new("vault-note")
                    .top_bar(n.title())
                    .reading(true)
                    .text(n.rendered())
                    .buttons([
                        (
                            "backlinks",
                            format!("Backlinks ({})", backlinks(&self.notes, &n.path).len()),
                        ),
                        ("tags", "Tags".to_owned()),
                    ])
                    .build()
            }
            View::Tags => ScreenBuilder::new("vault-tags")
                .top_bar("Tags")
                .rows(self.notes.iter().flat_map(model::Note::tags).map(|tag| {
                    (
                        format!("tag-{tag}"),
                        format!("#{tag}"),
                        "Open matching notes".to_owned(),
                        Glyph::Note,
                    )
                }))
                .build(),
            View::Recent => {
                ScreenBuilder::new("vault-recent")
                    .top_bar("Recent")
                    .rows(self.notes.iter().enumerate().rev().map(|(i, n)| {
                        (format!("note-{i}"), n.title(), n.path.clone(), Glyph::Clock)
                    }))
                    .build()
            }
            View::Search => ScreenBuilder::new("vault-search-results")
                .top_bar("Search")
                .rows(self.results.iter().map(|(i, line)| {
                    (
                        format!("note-{i}"),
                        self.notes[*i].title(),
                        line.clone(),
                        Glyph::Search,
                    )
                }))
                .button("search", "Search again")
                .build(),
            View::Backlinks => {
                let n = &self.notes[self.current];
                ScreenBuilder::new("vault-backlinks")
                    .top_bar("Backlinks")
                    .rows(backlinks(&self.notes, &n.path).iter().map(|(i, line)| {
                        (
                            format!("note-{i}"),
                            self.notes[*i].title(),
                            line.clone(),
                            Glyph::Note,
                        )
                    }))
                    .build()
            }
        }
    }
}
impl KoboApp for Vault {
    fn on_start(&mut self, cx: &mut Context) {
        cx.store().load(INDEX_KEY);
        self.show(cx);
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == INDEX_KEY {
                self.notes = value
                    .and_then(|v| String::from_utf8(v).ok())
                    .map(|raw| decode_index(&raw))
                    .unwrap_or_default();
                self.loaded = true;
                self.show(cx);
            }
        }
    }
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        if let Some(event) = self.entry.handle(a) {
            if let Typing::Submitted(query) = event {
                self.results = search(&self.notes, &query);
                self.view = View::Search;
            }
            self.show(cx);
            return;
        }
        if a == action_id("browse") {
            self.view = View::Browse;
        } else if a == action_id("tags") {
            self.view = View::Tags;
        } else if a == action_id("recent") {
            self.view = View::Recent;
        } else if a == action_id("search") {
            self.entry.open();
        } else if a == action_id("backlinks") {
            self.view = View::Backlinks;
        } else {
            for i in 0..self.notes.len() {
                if a == action_id(&format!("note-{i}")) {
                    self.current = i;
                    self.view = View::Note;
                }
            }
        }
        self.show(cx);
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("vault", Vault::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vault: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::AppRunner;

    fn seeded() -> AppRunner<Vault> {
        let mut runner = AppRunner::new(Vault::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: INDEX_KEY.into(),
            value: Some(
                "Welcome.md\n# Welcome\n\nhome note of the fixture. See [[Alpha]].\n\n---vault-note---\n\nProjects/Alpha.md\n# Alpha\n\nA project with a wiki link to [[Welcome]].\n\n#project\n"
                    .as_bytes()
                    .to_vec(),
            ),
        });
        runner
    }

    #[test]
    fn action_graph_reaches_every_view() {
        let mut runner = seeded();
        assert_eq!(runner.app().view, View::Home);
        assert_eq!(runner.app().notes.len(), 2);
        runner.action(action_id("browse"));
        assert_eq!(runner.app().view, View::Browse);
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().view, View::Note);
        runner.action(action_id("backlinks"));
        assert_eq!(runner.app().view, View::Backlinks);
        runner.action(action_id("tags"));
        assert_eq!(runner.app().view, View::Tags);
        runner.action(action_id("recent"));
        assert_eq!(runner.app().view, View::Recent);
        runner.action(action_id("search"));
        assert!(runner.app().entry.is_open());
    }
}
