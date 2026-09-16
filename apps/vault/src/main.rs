//! Vault: a reader for a folder of Markdown notes, published onto the shelf
//! by the companion CLI or ingested from what Sync delivers.

mod md;
mod model;
mod shelf;

use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, RowLead, Screen, ScreenBuilder, ShelfDownload,
    ShelfProgress, StoreResult,
};
use model::{link_matches_path, Note};
use shelf::{ImportFailure, Manifest, NoteEntry};
use std::collections::BTreeMap;
use std::process::ExitCode;

/// The pre-shelf index the first Vault kept in the app store, still read so a
/// vault packed by an older CLI is not stranded.
const LEGACY_INDEX_KEY: &str = "vault-index-v1";
const STATE: &str = "vault-state-v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Browse,
    Note,
    Tags,
    TagNotes,
    Recent,
    Search,
    Backlinks,
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Source {
    Pushed,
    Synced,
}

impl Source {
    const fn label(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::Synced => "synced",
        }
    }
}

/// Why a note body is on its way down: the reader opened the note, or a
/// search is loading every body it has not seen yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Loading {
    Open,
    Search,
}

type Rows = Vec<(String, String, String, RowLead)>;

struct Vault {
    loaded: bool,
    pushed: Option<Manifest>,
    synced: Option<Manifest>,
    /// Notes from both shelves, pushed first; bodies load lazily by id.
    notes: Vec<(NoteEntry, Source)>,
    bodies: BTreeMap<String, String>,
    manifest_load: Option<(Source, ShelfDownload)>,
    body_load: Option<(String, Loading, ShelfDownload)>,
    /// Notes a search still needs the bodies of, drained one at a time.
    search_queue: Vec<String>,
    pending_search: Option<String>,
    view: View,
    origin: View,
    browse_stack: Vec<String>,
    pages: BTreeMap<u8, usize>,
    current: usize,
    note_page: usize,
    positions: BTreeMap<String, usize>,
    tag_filter: String,
    entry: TextEntry,
    results: Vec<(usize, String)>,
    legacy_checked: bool,
}

impl Default for Vault {
    fn default() -> Self {
        Self {
            loaded: false,
            pushed: None,
            synced: None,
            notes: Vec::new(),
            bodies: BTreeMap::new(),
            manifest_load: None,
            body_load: None,
            search_queue: Vec::new(),
            pending_search: None,
            view: View::Home,
            origin: View::Home,
            browse_stack: Vec::new(),
            pages: BTreeMap::new(),
            current: 0,
            note_page: 0,
            positions: BTreeMap::new(),
            tag_filter: String::new(),
            entry: TextEntry::new().opened_by("search"),
            results: Vec::new(),
            legacy_checked: false,
        }
    }
}

impl Vault {
    fn show(&self, cx: &mut Context) {
        let screen = self.screen(cx);
        cx.set_screen(screen);
    }

    fn page(&self, view: View) -> usize {
        self.pages.get(&(view as u8)).copied().unwrap_or(0)
    }

    fn set_page(&mut self, view: View, page: usize) {
        self.pages.insert(view as u8, page);
    }

    fn current_entry(&self) -> Option<&(NoteEntry, Source)> {
        self.notes.get(self.current)
    }

    fn deduped_tags(&self) -> Vec<(String, usize)> {
        let mut tags: Vec<(String, usize)> = Vec::new();
        for (entry, _) in &self.notes {
            for tag in &entry.tags {
                match tags.iter_mut().find(|(known, _)| known == tag) {
                    Some((_, count)) => *count += 1,
                    None => tags.push((tag.clone(), 1)),
                }
            }
        }
        tags.sort_by(|left, right| left.0.cmp(&right.0));
        tags
    }

    fn tag_matches(&self, tag: &str) -> Vec<usize> {
        self.notes
            .iter()
            .enumerate()
            .filter_map(|(index, (entry, _))| {
                entry.tags.iter().any(|known| known == tag).then_some(index)
            })
            .collect()
    }

    /// Immediate subfolders of the browse position and the notes directly in
    /// it, so folders are drilled into rather than flattened.
    fn browse_level(&self) -> (Vec<String>, Vec<usize>) {
        let prefix = self.browse_stack.join("/");
        let mut folders: Vec<String> = Vec::new();
        let mut notes = Vec::new();
        for (index, (entry, _)) in self.notes.iter().enumerate() {
            let rest = if prefix.is_empty() {
                entry.path.as_str()
            } else if let Some(rest) = entry.path.strip_prefix(&format!("{prefix}/")) {
                rest
            } else {
                continue;
            };
            if let Some((folder, _)) = rest.split_once('/') {
                if !folders.iter().any(|known| known == folder) {
                    folders.push(folder.to_owned());
                }
            } else {
                notes.push(index);
            }
        }
        folders.sort();
        (folders, notes)
    }

    fn backlinks(&self, index: usize) -> Vec<usize> {
        let Some((target, _)) = self.notes.get(index) else {
            return Vec::new();
        };
        self.notes
            .iter()
            .enumerate()
            .filter_map(|(other, (entry, _))| {
                (other != index
                    && entry
                        .links
                        .iter()
                        .any(|link| link_matches_path(link, &target.path)))
                .then_some(other)
            })
            .collect()
    }

    fn start_manifest(&mut self, cx: &mut Context, source: Source) {
        let name = match source {
            Source::Pushed => shelf::MANIFEST,
            Source::Synced => shelf::SYNCED_MANIFEST,
        };
        let mut load = ShelfDownload::new(name).at_most(shelf::MAX_MANIFEST);
        load.start(cx);
        self.manifest_load = Some((source, load));
    }

    fn start_body(&mut self, cx: &mut Context, id: &str, why: Loading) {
        let mut load = ShelfDownload::new(shelf::note_name(id)).at_most(shelf::MAX_NOTE_BYTES);
        load.start(cx);
        self.body_load = Some((id.to_owned(), why, load));
    }

    fn open_note(&mut self, cx: &mut Context, index: usize) {
        if self.view != View::Note && self.view != View::Backlinks {
            self.origin = self.view;
        }
        self.current = index;
        let id = self.notes[index].0.id.clone();
        self.note_page = self.positions.get(&id).copied().unwrap_or(0);
        if self.bodies.contains_key(&id) {
            self.view = View::Note;
        } else {
            self.start_body(cx, &id, Loading::Open);
        }
        self.show(cx);
    }

    fn remember_position(&mut self, cx: &mut Context) {
        if let Some((entry, _)) = self.current_entry() {
            self.positions.insert(entry.id.clone(), self.note_page);
            let mut out = String::new();
            for (id, page) in &self.positions {
                out.push_str(&format!("pos {id} {page}\n"));
            }
            cx.store().save(STATE, out.into_bytes());
        }
    }

    fn note_pages(&self, cx: &Context) -> Vec<Vec<String>> {
        let Some((entry, _)) = self.current_entry() else {
            return Vec::new();
        };
        let Some(body) = self.bodies.get(&entry.id) else {
            return Vec::new();
        };
        cx.paginate_reading(&md::render(body), true)
    }

    fn run_search(&mut self, query: &str) {
        let indexed: Vec<(usize, Note)> = self
            .notes
            .iter()
            .enumerate()
            .filter_map(|(index, (entry, _))| {
                self.bodies.get(&entry.id).map(|body| {
                    (
                        index,
                        Note {
                            path: entry.path.clone(),
                            body: body.clone(),
                        },
                    )
                })
            })
            .collect();
        let models: Vec<Note> = indexed.iter().map(|(_, note)| note.clone()).collect();
        self.results = model::search(&models, query)
            .into_iter()
            .map(|(model_index, line)| (indexed[model_index].0, line))
            .collect();
        self.view = View::Search;
    }

    fn paged_rows(&self, cx: &Context, view: View, rows: &Rows, screen: ScreenBuilder) -> Screen {
        let borrowed: Vec<(&str, &str)> = rows
            .iter()
            .map(|(_, title, detail, _)| (title.as_str(), detail.as_str()))
            .collect();
        let chunks = cx.paginate_rows(&borrowed, true);
        let page = self.page(view).min(chunks.len().saturating_sub(1));
        let shown = chunks.get(page).map(Vec::as_slice).unwrap_or_default();
        let mut screen = screen.rows(shown.iter().filter_map(|index| rows.get(*index).cloned()));
        if chunks.len() > 1 {
            screen = screen
                .secondary(format!("Page {} of {}", page + 1, chunks.len()))
                .buttons([
                    ("list-prev", "Previous page".to_owned()),
                    ("list-next", "Next page".to_owned()),
                ]);
        }
        screen.build().with_own_back(true)
    }

    #[allow(clippy::too_many_lines)]
    fn screen(&self, cx: &mut Context) -> Screen {
        if self.entry.is_open() {
            return ScreenBuilder::new("vault-search")
                .top_bar("Search")
                .text_entry(&self.entry, "Find text", "Search")
                .build();
        }
        if !self.loaded || self.manifest_load.is_some() {
            return ScreenBuilder::new("vault-home")
                .top_bar("Vault")
                .skeleton(4)
                .build();
        }
        if let Some((_, why, _)) = &self.body_load {
            let screen = ScreenBuilder::new("vault-opening").top_bar("Vault");
            return match why {
                Loading::Open => screen.activity("Opening the note", None).build(),
                Loading::Search => screen
                    .activity(
                        format!(
                            "Loading notes to search ({} left)",
                            self.search_queue.len() + 1
                        ),
                        None,
                    )
                    .build(),
            };
        }
        match self.view {
            View::Home => self.home_screen(),
            View::About => self.about_screen(),
            View::Browse => {
                let (folders, notes) = self.browse_level();
                let mut rows: Rows = Vec::new();
                let prefix = self.browse_stack.join("/");
                for folder in &folders {
                    let base = if prefix.is_empty() {
                        format!("{folder}/")
                    } else {
                        format!("{prefix}/{folder}/")
                    };
                    let count = self
                        .notes
                        .iter()
                        .filter(|(entry, _)| entry.path.starts_with(&base))
                        .count();
                    rows.push((
                        format!("dir-{folder}"),
                        folder.clone(),
                        format!("{count} note(s)"),
                        RowLead::Icon(Glyph::Folder),
                    ));
                }
                for index in &notes {
                    let (entry, source) = &self.notes[*index];
                    rows.push((
                        format!("note-{index}"),
                        entry.title.clone(),
                        format!("{} · {}", entry.path, source.label()),
                        RowLead::Icon(Glyph::Note),
                    ));
                }
                let title = if self.browse_stack.is_empty() {
                    "Browse".to_owned()
                } else {
                    format!("Browse / {}", self.browse_stack.join(" / "))
                };
                let screen = ScreenBuilder::new("vault-browse").top_bar(title);
                if rows.is_empty() {
                    screen
                        .text("This folder is empty.")
                        .build()
                        .with_own_back(true)
                } else {
                    self.paged_rows(cx, View::Browse, &rows, screen)
                }
            }
            View::Note => self.note_screen(cx),
            View::Tags => {
                let tags = self.deduped_tags();
                let screen = ScreenBuilder::new("vault-tags").top_bar("Tags");
                if tags.is_empty() {
                    screen
                        .text("No tags in this vault yet.")
                        .build()
                        .with_own_back(true)
                } else {
                    let rows: Rows = tags
                        .iter()
                        .map(|(tag, count)| {
                            (
                                format!("tag-{tag}"),
                                format!("#{tag}"),
                                format!("{count} note(s)"),
                                RowLead::Icon(Glyph::Note),
                            )
                        })
                        .collect();
                    self.paged_rows(cx, View::Tags, &rows, screen)
                }
            }
            View::TagNotes => {
                let rows: Rows = self
                    .tag_matches(&self.tag_filter)
                    .iter()
                    .map(|index| {
                        let (entry, source) = &self.notes[*index];
                        (
                            format!("note-{index}"),
                            entry.title.clone(),
                            format!("{} · {}", entry.path, source.label()),
                            RowLead::Icon(Glyph::Note),
                        )
                    })
                    .collect();
                let screen =
                    ScreenBuilder::new("vault-tag-notes").top_bar(format!("#{}", self.tag_filter));
                self.paged_rows(cx, View::TagNotes, &rows, screen)
            }
            View::Recent => {
                let mut order: Vec<usize> = (0..self.notes.len()).collect();
                order.sort_by(|left, right| {
                    self.notes[*right].0.added.cmp(&self.notes[*left].0.added)
                });
                let rows: Rows = order
                    .iter()
                    .map(|index| {
                        let (entry, source) = &self.notes[*index];
                        (
                            format!("note-{index}"),
                            entry.title.clone(),
                            format!("{} · {}", entry.path, source.label()),
                            RowLead::Icon(Glyph::Clock),
                        )
                    })
                    .collect();
                let screen = ScreenBuilder::new("vault-recent").top_bar("Recent");
                self.paged_rows(cx, View::Recent, &rows, screen)
            }
            View::Search => {
                let rows: Rows = self
                    .results
                    .iter()
                    .map(|(index, line)| {
                        (
                            format!("note-{index}"),
                            self.notes[*index].0.title.clone(),
                            line.clone(),
                            RowLead::Icon(Glyph::Search),
                        )
                    })
                    .collect();
                let screen = ScreenBuilder::new("vault-search-results").top_bar("Search");
                if rows.is_empty() {
                    screen
                        .text("Nothing in this vault matches.")
                        .button("search", "Search again")
                        .build()
                        .with_own_back(true)
                } else {
                    self.paged_rows(cx, View::Search, &rows, screen)
                }
            }
            View::Backlinks => {
                let rows: Rows = self
                    .backlinks(self.current)
                    .iter()
                    .map(|index| {
                        let (entry, source) = &self.notes[*index];
                        (
                            format!("note-{index}"),
                            entry.title.clone(),
                            format!("{} · {}", entry.path, source.label()),
                            RowLead::Icon(Glyph::Note),
                        )
                    })
                    .collect();
                let title = self.current_entry().map_or_else(
                    || "Backlinks".to_owned(),
                    |(entry, _)| format!("Links to {}", entry.title),
                );
                let screen = ScreenBuilder::new("vault-backlinks").top_bar(title);
                if rows.is_empty() {
                    screen
                        .text("No note links here yet.")
                        .build()
                        .with_own_back(true)
                } else {
                    self.paged_rows(cx, View::Backlinks, &rows, screen)
                }
            }
        }
    }

    fn home_screen(&self) -> Screen {
        let pushed = self.pushed.as_ref().map_or(0, |m| m.notes.len());
        let synced = self.synced.as_ref().map_or(0, |m| m.notes.len());
        let screen = ScreenBuilder::new("vault-home").top_bar("Vault");
        if self.notes.is_empty() {
            return screen
                .splash(
                    Some(Glyph::Note),
                    "No vault yet",
                    "Run kobo vault init, then kobo vault push ~/Notes. Notes Sync delivers are ingested with kobo vault ingest.",
                )
                .button("about", "About Vault")
                .build();
        }
        let mut screen = screen;
        let failures: usize = self
            .pushed
            .iter()
            .chain(self.synced.iter())
            .map(|m| m.failures.len())
            .sum();
        if failures > 0 {
            screen = screen.banner(
                kobo_sdk::BannerLevel::Attention,
                format!("{failures} note(s) did not import; see About Vault."),
            );
        }
        let mut heading = format!("{pushed} notes");
        if synced > 0 {
            heading.push_str(&format!(" + {synced} synced"));
        }
        screen
            .heading(heading)
            .rows([
                ("browse", "Browse", "Folders and notes", Glyph::Folder),
                ("tags", "Tags", "Notes by tag", Glyph::Note),
                ("recent", "Recent", "Last changed notes", Glyph::Clock),
                ("search", "Search", "Find text in this vault", Glyph::Search),
            ])
            .button("about", "About Vault")
            .build()
    }

    fn about_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("vault-about")
            .top_bar("About Vault")
            .text(
                "Push a folder of Markdown notes with kobo vault push <folder>. The host \
                 folder is the source of truth; edits made here stay here. Notes Sync \
                 delivers are ingested with kobo vault ingest.",
            );
        let failures: Vec<&ImportFailure> = self
            .pushed
            .iter()
            .chain(self.synced.iter())
            .flat_map(|m| m.failures.iter())
            .collect();
        if !failures.is_empty() {
            screen = screen
                .heading("Did not import")
                .rows(failures.iter().enumerate().map(|(index, failure)| {
                    (
                        format!("failure-{index}"),
                        failure.input.clone(),
                        failure.reason.clone(),
                        RowLead::Icon(Glyph::Note),
                    )
                }));
        }
        screen.build().with_own_back(true)
    }

    fn note_screen(&self, cx: &Context) -> Screen {
        let Some((entry, _)) = self.current_entry() else {
            return ScreenBuilder::new("vault-note")
                .top_bar("Vault")
                .banner(
                    kobo_sdk::BannerLevel::Attention,
                    "This note is no longer on the shelf.",
                )
                .button("home", "Vault")
                .build()
                .with_own_back(true);
        };
        let pages = self.note_pages(cx);
        let index = self.note_page.min(pages.len().saturating_sub(1));
        let origin_label = match self.origin {
            View::Browse => "Browse",
            View::Tags | View::TagNotes => "Tags",
            View::Recent => "Recent",
            View::Search => "Search results",
            _ => "Vault",
        };
        let backlinks = self.backlinks(self.current).len();
        let mut screen = ScreenBuilder::new("vault-note")
            .top_bar(entry.title.clone())
            .reading(true)
            .page_position(
                u16::try_from(index + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
            );
        if backlinks > 0 {
            screen = screen.top_bar_action("backlinks", format!("Links ({backlinks})"));
        }
        if let Some(paragraphs) = pages.get(index) {
            for paragraph in paragraphs {
                screen = screen.text(paragraph);
            }
        }
        screen
            .action_bar([
                ("previous-page", "Previous".to_owned()),
                ("origin", origin_label.to_owned()),
                ("next-page", "Next".to_owned()),
            ])
            .build()
            .with_own_back(true)
    }
}

impl KoboApp for Vault {
    fn on_start(&mut self, cx: &mut Context) {
        cx.store().load(STATE);
        self.start_manifest(cx, Source::Pushed);
        self.show(cx);
    }

    #[allow(clippy::too_many_lines)]
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = &result {
            if key == STATE {
                if let Some(text) = value.as_deref().and_then(|v| std::str::from_utf8(v).ok()) {
                    for line in text.lines() {
                        if let Some(rest) = line.strip_prefix("pos ") {
                            if let Some((id, page)) = rest.rsplit_once(' ') {
                                if let Ok(page) = page.parse::<usize>() {
                                    self.positions.insert(id.to_owned(), page);
                                }
                            }
                        }
                    }
                }
                return;
            }
            if key == LEGACY_INDEX_KEY {
                let raw = value
                    .as_deref()
                    .and_then(|v| String::from_utf8(v.to_vec()).ok())
                    .unwrap_or_default();
                for note in model::decode_index(&raw) {
                    let id = format!("legacy-{}", self.notes.len());
                    let entry = NoteEntry {
                        path: note.path.clone(),
                        title: note.title(),
                        tags: note.tags(),
                        links: note.links(),
                        id: id.clone(),
                        digest: String::new(),
                        bytes: note.body.len() as u64,
                        added: 0,
                    };
                    self.bodies.insert(id, note.body);
                    self.notes.push((entry, Source::Pushed));
                }
                self.loaded = true;
                self.show(cx);
                return;
            }
        }
        if let Some((source, load)) = &mut self.manifest_load {
            match load.advance(cx, &result) {
                ShelfProgress::Done => {
                    let (source, load) = self.manifest_load.take().expect("active manifest");
                    let bytes = load.take();
                    let manifest = Manifest::decode(&bytes).unwrap_or_else(|_| Manifest {
                        notes: Vec::new(),
                        failures: vec![ImportFailure {
                            input: "shelf manifest".to_owned(),
                            reason: "could not be read".to_owned(),
                        }],
                    });
                    if source == Source::Pushed {
                        self.pushed = Some(manifest);
                        self.start_manifest(cx, Source::Synced);
                    } else {
                        self.synced = Some(manifest);
                        self.finish_loading(cx);
                    }
                    self.show(cx);
                }
                ShelfProgress::Failed(_) => {
                    let source = *source;
                    self.manifest_load = None;
                    if source == Source::Pushed {
                        self.start_manifest(cx, Source::Synced);
                    } else {
                        self.finish_loading(cx);
                    }
                    self.show(cx);
                }
                _ => {}
            }
            return;
        }
        let why = self.body_load.as_ref().map(|(_, why, _)| *why);
        if let Some(why) = why {
            match load_done(&mut self.body_load, cx, &result) {
                BodyOutcome::Ready(id, body) => {
                    self.bodies.insert(id, body);
                    match why {
                        Loading::Open => self.view = View::Note,
                        Loading::Search => self.advance_search(cx),
                    }
                    self.show(cx);
                }
                BodyOutcome::Failed => {
                    if why == Loading::Search {
                        self.advance_search(cx);
                    } else {
                        self.view = self.origin;
                    }
                    self.show(cx);
                }
                BodyOutcome::Waiting => {}
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        if let Some(event) = self.entry.handle(a) {
            if let Typing::Submitted(query) = event {
                let missing: Vec<String> = self
                    .notes
                    .iter()
                    .filter(|(entry, _)| !self.bodies.contains_key(&entry.id))
                    .map(|(entry, _)| entry.id.clone())
                    .collect();
                if missing.is_empty() {
                    self.run_search(&query);
                } else {
                    self.search_queue = missing;
                    self.pending_search = Some(query);
                    let id = self.search_queue.remove(0);
                    self.start_body(cx, &id, Loading::Search);
                }
            }
            self.show(cx);
            return;
        }
        if a == action_id("browse") {
            self.browse_stack.clear();
            self.view = View::Browse;
        } else if a == action_id("tags") {
            self.view = View::Tags;
        } else if a == action_id("recent") {
            self.view = View::Recent;
        } else if a == action_id("search") {
            self.entry.open();
        } else if a == action_id("about") {
            self.view = View::About;
        } else if a == action_id("home") {
            self.view = View::Home;
        } else if a == action_id("backlinks") {
            self.view = View::Backlinks;
        } else if a == action_id("origin") {
            self.remember_position(cx);
            self.view = self.origin;
        } else if a == action_id("previous-page") || a == action_id("next-page") {
            let pages = self.note_pages(cx);
            let current = self.note_page.min(pages.len().saturating_sub(1));
            let next = if a == action_id("next-page") {
                (current + 1).min(pages.len().saturating_sub(1))
            } else {
                current.saturating_sub(1)
            };
            if next != current {
                self.note_page = next;
                self.remember_position(cx);
            }
        } else if a == action_id("list-prev") || a == action_id("list-next") {
            let view = self.view;
            let page = self.page(view);
            let next = if a == action_id("list-next") {
                page + 1
            } else {
                page.saturating_sub(1)
            };
            self.set_page(view, next);
        } else if a == ActionId::BACK {
            match self.view {
                View::Note => {
                    self.remember_position(cx);
                    self.view = self.origin;
                }
                View::Backlinks => self.view = View::Note,
                View::TagNotes => self.view = View::Tags,
                View::Browse if !self.browse_stack.is_empty() => {
                    self.browse_stack.pop();
                    self.set_page(View::Browse, 0);
                }
                View::Home => {}
                _ => self.view = View::Home,
            }
        } else {
            for (index, _) in self.notes.iter().enumerate() {
                if a == action_id(&format!("note-{index}")) {
                    self.open_note(cx, index);
                    return;
                }
            }
            let (folders, _) = self.browse_level();
            for folder in folders {
                if a == action_id(&format!("dir-{folder}")) {
                    self.browse_stack.push(folder);
                    self.set_page(View::Browse, 0);
                    self.show(cx);
                    return;
                }
            }
            for (tag, _) in self.deduped_tags() {
                if a == action_id(&format!("tag-{tag}")) {
                    self.tag_filter = tag;
                    self.view = View::TagNotes;
                    self.show(cx);
                    return;
                }
            }
        }
        self.show(cx);
    }
}

enum BodyOutcome {
    Ready(String, String),
    Failed,
    Waiting,
}

fn load_done(
    slot: &mut Option<(String, Loading, ShelfDownload)>,
    cx: &mut Context,
    result: &StoreResult,
) -> BodyOutcome {
    let Some((_, _, load)) = slot else {
        return BodyOutcome::Waiting;
    };
    match load.advance(cx, result) {
        ShelfProgress::Done => {
            let (id, _, load) = slot.take().expect("active note");
            match String::from_utf8(load.take()) {
                Ok(body) => BodyOutcome::Ready(id, body),
                Err(_) => BodyOutcome::Failed,
            }
        }
        ShelfProgress::Failed(_) => {
            slot.take();
            BodyOutcome::Failed
        }
        _ => BodyOutcome::Waiting,
    }
}

impl Vault {
    fn advance_search(&mut self, cx: &mut Context) {
        if let Some(id) = self.search_queue.first().cloned() {
            self.search_queue.remove(0);
            self.start_body(cx, &id, Loading::Search);
        } else if let Some(query) = self.pending_search.take() {
            self.run_search(&query);
        }
    }

    fn finish_loading(&mut self, cx: &mut Context) {
        if let Some(manifest) = self.pushed.clone() {
            for entry in manifest.notes {
                self.notes.push((entry, Source::Pushed));
            }
        }
        if let Some(manifest) = self.synced.clone() {
            for entry in manifest.notes {
                self.notes.push((entry, Source::Synced));
            }
        }
        if self.notes.is_empty() && !self.legacy_checked {
            self.legacy_checked = true;
            cx.store().load(LEGACY_INDEX_KEY);
            return;
        }
        self.loaded = true;
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

    fn entry(id: &str, path: &str, title: &str, tags: &[&str], links: &[&str]) -> NoteEntry {
        NoteEntry {
            id: id.to_owned(),
            path: path.to_owned(),
            title: title.to_owned(),
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
            links: links.iter().map(|link| (*link).to_owned()).collect(),
            digest: String::new(),
            bytes: 10,
            added: 1,
        }
    }

    fn seeded() -> AppRunner<Vault> {
        let mut runner = AppRunner::new(Vault::default());
        {
            let app = runner.app_mut();
            app.pushed = Some(Manifest {
                notes: vec![
                    entry("note-a", "Welcome.md", "Welcome", &["home"], &["Alpha"]),
                    entry(
                        "note-b",
                        "Projects/Alpha.md",
                        "Alpha",
                        &["project", "active"],
                        &["Welcome"],
                    ),
                    entry("note-c", "Projects/Beta.md", "Beta", &["project"], &[]),
                ],
                failures: Vec::new(),
            });
            app.synced = Some(Manifest {
                notes: vec![entry(
                    "synced-note-s",
                    "Inbox/Synced.md",
                    "Synced",
                    &["inbox"],
                    &[],
                )],
                failures: Vec::new(),
            });
            app.finish_loading(&mut Context::default());
            app.bodies.insert(
                "note-a".to_owned(),
                "# Welcome\n\nHome note. See [[Alpha]].".to_owned(),
            );
            app.bodies.insert(
                "note-b".to_owned(),
                "# Alpha\n\nA project note linking to [[Welcome]].".to_owned(),
            );
        }
        runner
    }

    #[test]
    fn both_shelves_merge_with_their_source() {
        let runner = seeded();
        assert_eq!(runner.app().notes.len(), 4);
        assert_eq!(runner.app().notes[3].1, Source::Synced);
        assert_eq!(runner.app().pushed.as_ref().map_or(0, |m| m.notes.len()), 3);
        assert_eq!(runner.app().synced.as_ref().map_or(0, |m| m.notes.len()), 1);
    }

    #[test]
    fn tags_dedupe_and_filter() {
        let runner = seeded();
        let tags = runner.app().deduped_tags();
        assert_eq!(
            tags,
            vec![
                ("active".to_owned(), 1),
                ("home".to_owned(), 1),
                ("inbox".to_owned(), 1),
                ("project".to_owned(), 2),
            ]
        );
        assert_eq!(runner.app().tag_matches("project"), vec![1, 2]);
    }

    #[test]
    fn folders_drill_down() {
        let runner = seeded();
        let (folders, notes) = runner.app().browse_level();
        assert_eq!(folders, vec!["Inbox", "Projects"]);
        assert_eq!(notes, vec![0]);
        let mut runner = runner;
        runner.app_mut().browse_stack.push("Projects".to_owned());
        let (folders, notes) = runner.app().browse_level();
        assert!(folders.is_empty());
        assert_eq!(notes, vec![1, 2]);
    }

    #[test]
    fn backlinks_answer_from_the_manifest() {
        let runner = seeded();
        assert_eq!(runner.app().backlinks(0), vec![1]);
        assert_eq!(runner.app().backlinks(1), vec![0]);
    }

    #[test]
    fn action_graph_reaches_every_view() {
        let mut runner = seeded();
        runner.action(action_id("browse"));
        assert_eq!(runner.app().view, View::Browse);
        runner.action(action_id("dir-Projects"));
        assert_eq!(runner.app().browse_stack, vec!["Projects"]);
        runner.action(action_id("note-1"));
        assert_eq!(runner.app().view, View::Note);
        runner.action(action_id("backlinks"));
        assert_eq!(runner.app().view, View::Backlinks);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Note);
        runner.action(action_id("origin"));
        assert_eq!(runner.app().view, View::Browse);
        runner.action(ActionId::BACK);
        assert!(runner.app().browse_stack.is_empty());
        runner.action(action_id("tags"));
        runner.action(action_id("tag-project"));
        assert_eq!(runner.app().view, View::TagNotes);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Tags);
        runner.action(action_id("recent"));
        assert_eq!(runner.app().view, View::Recent);
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
    }

    #[test]
    fn reading_position_is_remembered_per_note() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().view, View::Note);
        runner.app_mut().note_page = 3;
        runner.action(action_id("origin"));
        assert_eq!(runner.app().positions.get("note-a"), Some(&3));
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().note_page, 3);
    }
}
