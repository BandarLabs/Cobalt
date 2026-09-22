mod md;
mod model;
use kobo_sdk::exports::{Export, Format};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, Context, Glyph, KoboApp, Screen, ScreenBuilder, ShelfDownload,
    ShelfProgress, StoreResult,
};
use model::{
    decode_manifest, decode_pending, encode_pending, NoteMeta, PendingEdit, ReadFilter, Sort,
    MANIFEST, MAX_BODY, MAX_MANIFEST, PENDING_KEY,
};
use std::process::ExitCode;

/// How many notes' text layout `extend_home_pages` measures per call. Small
/// enough that even the device's slower CPU stays well under the 250 ms
/// lifecycle-callback budget; see the comment on `Clippings::pages`.
const HOME_PAGINATION_CHUNK: usize = 60;

/// How many bytes of the open note's rendered body `extend_note_pages`
/// measures per call. Measured on host: 32 KiB of rendered text paginates
/// in ~5ms, against ~650ms for a full 4 MiB body measured in one call --
/// the gap `md::render` silently closed by truncating every note to 8 KiB
/// until its doc comment explains why that was wrong. Small enough to stay
/// well under the 250 ms budget on the device's slower CPU with real margin
/// left over for the chunk's own render and screen-diff costs.
const NOTE_PAGINATION_CHUNK: usize = 32 * 1024;

/// Tags shown per page of the Filter screen's tag picker. Tag labels are
/// short and single-line, unlike a note title, so this is a plain fixed-size
/// slice rather than the real text-layout pagination `extend_home_pages`
/// needs for rows whose height actually varies.
const FILTER_TAGS_PER_PAGE: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Note,
    Filter,
}

struct Clippings {
    notes: Vec<NoteMeta>,
    manifest_loaded: bool,
    manifest_load: Option<ShelfDownload>,
    pending_loaded: bool,
    pending: Vec<PendingEdit>,
    sort: Sort,
    /// What Home's read state is narrowed to. Defaults to `Unread`.
    read_filter: ReadFilter,
    /// The one tag Home is narrowed to, if any. Independent of
    /// `read_filter`: a reader can be looking at unread notes tagged
    /// "recipes" at the same time.
    tag_filter: Option<String>,
    /// Which page of the Filter screen's tag list is showing. Reset to 0
    /// whenever that screen opens.
    filter_tag_page: usize,
    /// `self.notes` indices matching both filters, in sort order. Cheap to
    /// compute in full (a plain filter and sort, no text layout) -- unlike
    /// `pages`, never chunked.
    order: Vec<usize>,
    /// Page boundaries as positions into `order` (`pages[p]` lists which
    /// positions in `order` are on page `p`). Cached rather than recomputed
    /// on every `screen()` call, and built incrementally rather than for all
    /// of `order` at once: measuring a row's text layout to find page breaks
    /// is real work, and doing it for the whole real vault (~3,500 notes) in
    /// one lifecycle callback measured over 1 second on the actual device --
    /// well past the 250 ms budget -- even though a reader only ever looks at
    /// a handful of rows per page. See `extend_home_pages`.
    pages: Vec<Vec<usize>>,
    /// How many entries of `order` have been measured into `pages` so far.
    paginated_through: usize,
    /// Which page of `pages` is showing. Reset to 0 whenever `order` is
    /// rebuilt (manifest load, sort change), since the underlying layout
    /// changed.
    page: usize,
    view: View,
    current: usize,
    body: Option<String>,
    body_load: Option<ShelfDownload>,
    body_load_id: Option<String>,
    /// The open note's body, rendered to plain text once and then measured
    /// in `NOTE_PAGINATION_CHUNK`-sized slices across possibly several
    /// lifecycle callbacks -- see `extend_note_pages`. `None` once the whole
    /// thing has been consumed into `note_pages`; kept until then since
    /// re-rendering per chunk would mean re-running the Markdown parser
    /// over the whole body on every call instead of once.
    note_rendered: Option<String>,
    /// How many bytes of `note_rendered` have been measured into
    /// `note_pages` so far.
    note_paginated_through: usize,
    /// The open note's rendered body, broken into pages the reading face
    /// actually fits -- physical page-turn buttons only reach a screen that
    /// declares `page_turns`; without pages to turn between, they arrive as
    /// the runtime's raw `Message::PageTurn` and, unhandled, do nothing.
    note_pages: Vec<Vec<String>>,
    /// Which of `note_pages` is showing. Reset to 0 whenever a note is opened
    /// or its body finishes loading.
    note_page: usize,
    /// Set when the open note's body failed to load or was not readable
    /// text. Checked before `body` in the note view, so a failure shows the
    /// reason instead of leaving the loading skeleton up forever -- `body`
    /// alone can't tell "still loading" apart from "never going to load".
    body_error: Option<String>,
    entry: TextEntry,
    export: Option<Export>,
    /// Set when `pending` changes while `export` is still outstanding (not
    /// `is_ready()`). An `Export` is a single owner-initiated slot whose
    /// callbacks the runtime matches by key, not by which `Export` value
    /// asked for them (see `kobo_sdk::exports`'s doc comment); replacing
    /// `self.export` while one is still in flight would abandon those
    /// callbacks, or worse, let them land on the replacement's state. Left
    /// set until the outstanding export reaches `is_ready()`, at which point
    /// `on_save`/`on_shelf` publish a fresh one from the latest `pending`.
    export_dirty: bool,
    notice: Option<String>,
}
impl Default for Clippings {
    fn default() -> Self {
        Self {
            notes: vec![],
            manifest_loaded: false,
            manifest_load: None,
            pending_loaded: false,
            pending: vec![],
            sort: Sort::Published,
            read_filter: ReadFilter::default(),
            tag_filter: None,
            filter_tag_page: 0,
            order: vec![],
            pages: vec![],
            paginated_through: 0,
            page: 0,
            view: View::Home,
            current: 0,
            body: None,
            body_load: None,
            body_load_id: None,
            note_rendered: None,
            note_paginated_through: 0,
            note_pages: vec![],
            note_page: 0,
            body_error: None,
            entry: TextEntry::new().opened_by("add-tag"),
            export: None,
            export_dirty: false,
            notice: None,
        }
    }
}
impl Clippings {
    fn loaded(&self) -> bool {
        self.manifest_loaded && self.pending_loaded
    }
    /// Starts fetching the manifest fresh, unless one is already in flight.
    fn refresh_manifest(&mut self, cx: &mut Context) {
        if self.manifest_load.is_some() {
            return;
        }
        let mut manifest = ShelfDownload::new(MANIFEST).at_most(MAX_MANIFEST);
        manifest.start(cx);
        self.manifest_load = Some(manifest);
    }
    fn pending_index(&self, path: &str) -> Option<usize> {
        self.pending.iter().position(|edit| edit.path == path)
    }
    fn is_read(&self, note: &NoteMeta) -> bool {
        self.pending_index(&note.path)
            .map_or(note.read, |i| self.pending[i].read)
    }
    fn added_tags(&self, note: &NoteMeta) -> Vec<String> {
        self.pending_index(&note.path)
            .map(|i| self.pending[i].added_tags.clone())
            .unwrap_or_default()
    }
    /// Both filters apply together: a reader can be looking at unread notes
    /// tagged "recipes" at the same time, not one or the other.
    fn matches_filter(&self, note: &NoteMeta) -> bool {
        let read_matches = match self.read_filter {
            ReadFilter::Unread => !self.is_read(note),
            ReadFilter::Read => self.is_read(note),
            ReadFilter::All => true,
        };
        let tag_matches = self.tag_filter.as_ref().is_none_or(|tag| {
            note.tags.iter().any(|t| t == tag) || self.added_tags(note).iter().any(|t| t == tag)
        });
        read_matches && tag_matches
    }
    /// Every tag on any note, including ones only pending (typed but not yet
    /// synced), sorted and deduplicated. Cheap enough (plain string
    /// comparisons over a personal tag vocabulary, not the whole vault) to
    /// call straight from `screen()` rather than caching.
    fn available_tags(&self) -> Vec<String> {
        let mut set = std::collections::BTreeSet::new();
        for note in &self.notes {
            set.extend(note.tags.iter().cloned());
            set.extend(self.added_tags(note));
        }
        set.into_iter().collect()
    }
    /// A new edit starts from the note's own current read state, not a
    /// hardcoded default -- otherwise tagging an already-read note (the only
    /// caller that doesn't also set `read` itself) would silently mark it
    /// unread in the pending edit, and export that unintended state to the
    /// companion plugin.
    fn edit(&mut self, path: &str, base_read: bool) -> &mut PendingEdit {
        if self.pending_index(path).is_none() {
            self.pending.push(PendingEdit {
                path: path.to_owned(),
                read: base_read,
                added_tags: vec![],
            });
        }
        let i = self.pending_index(path).expect("just inserted");
        &mut self.pending[i]
    }
    fn toggle_read(&mut self, cx: &mut Context) {
        let note = &self.notes[self.current];
        let was_read = self.is_read(note);
        let path = note.path.clone();
        let base_read = note.read;
        let edit = self.edit(&path, base_read);
        edit.read = !was_read;
        if edit.read == base_read && edit.added_tags.is_empty() {
            let i = self.pending_index(&path).expect("just edited");
            self.pending.remove(i);
        }
        self.save_pending(cx);
        // A read toggle can change whether this note still matches the
        // active filter (Unread, most of all) -- Home's shelf has to catch
        // up before the reader sees it again.
        self.recompute_home_pages(cx);
        self.sync_pending(cx);
    }
    fn add_tag(&mut self, cx: &mut Context, tag: &str) {
        let tag = tag.trim();
        if tag.is_empty() {
            return;
        }
        let note = &self.notes[self.current];
        if note.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            return;
        }
        let path = note.path.clone();
        let base_read = note.read;
        let edit = self.edit(&path, base_read);
        if !edit.added_tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            edit.added_tags.push(tag.to_owned());
        }
        self.save_pending(cx);
        // A new tag can be exactly the one a tag filter is narrowed to.
        self.recompute_home_pages(cx);
        self.sync_pending(cx);
    }
    /// `pending` grows with every read/tag toggle, but one store key is
    /// capped at `MAX_STORE_VALUE`; a big vault worked through in one
    /// sitting, without a sync in between, can outgrow it. A save the
    /// runtime refuses is not reported back here (see `on_store`), so
    /// letting that happen would leave every edit since the last successful
    /// save looking queued in memory while actually gone after a restart.
    /// Dropping the oldest edits first -- the ones a companion plugin
    /// polling every few minutes has had the most chances to already pick
    /// up -- keeps the write within budget instead.
    fn save_pending(&mut self, cx: &mut Context) {
        let mut dropped = false;
        while encode_pending(&self.pending).len() > kobo_sdk::MAX_STORE_VALUE
            && !self.pending.is_empty()
        {
            self.pending.remove(0);
            dropped = true;
        }
        if dropped {
            self.notice = Some(
                "Too many unsynced changes to keep them all on the device -- the oldest were dropped. Sync soon to avoid losing more.".to_owned(),
            );
        }
        cx.store().save(PENDING_KEY, encode_pending(&self.pending));
    }
    fn open_note(&mut self, cx: &mut Context, index: usize) {
        self.current = index;
        self.view = View::Note;
        self.body = None;
        self.note_rendered = None;
        self.note_paginated_through = 0;
        self.note_pages = vec![];
        self.note_page = 0;
        self.body_error = None;
        let id = self.notes[index].id.clone();
        // Pushed bodies are written to disk as `{id}.md` (see
        // `kobo-cli`'s `clippings::transfer`); the shelf name requested here
        // has to match that exactly or the read never finds the file.
        let mut load = ShelfDownload::new(format!("{id}.md")).at_most(MAX_BODY);
        load.start(cx);
        self.body_load = Some(load);
        self.body_load_id = Some(id);
    }
    /// Keeps the export offer for a paired computer to pull matched to
    /// `self.pending`, without the reader ever having to ask for it.
    ///
    /// Exports are owner-initiated and single-slot (see
    /// `kobo_sdk::exports`'s doc comment): nothing is exposed to `kobo
    /// export` until something publishes an offer, and a later publish
    /// simply replaces whatever offer was there before. Calling this
    /// wherever `self.pending` can change is what stands in for the reader
    /// tapping a "Prepare sync" button themselves -- a fresh offer, matching
    /// the latest edits, is always either being prepared or already sitting
    /// on the shelf.
    ///
    /// Only actually replaces `self.export` when the outstanding one (if
    /// any) has reached `is_ready()`. An edit made while the previous export
    /// is still being prepared just marks `export_dirty`; `on_save`/
    /// `on_shelf` publish the fresh content once that export settles. See
    /// `export_dirty`'s field comment for why replacing mid-flight is unsafe.
    fn sync_pending(&mut self, cx: &mut Context) {
        if let Some(export) = &self.export {
            if !export.is_ready() {
                // Reconciliation needed once this export settles even if
                // `pending` is now empty: a ready export sitting there for
                // edits that were since reverted is exactly as stale as one
                // that no longer matches non-empty `pending`, and both need
                // `publish_pending` to either replace it or clear it.
                self.export_dirty = true;
                return;
            }
        }
        self.publish_pending(cx);
    }
    fn publish_pending(&mut self, cx: &mut Context) {
        self.export_dirty = false;
        if self.pending.is_empty() {
            self.export = None;
            return;
        }
        match Export::new(
            "Clippings sync",
            Format::Text,
            encode_pending(&self.pending),
        ) {
            Ok(mut export) => {
                export.begin(cx);
                self.export = Some(export);
            }
            // Rare (an encode failure, not a transport one) and not worth a
            // banner over: the next edit calls this again and tries fresh.
            Err(_) => self.export = None,
        }
    }
    fn advance_manifest(&mut self, cx: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.manifest_load else {
            return false;
        };
        match load.advance(cx, result) {
            ShelfProgress::Done => {
                let bytes = self.manifest_load.take().expect("active manifest").take();
                match decode_manifest(&bytes) {
                    Ok(notes) => {
                        // A refresh (see `on_foreground`) replaces `notes`
                        // wholesale; `self.current` is an index into it, and
                        // a reordered or shortened list can leave that index
                        // pointing at a different note than the one on
                        // screen. Re-anchor on the open note's stable id
                        // instead of trusting the index to still mean the
                        // same thing.
                        let open_note_id = (self.view == View::Note)
                            .then(|| self.notes.get(self.current))
                            .flatten()
                            .map(|note| note.id.clone());
                        self.notes = notes;
                        self.notice = None;
                        if let Some(id) = open_note_id {
                            match self.notes.iter().position(|note| note.id == id) {
                                Some(index) => self.current = index,
                                // The open note is gone from the refreshed
                                // manifest; back out rather than show a
                                // stale note or silently jump to whatever
                                // now sits at the old index.
                                None => self.view = View::Home,
                            }
                        }
                        self.recompute_home_pages(cx);
                    }
                    Err(error) => {
                        self.notes.clear();
                        self.order.clear();
                        self.pages.clear();
                        self.notice = Some(format!("Clippings shelf needs re-pushing: {error}"));
                    }
                }
                self.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(kobo_sdk::StoreError::Missing) => {
                self.manifest_load = None;
                self.manifest_loaded = true;
                true
            }
            ShelfProgress::Failed(_) => {
                self.manifest_load = None;
                self.manifest_loaded = true;
                self.notice = Some("Clippings shelf could not be opened.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
    fn advance_body(&mut self, cx: &mut Context, result: &StoreResult) -> bool {
        let Some(load) = &mut self.body_load else {
            return false;
        };
        match load.advance(cx, result) {
            ShelfProgress::Done => {
                let bytes = self.body_load.take().expect("active body").take();
                let id = self.body_load_id.take().expect("active body identity");
                if self
                    .notes
                    .get(self.current)
                    .is_some_and(|note| note.id == id)
                {
                    if let Ok(text) = String::from_utf8(bytes) {
                        self.note_rendered = Some(md::render(&text, MAX_BODY));
                        self.note_paginated_through = 0;
                        self.note_pages = vec![];
                        self.note_page = 0;
                        self.body = Some(text);
                        self.extend_note_pages(cx);
                    } else {
                        self.note_pages = vec![];
                        self.note_page = 0;
                        self.body_error = Some("This note could not be read.".to_owned());
                    }
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.body_load = None;
                let id = self.body_load_id.take();
                if id.is_some_and(|id| {
                    self.notes
                        .get(self.current)
                        .is_some_and(|note| note.id == id)
                }) {
                    self.body_error = Some(
                        "This note could not be read. Re-push your vault and try again.".to_owned(),
                    );
                }
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
    fn show(&self, cx: &mut Context) {
        let screen = self.screen();
        cx.set_screen(screen);
    }
    /// The Home screen's top bar and heading, with no rows yet -- built once
    /// so both `home_pages` (to measure how much room is left for rows) and
    /// `screen` (to actually draw it) start from the same thing.
    fn home_header(&self) -> ScreenBuilder {
        let mut s = ScreenBuilder::new("clippings-home")
            .top_bar("Clippings")
            .owns_back(true)
            .top_bar_glyph("filter", "Filter", Glyph::Filter)
            .top_bar_glyph("sort", "Sort", Glyph::Clock);
        if let Some(notice) = &self.notice {
            s = s.banner(kobo_sdk::BannerLevel::Info, notice.clone());
        }
        let unread = self.notes.iter().filter(|n| !self.is_read(n)).count();
        let mut narrowed = self.read_filter.label().to_owned();
        if let Some(tag) = &self.tag_filter {
            narrowed = format!("{narrowed} · #{tag}");
        }
        s.heading(format!("{unread}/{} unread", self.notes.len()))
            .secondary(format!("{narrowed} · {}", self.sort.label()))
    }
    /// Recomputes `order` and `pages`, and resets to page 0.
    ///
    /// Call this whenever the notes set, the active filter, or the sort order
    /// changes (manifest load, filter cycle, sort toggle, marking a note read
    /// or tagging it, either of which can change whether it still matches
    /// the active filter), never from `screen()`. Pagination measures every
    /// row's text layout to find page breaks, which is cheap once but was a
    /// several-hundred-millisecond lifecycle-callback warning when redone on
    /// every single tap against the real ~3,500-note vault, since `screen()`
    /// runs after every action.
    ///
    /// Measured against `published` regardless of read state, not the "Read"
    /// label a row may show once toggled: keeping the measurement text stable
    /// means marking a note read never needs a repagination, and "Read" is
    /// never much longer than the date it replaces on screen.
    fn recompute_home_pages(&mut self, cx: &Context) {
        let candidates: Vec<usize> = (0..self.notes.len())
            .filter(|&i| self.matches_filter(&self.notes[i]))
            .collect();
        self.order = model::sort_indices(&self.notes, self.sort, candidates);
        self.pages.clear();
        self.paginated_through = 0;
        self.page = 0;
        self.extend_home_pages(cx);
    }
    /// Measures and paginates the next chunk of `order`, appending to
    /// `pages`. A no-op once every note has been measured.
    ///
    /// Bounded rather than doing all of `order` at once: this is the actual
    /// fix for the >1 second lifecycle-callback warning against the real
    /// vault (~3,500 notes) -- `HOME_PAGINATION_CHUNK` keeps any single call
    /// comfortably under the 250 ms budget even on the device's slower CPU,
    /// at the cost of very slightly less tight packing at each chunk seam
    /// (the last page of one chunk is never topped up from the next).
    fn extend_home_pages(&mut self, cx: &Context) {
        if self.paginated_through >= self.order.len() {
            return;
        }
        let end = (self.paginated_through + HOME_PAGINATION_CHUNK).min(self.order.len());
        let slice = &self.order[self.paginated_through..end];
        let pairs: Vec<(&str, &str)> = slice
            .iter()
            .map(|&i| {
                let n = &self.notes[i];
                (n.title.as_str(), n.published.as_str())
            })
            .collect();
        let pages = cx.paginate_rows_under(
            &pairs,
            true,
            kobo_sdk::Position::AtTheFoot,
            &self.home_header().build(),
        );
        let offset = self.paginated_through;
        self.pages.extend(
            pages
                .into_iter()
                .map(|page| page.into_iter().map(|position| position + offset).collect()),
        );
        self.paginated_through = end;
    }
    /// Extends `pages` once more if the reader is about to page past what has
    /// been measured so far, so `next` never has to wait on the boundary tap
    /// itself.
    fn ensure_pages_ahead(&mut self, cx: &Context) {
        if self.page + 2 >= self.pages.len() && self.paginated_through < self.order.len() {
            self.extend_home_pages(cx);
        }
    }
    /// Measures and paginates the next `NOTE_PAGINATION_CHUNK` bytes of the
    /// open note's rendered body, appending to `note_pages`. A no-op once
    /// the whole body has been measured (`note_rendered` is cleared then, to
    /// free it rather than hold a second copy of the note alongside
    /// `note_pages` for the rest of the time it stays open).
    fn extend_note_pages(&mut self, cx: &Context) {
        let Some(rendered) = &self.note_rendered else {
            return;
        };
        let mut end = (self.note_paginated_through + NOTE_PAGINATION_CHUNK).min(rendered.len());
        while end < rendered.len() && !rendered.is_char_boundary(end) {
            end += 1;
        }
        let pages = cx.paginate_reading(&rendered[self.note_paginated_through..end], false);
        self.note_pages.extend(pages);
        self.note_paginated_through = end;
        if end >= rendered.len() {
            self.note_rendered = None;
        }
    }
    /// Extends `note_pages` once more if the reader is about to turn past
    /// what has been measured so far, so `note-next` never has to wait on
    /// the boundary tap itself. Mirrors `ensure_pages_ahead`.
    fn ensure_note_pages_ahead(&mut self, cx: &Context) {
        if self.note_page + 2 >= self.note_pages.len() && self.note_rendered.is_some() {
            self.extend_note_pages(cx);
        }
    }
    #[allow(clippy::too_many_lines)]
    fn screen(&self) -> Screen {
        if self.entry.is_open() {
            return ScreenBuilder::new("clippings-add-tag")
                .top_bar("Add tag")
                .owns_back(true)
                .text_entry(&self.entry, "Tag name", "Add")
                .build();
        }
        if !self.loaded() {
            return ScreenBuilder::new("clippings-home")
                .top_bar("Clippings")
                .owns_back(true)
                .skeleton(4)
                .build();
        }
        match self.view {
            View::Home => {
                if self.notes.is_empty() {
                    return self
                        .home_header()
                        .splash(
                            Some(Glyph::Note),
                            "No clippings yet",
                            "Push your Web Clipper vault with the clippings-sync Obsidian plugin.",
                        )
                        .build();
                }
                let page = self.page.min(self.pages.len().saturating_sub(1));
                let visible = self.pages.get(page).map(Vec::as_slice).unwrap_or_default();
                self.home_header()
                    .rows(visible.iter().map(|&position| {
                        let note_index = self.order[position];
                        let n = &self.notes[note_index];
                        let read = self.is_read(n);
                        (
                            format!("note-{note_index}"),
                            n.title.clone(),
                            if read {
                                "Read".to_owned()
                            } else {
                                n.published.clone()
                            },
                            if read { Glyph::Check } else { Glyph::Note },
                        )
                    }))
                    .page_turns("previous", "next")
                    .page_position(
                        u16::try_from(page + 1).unwrap_or(u16::MAX),
                        u16::try_from(self.pages.len().max(1)).unwrap_or(u16::MAX),
                    )
                    .build()
            }
            View::Note => {
                let n = &self.notes[self.current];
                let read = self.is_read(n);
                let mut tags = n.tags.clone();
                tags.extend(self.added_tags(n));
                let mut s = ScreenBuilder::new("clippings-note")
                    .top_bar(n.title.clone())
                    .owns_back(true)
                    .top_bar_glyph(
                        "toggle-read",
                        if read { "Unread" } else { "Read" },
                        if read { Glyph::Check } else { Glyph::Circle },
                    )
                    .top_bar_glyph("add-tag", "Tag", Glyph::Tag)
                    .reading(true);
                let note_page = self.note_page.min(self.note_pages.len().saturating_sub(1));
                if let Some(error) = &self.body_error {
                    s = s.text(error.clone());
                } else if self.body.is_some() {
                    for line in self
                        .note_pages
                        .get(note_page)
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        s = s.text(line.clone());
                    }
                } else {
                    s = s.skeleton(6);
                }
                s.secondary(if tags.is_empty() {
                    "No tags".to_owned()
                } else {
                    tags.iter()
                        .map(|t| format!("#{t}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .page_turns("note-previous", "note-next")
                .page_position(
                    u16::try_from(note_page + 1).unwrap_or(u16::MAX),
                    u16::try_from(self.note_pages.len().max(1)).unwrap_or(u16::MAX),
                )
                .build()
            }
            View::Filter => {
                let tags = self.available_tags();
                let total_pages = tags.len().div_ceil(FILTER_TAGS_PER_PAGE).max(1);
                let page = self.filter_tag_page.min(total_pages - 1);
                let start = page * FILTER_TAGS_PER_PAGE;
                let visible = &tags[start..(start + FILTER_TAGS_PER_PAGE).min(tags.len())];
                let selected = match self.read_filter {
                    ReadFilter::Unread => 0,
                    ReadFilter::Read => 1,
                    ReadFilter::All => 2,
                };
                let mut s = ScreenBuilder::new("clippings-filter")
                    .top_bar("Filter")
                    .owns_back(true)
                    .choose(
                        "Read state",
                        [
                            ("read-unread", "Unread"),
                            ("read-read", "Read"),
                            ("read-all", "All"),
                        ],
                    )
                    .chosen(selected);
                if self.tag_filter.is_some() {
                    s = s.button("clear-tag", "Clear tag filter");
                }
                s.secondary(if tags.is_empty() {
                    "No tags yet".to_owned()
                } else {
                    "Tag".to_owned()
                })
                .rows(visible.iter().enumerate().map(|(i, tag)| {
                    let selected = self.tag_filter.as_deref() == Some(tag.as_str());
                    (
                        format!("tag-{}", start + i),
                        format!("#{tag}"),
                        String::new(),
                        if selected { Glyph::Check } else { Glyph::Tag },
                    )
                }))
                .page_turns("filter-tag-previous", "filter-tag-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(total_pages).unwrap_or(u16::MAX),
                )
                .build()
            }
        }
    }
}
impl KoboApp for Clippings {
    fn on_start(&mut self, cx: &mut Context) {
        cx.store().load(PENDING_KEY);
        self.refresh_manifest(cx);
        self.show(cx);
    }
    /// `kobo clippings push` writes straight to the shelf; a companion
    /// plugin's interval push while Clippings sits backgrounded produces no
    /// callback here to notice by itself, so the list on screen would
    /// otherwise still be whatever was loaded at process start until the
    /// reader restarts the app. Re-fetching on every return keeps it honest.
    fn on_foreground(&mut self, cx: &mut Context) {
        self.refresh_manifest(cx);
        self.show(cx);
    }
    fn on_store(&mut self, cx: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == PENDING_KEY {
                self.pending = value.map(|v| decode_pending(&v)).unwrap_or_default();
                self.pending_loaded = true;
                // Edits left over from a session that ended before a
                // computer ever pulled them still need an offer waiting.
                self.sync_pending(cx);
            }
        }
        self.show(cx);
    }
    fn on_save(&mut self, cx: &mut Context, key: &str, result: StoreResult) {
        if key == PENDING_KEY {
            // save_pending already bounds the write to fit, so a refusal
            // here means the store itself rejected it for some other
            // reason; say so rather than let the edits look queued when
            // they were not actually kept.
            if matches!(result, StoreResult::Denied(_)) {
                self.notice = Some(
                    "Your latest change could not be saved on the device. Try it again.".to_owned(),
                );
            }
            self.show(cx);
            return;
        }
        if let Some(export) = self.export.as_mut() {
            if export.on_save(cx, key, &result) {
                if export.is_ready() && self.export_dirty {
                    self.publish_pending(cx);
                }
                self.show(cx);
                return;
            }
        }
        self.show(cx);
    }
    fn on_shelf(&mut self, cx: &mut Context, name: &str, result: StoreResult) {
        if let Some(export) = self.export.as_mut() {
            if export.on_shelf(cx, name, &result) {
                self.show(cx);
                return;
            }
        }
        if self.advance_manifest(cx, &result) || self.advance_body(cx, &result) {
            self.show(cx);
            return;
        }
        self.show(cx);
    }
    fn on_action(&mut self, cx: &mut Context, a: ActionId) {
        // The back chevron, not just Cancel: while the entry is open it is
        // the only screen shown (see `screen`'s early return), so a BACK that
        // only changed `self.view` internally would redraw the exact same
        // screen. The runtime treats an unanswered back offer as the app
        // never having responded and leaves to the launcher after its grace
        // period -- closing the entry here instead means a visibly different
        // screen (the note) actually goes out.
        if a == ActionId::BACK && self.entry.is_open() {
            self.entry.close();
            self.show(cx);
            return;
        }
        if let Some(event) = self.entry.handle(a) {
            if let Typing::Submitted(tag) = event {
                self.add_tag(cx, &tag);
            }
            self.show(cx);
            return;
        }
        self.notice = None;
        if self.view == View::Filter {
            if a == action_id("read-unread") {
                self.read_filter = ReadFilter::Unread;
                self.recompute_home_pages(cx);
            } else if a == action_id("read-read") {
                self.read_filter = ReadFilter::Read;
                self.recompute_home_pages(cx);
            } else if a == action_id("read-all") {
                self.read_filter = ReadFilter::All;
                self.recompute_home_pages(cx);
            } else if a == action_id("clear-tag") {
                self.tag_filter = None;
                self.recompute_home_pages(cx);
            } else if a == action_id("filter-tag-previous") {
                self.filter_tag_page = self.filter_tag_page.saturating_sub(1);
            } else if a == action_id("filter-tag-next") {
                let total_pages = self
                    .available_tags()
                    .len()
                    .div_ceil(FILTER_TAGS_PER_PAGE)
                    .max(1);
                self.filter_tag_page = (self.filter_tag_page + 1).min(total_pages - 1);
            } else if a == ActionId::BACK {
                self.view = View::Home;
            } else {
                let tags = self.available_tags();
                for (i, tag) in tags.iter().enumerate() {
                    if a == action_id(&format!("tag-{i}")) {
                        self.tag_filter = if self.tag_filter.as_deref() == Some(tag.as_str()) {
                            None
                        } else {
                            Some(tag.clone())
                        };
                        self.recompute_home_pages(cx);
                        break;
                    }
                }
            }
            self.show(cx);
            return;
        }
        if a == action_id("filter") {
            self.view = View::Filter;
            self.filter_tag_page = 0;
        } else if a == action_id("sort") {
            self.sort = self.sort.next();
            self.recompute_home_pages(cx);
        } else if a == action_id("previous") {
            self.page = self.page.saturating_sub(1);
        } else if a == action_id("next") {
            self.ensure_pages_ahead(cx);
            self.page = (self.page + 1).min(self.pages.len().saturating_sub(1));
        } else if a == action_id("toggle-read") {
            self.toggle_read(cx);
        } else if a == action_id("add-tag") {
            self.entry.open();
        } else if a == action_id("note-previous") {
            self.note_page = self.note_page.saturating_sub(1);
        } else if a == action_id("note-next") {
            self.ensure_note_pages_ahead(cx);
            self.note_page = (self.note_page + 1).min(self.note_pages.len().saturating_sub(1));
        } else if a == ActionId::BACK && self.view == View::Note {
            self.view = View::Home;
        } else if a == ActionId::BACK && self.view == View::Home {
            // No-op: the table of contents is the ceiling of Back's own
            // in-app navigation. Leaving Clippings entirely is the runtime's
            // call, not this screen's -- see `Context::exit`'s docs.
        } else {
            for i in 0..self.notes.len() {
                if a == action_id(&format!("note-{i}")) {
                    self.open_note(cx, i);
                }
            }
        }
        self.show(cx);
    }
}
fn main() -> ExitCode {
    match kobo_sdk::run("clippings", Clippings::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("clippings: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command, StoreRequest};

    /// Feeds a generic success back to every store command an `Export`'s
    /// `Import` issues (check-existing miss, upload, re-verify, receipt
    /// save, then the offer save itself), driving it all the way to
    /// `is_ready()` the same way the runtime eventually would. Mirrors
    /// `kobo_sdk::exports`'s own test, adapted to go through `AppRunner`
    /// instead of a bare `Context` + fake shelf/store.
    ///
    /// A shelf name's bytes are recorded as each `ShelfWrite` lands, so the
    /// re-verify `ShelfRead` for that same name can hand back what was
    /// actually written (its digest has to match for `Import` to accept
    /// it); a name with nothing recorded yet is the initial check-existing
    /// read, which is genuinely missing.
    fn settle_export(runner: &mut AppRunner<Clippings>, commands: Vec<Command>) {
        let mut pending = commands;
        let mut written: std::collections::HashMap<String, Vec<u8>> =
            std::collections::HashMap::new();
        for _ in 0..64 {
            let Some(request) = pending.iter().find_map(|command| match command {
                Command::Store(request) => Some(request.clone()),
                _ => None,
            }) else {
                return;
            };
            pending.retain(|command| !matches!(command, Command::Store(r) if r == &request));
            let result = match &request {
                StoreRequest::Save { key, .. } => StoreResult::Saved { key: key.clone() },
                StoreRequest::ShelfWrite {
                    name,
                    offset,
                    bytes,
                    ..
                } => {
                    let entry = written.entry(name.clone()).or_default();
                    let start = usize::try_from(*offset).unwrap_or(0);
                    if entry.len() < start {
                        entry.resize(start, 0);
                    }
                    entry.truncate(start);
                    entry.extend_from_slice(bytes);
                    StoreResult::ShelfWritten {
                        name: name.clone(),
                        size: u32::try_from(entry.len()).unwrap_or(u32::MAX),
                    }
                }
                StoreRequest::ShelfRead { name, .. } => match written.get(name) {
                    Some(bytes) => StoreResult::ShelfRead {
                        name: name.clone(),
                        offset: 0,
                        size: u32::try_from(bytes.len()).unwrap_or(0),
                        bytes: bytes.clone(),
                    },
                    None => StoreResult::Denied(kobo_sdk::StoreError::Missing),
                },
                _ => panic!("unexpected export command {request:?}"),
            };
            pending.extend(runner.store_result(result));
        }
        panic!("export did not settle within 64 store round-trips");
    }

    #[test]
    fn timing_probe_for_sync_pending_with_a_realistic_pending_list() {
        let mut app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            pending: (0..50)
                .map(|i| PendingEdit {
                    path: format!("Clippings/{i}.md"),
                    read: true,
                    added_tags: vec!["read-on-kobo".to_owned()],
                })
                .collect(),
            ..Clippings::default()
        };
        let mut context = kobo_sdk::AppRunner::new(Clippings::default()).context();
        let t0 = std::time::Instant::now();
        app.sync_pending(&mut context);
        let elapsed = t0.elapsed();
        println!("sync_pending with 50 pending edits: {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "sync_pending took {elapsed:?} on host -- should be trivial, not a real contributor \
             to a lifecycle-callback overrun"
        );
    }

    #[test]
    #[ignore = "diagnostic timing probe against a real on-device manifest dump, not CI"]
    fn timing_probe_against_the_real_manifest() {
        let bytes = std::fs::read("/tmp/real-manifest-v1.json").expect("real manifest fixture");
        let text = std::str::from_utf8(&bytes).expect("utf8");
        let t_json = std::time::Instant::now();
        let value = kobo_json::parse(text).expect("valid json");
        let json_tokenize_time = t_json.elapsed();
        drop(value);
        let t0 = std::time::Instant::now();
        let notes = decode_manifest(&bytes).expect("real manifest parses");
        let parse_time = t0.elapsed();
        println!("json_tokenize_only={json_tokenize_time:?} full_decode={parse_time:?}");
        let context = kobo_sdk::AppRunner::with_metrics(
            Clippings::default(),
            kobo_sdk::DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                text_scale: kobo_ui::TextScale::Default,
            },
        )
        .context();
        let mut app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            notes,
            ..Clippings::default()
        };
        let t1 = std::time::Instant::now();
        app.recompute_home_pages(&context);
        let first_chunk_time = t1.elapsed();
        let t2 = std::time::Instant::now();
        while app.paginated_through < app.order.len() {
            app.extend_home_pages(&context);
        }
        let remaining_chunks_time = t2.elapsed();
        println!(
            "notes={} parse={parse_time:?} first_chunk(<={HOME_PAGINATION_CHUNK})={first_chunk_time:?} \
             rest_of_vault={remaining_chunks_time:?} pages={}",
            app.notes.len(),
            app.pages.len()
        );
    }

    #[test]
    #[ignore = "diagnostic timing probe against a real oversized clipping, not CI"]
    fn timing_probe_against_a_real_large_note() {
        let path = std::env::var("CLIPPING_PATH").expect("set CLIPPING_PATH to a real .md file");
        let raw = std::fs::read_to_string(&path).expect("read the real clipping");
        let metrics = kobo_sdk::DisplayMetrics {
            width: 1264,
            height: 1680,
            pixels_per_inch: 300,
            text_scale: kobo_ui::TextScale::Default,
        };
        let t0 = std::time::Instant::now();
        let rendered = md::render(&raw, MAX_BODY);
        let render_time = t0.elapsed();
        let context = kobo_sdk::AppRunner::with_metrics(Clippings::default(), metrics).context();
        let t1 = std::time::Instant::now();
        let note_pages = context.paginate_reading(&rendered, false);
        let paginate_time = t1.elapsed();
        let app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            notes: vec![NoteMeta {
                id: "note-0000000000000001".to_owned(),
                path: "A.md".to_owned(),
                title: "Alpha".to_owned(),
                read: false,
                tags: vec![],
                published: String::new(),
                created: String::new(),
            }],
            body: Some(rendered.clone()),
            note_pages: note_pages.clone(),
            view: View::Note,
            ..Clippings::default()
        };
        let screen = app.screen();
        let chrome = kobo_ui::Chrome::for_screen(&screen, false, None);
        let t2 = std::time::Instant::now();
        let issues = screen.diagnostics(&metrics, &chrome).issues;
        let layout_time = t2.elapsed();
        println!(
            "path={path} raw_bytes={} rendered_chars={} pages={} render={render_time:?} \
             paginate={paginate_time:?} layout(page 1)={layout_time:?} issues={}",
            raw.len(),
            rendered.len(),
            note_pages.len(),
            issues.len()
        );
    }

    fn screen_text(screen: &Screen, needle: &str) -> bool {
        screen.nodes.iter().any(|node| match node {
            kobo_sdk::Node::Heading { text, .. }
            | kobo_sdk::Node::Text { text, .. }
            | kobo_sdk::Node::Secondary { text, .. }
            | kobo_sdk::Node::RichText { text, .. } => text.contains(needle),
            kobo_sdk::Node::Rows { rows, .. } => rows
                .iter()
                .any(|row| row.title.contains(needle) || row.summary.contains(needle)),
            kobo_sdk::Node::Button { label, .. } => label.contains(needle),
            _ => false,
        })
    }

    fn manifest_bytes() -> Vec<u8> {
        br#"{"version":1,"notes":[
            {"id":"note-0000000000000001","path":"A.md","title":"Alpha","published":"2026-01-01","tags":["one"],"read":false},
            {"id":"note-0000000000000002","path":"B.md","title":"Beta","published":"2026-02-01","read":false}
        ]}"#.to_vec()
    }

    fn seeded() -> AppRunner<Clippings> {
        let mut runner = AppRunner::new(Clippings::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: PENDING_KEY.into(),
            value: None,
        });
        runner.store_result(StoreResult::ShelfRead {
            name: MANIFEST.into(),
            offset: 0,
            bytes: manifest_bytes(),
            size: u32::try_from(manifest_bytes().len()).unwrap(),
        });
        runner
    }

    #[test]
    fn action_graph_reaches_every_view() {
        let mut runner = seeded();
        assert_eq!(runner.app().view, View::Home);
        assert_eq!(runner.app().notes.len(), 2);
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().view, View::Note);
        assert!(runner.app().body_load.is_some());
        let body = b"Alpha body.\n".to_vec();
        runner.store_result(StoreResult::ShelfRead {
            name: "note-0000000000000001.md".into(),
            offset: 0,
            bytes: body.clone(),
            size: u32::try_from(body.len()).unwrap(),
        });
        assert_eq!(runner.app().body.as_deref(), Some("Alpha body.\n"));
        runner.action(action_id("add-tag"));
        assert!(runner.app().entry.is_open());
    }

    #[test]
    fn back_out_of_the_tag_entry_closes_it_instead_of_leaving_the_app() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        runner.action(action_id("add-tag"));
        assert!(runner.app().entry.is_open());
        let commands = runner.action(ActionId::BACK);
        assert!(!runner.app().entry.is_open());
        assert_eq!(runner.app().view, View::Note);
        assert!(!commands.contains(&kobo_sdk::Command::Exit), "{commands:?}");
    }

    #[test]
    fn marking_read_toggles_and_reverts_cleanly() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        assert!(!runner.app().is_read(&runner.app().notes[0].clone()));
        runner.action(action_id("toggle-read"));
        assert!(runner
            .app()
            .pending
            .iter()
            .any(|e| e.path == "A.md" && e.read));
        runner.action(action_id("toggle-read"));
        assert!(
            runner.app().pending.is_empty(),
            "reverting to the original state clears the pending edit"
        );
    }

    #[test]
    fn sort_toggles_order() {
        let mut runner = seeded();
        assert_eq!(runner.app().sort, Sort::Published);
        runner.action(action_id("sort"));
        assert_eq!(runner.app().sort, Sort::Created);
    }

    #[test]
    fn unread_is_the_default_filter_and_a_read_note_starts_out_of_home() {
        let runner = seeded();
        assert_eq!(runner.app().read_filter, ReadFilter::Unread);
        assert_eq!(runner.app().tag_filter, None);
        assert_eq!(
            runner.app().order,
            vec![1, 0],
            "both seeded notes are unread"
        );
    }

    #[test]
    fn marking_a_note_read_drops_it_out_of_the_unread_filter() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        runner.action(action_id("toggle-read"));
        assert_eq!(
            runner.app().order,
            vec![1],
            "the just-read note leaves Home's shelf under the default Unread filter"
        );
        // And marking it unread again brings it right back.
        runner.action(action_id("toggle-read"));
        assert_eq!(runner.app().order, vec![1, 0]);
    }

    #[test]
    fn the_filter_action_opens_a_dedicated_screen_rather_than_cycling_in_place() {
        let mut runner = seeded();
        assert_eq!(runner.app().view, View::Home);
        runner.action(action_id("filter"));
        assert_eq!(runner.app().view, View::Filter);
        // Unchanged: opening the picker is not itself a choice.
        assert_eq!(runner.app().read_filter, ReadFilter::Unread);
    }

    #[test]
    fn read_state_and_tag_filter_are_independent_and_both_apply() {
        let mut runner = seeded();
        runner.action(action_id("filter"));
        runner.action(action_id("read-all"));
        assert_eq!(runner.app().read_filter, ReadFilter::All);
        assert_eq!(runner.app().tag_filter, None);
        // Only "Alpha" (index 0) carries the tag "one" in the seeded manifest.
        runner.action(action_id("tag-0"));
        assert_eq!(runner.app().tag_filter, Some("one".to_owned()));
        assert_eq!(
            runner.app().order,
            vec![0],
            "All plus tag \"one\" is still just Alpha, not reset back to Unread"
        );
        // Tapping the same tag row again clears it.
        runner.action(action_id("tag-0"));
        assert_eq!(runner.app().tag_filter, None);
        // Back leaves the picker without disturbing either choice.
        runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
        assert_eq!(runner.app().read_filter, ReadFilter::All);
    }

    #[test]
    fn the_tag_list_paginates_instead_of_showing_every_tag_at_once() {
        let manifest: String = (0..25)
            .map(|i| {
                format!(
                    r#"{{"id":"note-{i:016x}","path":"{i}.md","title":"Note {i}","tags":["tag-{i:02}"],"read":false}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut runner = AppRunner::new(Clippings::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: PENDING_KEY.into(),
            value: None,
        });
        let bytes = format!(r#"{{"version":1,"notes":[{manifest}]}}"#).into_bytes();
        runner.store_result(StoreResult::ShelfRead {
            name: MANIFEST.into(),
            offset: 0,
            bytes: bytes.clone(),
            size: u32::try_from(bytes.len()).unwrap(),
        });
        assert_eq!(runner.app().available_tags().len(), 25);
        runner.action(action_id("filter"));
        assert_eq!(runner.app().filter_tag_page, 0);
        runner.action(action_id("filter-tag-next"));
        assert_eq!(runner.app().filter_tag_page, 1);
        runner.action(action_id("filter-tag-next"));
        assert_eq!(
            runner.app().filter_tag_page,
            2,
            "25 tags at 10 per page makes 3 pages"
        );
        // Bounded: one more "next" past the last page stays put.
        runner.action(action_id("filter-tag-next"));
        assert_eq!(runner.app().filter_tag_page, 2);
        runner.action(action_id("filter-tag-previous"));
        assert_eq!(runner.app().filter_tag_page, 1);
    }

    #[test]
    fn a_newly_added_tag_is_reachable_by_the_tag_filter_immediately() {
        let mut runner = seeded();
        runner.action(action_id("note-1"));
        let mut context = runner.context();
        runner.app_mut().add_tag(&mut context, "on");
        runner.app_mut().tag_filter = Some("on".to_owned());
        runner.app_mut().recompute_home_pages(&context);
        assert_eq!(
            runner.app().order,
            vec![1],
            "Beta (index 1) just gained the tag \"on\" and should match its filter"
        );
    }

    #[test]
    fn back_at_home_stays_on_the_table_of_contents() {
        let mut runner = seeded();
        assert_eq!(runner.app().view, View::Home);
        let commands = runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
        assert!(!commands.contains(&kobo_sdk::Command::Exit), "{commands:?}");
    }

    #[test]
    fn back_from_a_note_returns_to_home_without_exiting() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().view, View::Note);
        let commands = runner.action(ActionId::BACK);
        assert_eq!(runner.app().view, View::Home);
        assert!(!commands.contains(&kobo_sdk::Command::Exit), "{commands:?}");
    }

    #[test]
    fn note_page_turns_move_through_a_multi_page_note_and_reset_on_reopen() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().view, View::Note);
        // Simulates a body long enough to need more than one page -- the
        // real path through `advance_body` calls `paginate_reading` for
        // this, which needs real display metrics a unit test does not have.
        runner.app_mut().note_pages =
            vec![vec!["page one".to_owned()], vec!["page two".to_owned()]];
        runner.app_mut().note_page = 0;
        runner.action(action_id("note-next"));
        assert_eq!(runner.app().note_page, 1);
        // Bounded: one more "next" past the last page stays put.
        runner.action(action_id("note-next"));
        assert_eq!(runner.app().note_page, 1);
        runner.action(action_id("note-previous"));
        assert_eq!(runner.app().note_page, 0);
        // Bounded on the other side too.
        runner.action(action_id("note-previous"));
        assert_eq!(runner.app().note_page, 0);
        // Reopening (even the same note) starts back at the first page.
        runner.app_mut().note_page = 1;
        runner.action(action_id("note-0"));
        assert_eq!(runner.app().note_page, 0);
    }

    #[test]
    fn no_pending_edits_means_no_export_offer() {
        let runner = seeded();
        assert!(runner.app().export.is_none());
    }

    #[test]
    fn marking_a_note_read_publishes_an_export_offer_without_being_asked() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        // Opening the note also starts its own body fetch; settle that
        // (unrelated to this test) before it sits ahead of the export's own
        // requests in the runner's reply queue.
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        let commands = runner.action(action_id("toggle-read"));
        assert!(
            runner.app().export.is_some(),
            "an edit should publish an offer on its own, with no \"Prepare sync\" tap"
        );
        assert_eq!(runner.app().view, View::Note, "publishing stays invisible");
        // Settle the outstanding export before reverting, matching the real
        // runtime: `sync_pending` must not replace or drop `export` while
        // its own callbacks are still in flight (see `export_dirty`), so
        // reverting while it is still being prepared only clears it once
        // this finishes, not immediately.
        settle_export(&mut runner, commands);
        assert!(runner.app().export.as_ref().is_some_and(Export::is_ready));
        // Reverting the same edit empties `pending` again, and the offer
        // goes with it -- nothing left worth a paired computer pulling.
        runner.action(action_id("toggle-read"));
        assert!(runner.app().export.is_none());
    }

    #[test]
    fn tagging_an_already_read_note_does_not_mark_it_unread() {
        let mut runner = seeded();
        runner.app_mut().notes[0].read = true;
        let mut context = runner.context();
        runner.app_mut().add_tag(&mut context, "recipe");
        let edit = runner
            .app()
            .pending
            .iter()
            .find(|edit| edit.path == "A.md")
            .expect("adding a tag creates a pending edit");
        assert!(
            edit.read,
            "a tag-only edit on an already-read note must not flip it to unread"
        );
    }

    #[test]
    fn save_pending_drops_the_oldest_edits_once_the_store_value_cap_is_reached() {
        let mut runner = seeded();
        // Each path is unique and long enough that a few thousand of them
        // clears MAX_STORE_VALUE (256 KiB) well before running out of
        // patience, without needing a real multi-thousand-note vault.
        runner.app_mut().pending = (0..5000)
            .map(|i| PendingEdit {
                path: format!("Clippings/2026/a-fairly-long-article-title-{i:04}.md"),
                read: true,
                added_tags: vec![],
            })
            .collect();
        let mut context = runner.context();
        runner.app_mut().save_pending(&mut context);
        let app = runner.app();
        assert!(
            encode_pending(&app.pending).len() <= kobo_sdk::MAX_STORE_VALUE,
            "save_pending must bound the write to the store's own cap"
        );
        assert!(
            app.pending.len() < 5000,
            "oldest edits should have been dropped to fit"
        );
        assert_eq!(
            app.pending.last().unwrap().path,
            "Clippings/2026/a-fairly-long-article-title-4999.md",
            "the newest edit should survive; dropping starts from the front"
        );
        assert!(
            app.notice.as_deref().is_some_and(|n| n.contains("dropped")),
            "the reader should be told edits were dropped, not left to find out at restart"
        );
    }

    #[test]
    fn a_failed_body_load_shows_an_error_instead_of_the_loading_skeleton_forever() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        assert!(
            runner.app().body_error.is_none(),
            "no error before the load answers"
        );
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        assert!(
            runner.app().body_error.is_some(),
            "a failed load must leave a visible reason, not just clear body_load"
        );
        assert!(runner.app().body.is_none());
        assert!(
            screen_text(&runner.app().screen(), "could not be read"),
            "the note view itself should say so"
        );
    }

    #[test]
    fn extend_note_pages_only_measures_one_chunk_at_a_time() {
        let mut runner = seeded();
        runner.action(action_id("note-0"));
        let paragraph = "The quick brown fox jumps over the lazy dog near the riverbank.\n\n";
        let mut body = String::new();
        while body.len() < NOTE_PAGINATION_CHUNK * 3 {
            body.push_str(paragraph);
        }
        runner.store_result(StoreResult::ShelfRead {
            name: "note-0000000000000001.md".into(),
            offset: 0,
            size: u32::try_from(body.len()).unwrap(),
            bytes: body.clone().into_bytes(),
        });
        assert!(
            runner.app().note_pages.len() < body.len() / 40,
            "the first callback should only have measured one chunk, not the whole body"
        );
        assert!(
            runner.app().note_rendered.is_some(),
            "there is more of the body left to paginate"
        );
        let first_chunk_pages = runner.app().note_pages.len();
        // Turning forward past what has been measured extends it just in
        // time, the same as `ensure_pages_ahead` does for Home.
        while runner.app().note_page + 2 < runner.app().note_pages.len() {
            runner.action(action_id("note-next"));
        }
        runner.action(action_id("note-next"));
        assert!(
            runner.app().note_pages.len() > first_chunk_pages,
            "turning near the measured edge should have extended note_pages"
        );
    }

    #[test]
    fn on_foreground_refetches_the_manifest_and_keeps_the_open_note_anchored_by_id() {
        let mut runner = seeded();
        runner.action(action_id("note-1"));
        assert_eq!(
            runner.app().notes[runner.app().current].id,
            "note-0000000000000002"
        );
        runner.resume();
        // A push while backgrounded reordered the manifest (Beta now comes
        // first) and dropped nothing -- `current` must follow Beta's id, not
        // stay pinned to index 1, which is Alpha's new position.
        let reordered = br#"{"version":1,"notes":[
            {"id":"note-0000000000000002","path":"B.md","title":"Beta","published":"2026-02-01","read":false},
            {"id":"note-0000000000000001","path":"A.md","title":"Alpha","published":"2026-01-01","tags":["one"],"read":false}
        ]}"#;
        runner.store_result(StoreResult::ShelfRead {
            name: MANIFEST.into(),
            offset: 0,
            bytes: reordered.to_vec(),
            size: u32::try_from(reordered.len()).unwrap(),
        });
        assert_eq!(runner.app().view, View::Note, "the open note stays open");
        assert_eq!(
            runner.app().notes[runner.app().current].id,
            "note-0000000000000002",
            "current must follow Beta's id to its new index, not stay at the old one"
        );
    }

    #[test]
    fn a_missing_manifest_is_treated_as_an_empty_shelf_not_an_error() {
        let mut runner = AppRunner::new(Clippings::default());
        runner.start();
        runner.store_result(StoreResult::Loaded {
            key: PENDING_KEY.into(),
            value: None,
        });
        runner.store_result(StoreResult::Denied(kobo_sdk::StoreError::Missing));
        assert!(runner.app().notes.is_empty());
        assert!(runner.app().manifest_loaded);
        assert!(runner.app().notice.is_none());
    }

    #[test]
    fn home_and_note_screens_fit_at_every_supported_size_and_text_scale() {
        use kobo_ui::Chrome;
        let mut app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            notes: vec![
                NoteMeta {
                    id: "note-0000000000000001".to_owned(),
                    path: "A.md".to_owned(),
                    title: "Alpha".to_owned(),
                    read: false,
                    tags: vec!["one".to_owned()],
                    published: "2026-01-01".to_owned(),
                    created: String::new(),
                },
                NoteMeta {
                    id: "note-0000000000000002".to_owned(),
                    path: "B.md".to_owned(),
                    title: "Beta".to_owned(),
                    read: true,
                    tags: vec![],
                    published: String::new(),
                    created: String::new(),
                },
            ],
            body: Some("Alpha body.".to_owned()),
            ..Clippings::default()
        };
        for (width, height, pixels_per_inch) in
            [(1072, 1448, 300), (1448, 1072, 300), (758, 1024, 212)]
        {
            for text_scale in kobo_ui::TextScale::STEPS {
                let metrics = kobo_sdk::DisplayMetrics {
                    width,
                    height,
                    pixels_per_inch,
                    text_scale,
                };
                let context =
                    kobo_sdk::AppRunner::with_metrics(Clippings::default(), metrics).context();
                app.recompute_home_pages(&context);
                app.note_pages =
                    context.paginate_reading(&md::render("Alpha body.", MAX_BODY), false);
                app.note_page = 0;
                for view in [View::Home, View::Note, View::Filter] {
                    app.view = view;
                    let screen = app.screen();
                    let chrome = Chrome::for_screen(&screen, false, None);
                    let issues = screen
                        .diagnostics(&metrics, &chrome)
                        .issues
                        .into_iter()
                        .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                        .collect::<Vec<_>>();
                    assert!(issues.is_empty(), "{metrics:?} {view:?}: {issues:?}");
                }
            }
        }
    }

    /// The real vault has ~3,500 clippings. A trailing button placed after
    /// that many rows would sit wherever the row list happened to end,
    /// reachable only by scrolling past all of them first -- this is what
    /// caught it (`top_bar_action` fixed it; see the comment in `screen()`).
    #[test]
    fn the_home_screen_keeps_its_top_bar_actions_reachable_with_thousands_of_notes() {
        use kobo_ui::Chrome;
        let notes: Vec<NoteMeta> = (0..3549)
            .map(|i| NoteMeta {
                id: format!("note-{i:016x}"),
                path: format!("Clippings/{i}.md"),
                title: format!("Clipping number {i}"),
                read: i % 3 == 0,
                tags: vec![],
                published: format!("2026-{:02}-01", (i % 12) + 1),
                created: String::new(),
            })
            .collect();
        let mut app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            notes,
            view: View::Home,
            ..Clippings::default()
        };
        let metrics = kobo_sdk::DisplayMetrics {
            width: 1264,
            height: 1680,
            pixels_per_inch: 300,
            text_scale: kobo_ui::TextScale::Default,
        };
        let context = kobo_sdk::AppRunner::with_metrics(Clippings::default(), metrics).context();
        app.recompute_home_pages(&context);
        while app.paginated_through < app.order.len() {
            app.extend_home_pages(&context);
        }
        assert!(app.pages.len() > 1, "3,549 notes must not fit one page");
        for page in 0..app.pages.len() {
            app.page = page;
            let screen = app.screen();
            let chrome = Chrome::for_screen(&screen, false, None);
            let issues = screen
                .diagnostics(&metrics, &chrome)
                .issues
                .into_iter()
                .filter(|issue| issue.severity == kobo_ui::DiagnosticSeverity::Error)
                .collect::<Vec<_>>();
            assert!(issues.is_empty(), "page {page}: {issues:?}");
        }
    }

    /// The actual fix for the lifecycle-callback warning: loading the real
    /// vault must not measure all ~3,500 notes' text layout in one call.
    #[test]
    fn recompute_home_pages_only_measures_one_chunk_at_a_time() {
        let notes: Vec<NoteMeta> = (0..3549)
            .map(|i| NoteMeta {
                id: format!("note-{i:016x}"),
                path: format!("Clippings/{i}.md"),
                title: format!("Clipping number {i}"),
                read: false,
                tags: vec![],
                published: format!("2026-{:02}-01", (i % 12) + 1),
                created: String::new(),
            })
            .collect();
        let mut app = Clippings {
            manifest_loaded: true,
            pending_loaded: true,
            notes,
            ..Clippings::default()
        };
        let context = kobo_sdk::AppRunner::with_metrics(
            Clippings::default(),
            kobo_sdk::DisplayMetrics {
                width: 1264,
                height: 1680,
                pixels_per_inch: 300,
                text_scale: kobo_ui::TextScale::Default,
            },
        )
        .context();
        app.recompute_home_pages(&context);
        assert_eq!(app.paginated_through, HOME_PAGINATION_CHUNK);
        assert!(app.paginated_through < app.order.len());
        let pages_after_first_chunk = app.pages.len();
        app.extend_home_pages(&context);
        assert_eq!(app.paginated_through, 2 * HOME_PAGINATION_CHUNK);
        assert!(app.pages.len() > pages_after_first_chunk);
        while app.paginated_through < app.order.len() {
            app.extend_home_pages(&context);
        }
        assert_eq!(app.paginated_through, app.order.len());
        // Idempotent past the end.
        let final_pages = app.pages.len();
        app.extend_home_pages(&context);
        assert_eq!(app.pages.len(), final_pages);
    }
}
