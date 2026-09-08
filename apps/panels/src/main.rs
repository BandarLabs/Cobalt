//! A local and self-hosted comics shelf with resumable downloads.

mod archive;
mod komga;
mod transfer;

use kobo_bookview::comic::{ComicView, Outcome as ComicOutcome, SaveState};
use kobo_opds::{Feed, ImageSource, Publication};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, PictureHandle,
    Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId,
    TaskOutcome, TilePicture,
};
use kobo_state::draft::{Draft, Status as DraftStatus};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::process::ExitCode;

const SIDELOAD: &str = "volume.cbz";
const LIBRARY: &str = "library";
const PARTIAL_META: &str = "partial";
const PARTIAL_BLOB: &str = "partial.cbz";
const COVER: PictureHandle = PictureHandle(2);
const MAX_IMAGE: u32 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Route {
    #[default]
    Library,
    Catalog,
    Search,
    Detail,
    Download,
    Reader,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Awaiting {
    Catalog,
    Cover,
    Comic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Saving {
    Partial,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Kept {
    key: String,
    title: String,
    pages: usize,
    rtl: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Pending {
    key: String,
    title: String,
    url: String,
}

struct Panels {
    route: Route,
    view: Option<ComicView>,
    pending_memory: Option<Vec<u8>>,
    opened: Option<Kept>,
    rtl: bool,
    cover: Option<TilePicture>,
    notice: Option<String>,
    task: Option<(TaskId, Awaiting)>,
    catalog: Option<Feed>,
    catalog_url: String,
    history: Vec<(String, Feed)>,
    selected: Option<Publication>,
    query: String,
    keyboard: Keyboard,
    library: Vec<Kept>,
    loaded: bool,
    transfer: Option<transfer::Download>,
    pending: Option<Pending>,
    upload: Option<(ShelfUpload, Saving)>,
    shelf_load: Option<ShelfDownload>,
    partial_load: Option<ShelfDownload>,
    pending_open: Option<Kept>,
    paused: bool,
    progress: BTreeMap<String, Draft>,
    progress_active: Option<(String, u64)>,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            route: Route::Library,
            view: None,
            pending_memory: None,
            opened: None,
            rtl: false,
            cover: None,
            notice: None,
            task: None,
            catalog: None,
            catalog_url: String::new(),
            history: Vec::new(),
            selected: None,
            query: String::new(),
            keyboard: Keyboard::new(),
            library: Vec::new(),
            loaded: false,
            transfer: None,
            pending: None,
            upload: None,
            shelf_load: None,
            partial_load: None,
            pending_open: None,
            paused: false,
            progress: BTreeMap::new(),
            progress_active: None,
        }
    }
}

impl Panels {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen().with_own_back(self.route != Route::Library));
    }

    fn screen(&self) -> Screen {
        match self.route {
            Route::Library => self.library_screen(),
            Route::Catalog => self.catalog_screen(),
            Route::Search => self.search_screen(),
            Route::Detail => self.detail_screen(),
            Route::Download => self.download_screen(),
            Route::Reader => self.reader_screen(),
        }
    }

    fn with_notice(&self, mut screen: ScreenBuilder) -> ScreenBuilder {
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
    }

    fn library_screen(&self) -> Screen {
        let mut screen = self.with_notice(
            ScreenBuilder::new("panels-library")
                .top_bar("Panels")
                .top_bar_action("browse-komga", "Browse"),
        );
        if !self.loaded {
            screen = screen.secondary("Opening your shelf…");
        } else if self.library.is_empty() {
            screen = screen.splash(
                Some(Glyph::Reader),
                "Your shelf is empty",
                "Browse your home library or add a comic from your computer.",
            );
        } else {
            screen = screen
                .section("On this reader")
                .rows(self.library.iter().enumerate().map(|(index, comic)| {
                    (
                        format!("kept-{index}"),
                        comic.title.clone(),
                        format!(
                            "{} pages · {}",
                            comic.pages,
                            if comic.rtl {
                                "right to left"
                            } else {
                                "left to right"
                            }
                        ),
                        Glyph::Book,
                    )
                }));
        }
        if self
            .progress
            .values()
            .any(|draft| matches!(draft.status(), DraftStatus::Failed(_)))
        {
            screen = screen.button("retry-reading-save", "Retry saving positions");
        }
        screen.button("load-sideload", "Open added comic").build()
    }

    fn catalog_screen(&self) -> Screen {
        let title = self
            .catalog
            .as_ref()
            .and_then(|feed| feed.title.as_deref())
            .unwrap_or("Home library");
        let mut base = ScreenBuilder::new("panels-catalog")
            .top_bar(title)
            .owns_back(true);
        if self.catalog.is_some() {
            base = base.top_bar_action("search", "Search");
        }
        let mut screen = self.with_notice(base);
        let Some(feed) = &self.catalog else {
            return if self.notice.is_some() {
                screen
                    .splash(
                        Some(Glyph::Reader),
                        "Library unavailable",
                        "Check the address and sign-in on your computer, then try again.",
                    )
                    .primary_button("retry-catalog", "Try again")
                    .build()
            } else {
                screen.activity("Opening library", None).build()
            };
        };
        let query = self.query.to_ascii_lowercase();
        let publications = feed
            .publications
            .iter()
            .enumerate()
            .filter(|(_, publication)| {
                query.is_empty() || publication.title.to_ascii_lowercase().contains(&query)
            });
        screen = screen.rows(
            feed.navigation
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    (
                        format!("section-{index}"),
                        item.title.clone(),
                        item.summary.clone().unwrap_or_else(|| "Open".to_owned()),
                        Glyph::Reader,
                    )
                })
                .chain(publications.map(|(index, item)| {
                    (
                        format!("volume-{index}"),
                        item.title.clone(),
                        item.authors.join(", "),
                        Glyph::Book,
                    )
                })),
        );
        if !query.is_empty() {
            screen = screen.secondary(format!("Results for “{}”", self.query));
        }
        if let Some(previous) = feed.previous() {
            screen = screen.button(
                "catalog-previous",
                previous.title.as_deref().unwrap_or("Previous"),
            );
        }
        if let Some(next) = feed.next() {
            screen = screen.button("catalog-next", next.title.as_deref().unwrap_or("More"));
        }
        screen.build()
    }

    fn search_screen(&self) -> Screen {
        ScreenBuilder::new("panels-search")
            .top_bar("Search")
            .heading("Find a series or volume")
            .secondary("Search the library page you opened.")
            .keyboard(&self.keyboard, "Search")
            .owns_back(true)
            .build()
    }

    fn detail_screen(&self) -> Screen {
        let Some(publication) = &self.selected else {
            return self.catalog_screen();
        };
        let mut screen = self.with_notice(
            ScreenBuilder::new("panels-detail")
                .top_bar("Volume")
                .heading(&publication.title)
                .secondary(publication.authors.join(", "))
                .owns_back(true),
        );
        if let Some(cover) = self.cover {
            screen = screen.unframed_picture(cover, 42);
        }
        if let Some(summary) = &publication.summary {
            screen = screen.text(summary);
        }
        screen
            .primary_button("download", "Download")
            .button(
                "detail-rtl",
                if self.rtl {
                    "Reading order: right to left"
                } else {
                    "Reading order: left to right"
                },
            )
            .build()
    }

    fn download_screen(&self) -> Screen {
        let title = self
            .pending
            .as_ref()
            .map_or("Comic", |pending| pending.title.as_str());
        let received = self
            .transfer
            .as_ref()
            .map_or(0, |download| download.received.len() as u64);
        let mut screen = self.with_notice(
            ScreenBuilder::new("panels-download")
                .top_bar("Download")
                .heading(title)
                .transfer("Saving for offline reading", received, None)
                .owns_back(true),
        );
        if self.paused || self.notice.is_some() {
            screen = screen.buttons([("retry", "Retry"), ("cancel-download", "Remove")]);
        } else {
            screen = screen.button("pause-download", "Pause");
        }
        screen.build()
    }

    fn reader_screen(&self) -> Screen {
        self.view
            .as_ref()
            .map_or_else(|| self.library_screen(), ComicView::screen)
    }

    fn fetch_catalog(&mut self, context: &mut Context, url: String) {
        self.catalog = None;
        self.catalog_url.clone_from(&url);
        self.task = context
            .spawn_retrying(komga::fetch(url))
            .map(|task| (task, Awaiting::Catalog));
        self.notice = None;
    }

    fn select_publication(&mut self, context: &mut Context, publication: Publication) {
        self.selected = Some(publication);
        self.cover = None;
        self.notice = None;
        self.route = Route::Detail;
        let image = self
            .selected
            .as_ref()
            .and_then(Publication::cover)
            .map(|image| image.href.clone());
        match image {
            Some(ImageSource::Inline { bytes, .. }) => self.set_cover(context, &bytes),
            Some(ImageSource::Url(url)) => {
                self.task = context
                    .spawn_retrying(Task::Fetch {
                        url,
                        offset: 0,
                        max_bytes: MAX_IMAGE,
                        credential: Some(Credential::basic("komga")),
                        headers: Vec::new(),
                    })
                    .map(|task| (task, Awaiting::Cover));
            }
            None => {}
        }
    }

    fn set_cover(&mut self, context: &mut Context, bytes: &[u8]) {
        if let Ok(picture) = kobo_image::decode(bytes) {
            self.cover = context.put_picture(
                COVER,
                picture.width(),
                picture.height(),
                picture.grey().to_vec(),
            );
        }
    }

    fn load_sideload(&mut self, context: &mut Context) {
        let kept = self
            .library
            .iter()
            .find(|entry| entry.key == SIDELOAD)
            .cloned()
            .unwrap_or(Kept {
                key: SIDELOAD.into(),
                title: "Added comic".into(),
                pages: 0,
                rtl: false,
            });
        self.open_kept(context, &kept);
    }

    fn begin_download(&mut self, context: &mut Context) {
        let Some(publication) = self.selected.as_ref() else {
            return;
        };
        let Some(url) = komga::cbz(publication) else {
            self.notice = Some("This volume has no downloadable comic file.".to_owned());
            return;
        };
        let key = shelf_key(publication.identifier.as_deref().unwrap_or(&url));
        let pending = Pending {
            key,
            title: publication.title.clone(),
            url,
        };
        context.store().save(PARTIAL_META, encode_pending(&pending));
        self.transfer = Some(transfer::Download::new(pending.url.clone(), Vec::new()));
        self.pending = Some(pending);
        self.paused = false;
        self.notice = None;
        self.route = Route::Download;
        self.fetch_next_chunk(context);
    }

    fn fetch_next_chunk(&mut self, context: &mut Context) {
        if self.paused || self.task.is_some() || self.upload.is_some() {
            return;
        }
        let Some(download) = &self.transfer else {
            return;
        };
        self.task = context
            .spawn(Task::Fetch {
                url: download.url.clone(),
                offset: download.offset(),
                max_bytes: u32::try_from(transfer::CHUNK).expect("transfer chunk fits u32"),
                credential: Some(Credential::basic("komga")),
                headers: Vec::new(),
            })
            .map(|task| (task, Awaiting::Comic));
    }

    fn save_transfer(&mut self, context: &mut Context, complete: bool) {
        let Some(download) = &self.transfer else {
            return;
        };
        let name = if complete {
            self.pending
                .as_ref()
                .map_or(PARTIAL_BLOB, |pending| pending.key.as_str())
        } else {
            PARTIAL_BLOB
        };
        let mut upload = ShelfUpload::new(name, download.received.clone());
        upload.start(context);
        self.upload = Some((
            upload,
            if complete {
                Saving::Complete
            } else {
                Saving::Partial
            },
        ));
    }

    fn cancel_download(&mut self, context: &mut Context, remove: bool) {
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
        self.paused = true;
        if remove {
            context.shelf().remove(PARTIAL_BLOB);
            context.store().save(PARTIAL_META, Vec::new());
            self.transfer = None;
            self.pending = None;
            self.upload = None;
            self.route = Route::Library;
            self.notice = None;
        }
    }

    fn finish_download(&mut self, context: &mut Context) {
        let (Some(pending), Some(download)) = (self.pending.take(), self.transfer.take()) else {
            return;
        };
        let comic = match archive::inspect(&download.received) {
            Ok(comic) => comic,
            Err(error) => {
                self.notice = Some(error.to_string());
                self.pending = Some(pending);
                self.transfer = Some(download);
                self.paused = true;
                return;
            }
        };
        let kept = Kept {
            key: pending.key,
            title: pending.title,
            pages: comic.pages.len(),
            rtl: self.rtl,
        };
        self.library.retain(|item| item.key != kept.key);
        self.library.insert(0, kept.clone());
        context.store().save(LIBRARY, encode_library(&self.library));
        context.store().save(PARTIAL_META, Vec::new());
        context.shelf().remove(PARTIAL_BLOB);
        self.open_bytes(context, download.received, kept, None);
    }

    fn open_kept(&mut self, context: &mut Context, kept: &Kept) {
        if self.progress.len() >= 64 && !self.progress.contains_key(&progress_key(&kept.key)) {
            self.notice =
                Some("Save the pending reading positions before opening another comic.".into());
            return;
        }
        self.pending_open = Some(kept.clone());
        self.notice = Some("Opening comic.".to_owned());
        context.store().load(progress_key(&kept.key));
    }

    fn start_shelf_load(&mut self, context: &mut Context) {
        let Some(kept) = &self.pending_open else {
            return;
        };
        let mut download = ShelfDownload::new(&kept.key).at_most(transfer::MAX_COMIC);
        download.start(context);
        self.shelf_load = Some(download);
    }

    fn open_bytes(
        &mut self,
        context: &mut Context,
        bytes: Vec<u8>,
        mut kept: Kept,
        memory: Option<&[u8]>,
    ) {
        let mut view = match ComicView::open(context, bytes, &kept.title) {
            Ok(view) => view,
            Err(error) => {
                self.notice = Some(error.to_string());
                self.route = Route::Library;
                return;
            }
        };
        kept.pages = view.reader().comic().pages.len();
        if let Some(title) = &view.reader().comic().metadata.title {
            kept.title.clone_from(title);
        }
        if view.reader().comic().metadata.right_to_left.is_none() {
            view.set_direction(context, kept.rtl);
        }
        if let Some(draft) = self
            .progress
            .get(&progress_key(&kept.key))
            .filter(|draft| draft.status() != DraftStatus::Saved)
        {
            view.restore(context, Some(draft.bytes()));
        } else if memory.is_some() {
            view.restore(context, memory);
        }
        kept.rtl = view.reader().memory().right_to_left;
        self.library.retain(|entry| entry.key != kept.key);
        self.library.insert(0, kept.clone());
        context.store().save(LIBRARY, encode_library(&self.library));
        self.opened = Some(kept);
        self.view = Some(view);
        self.route = Route::Reader;
        self.notice = None;
        self.sync_progress_status();
    }

    fn save_reading_state(&mut self, context: &mut Context) {
        if let (Some(opened), Some(view)) = (&self.opened, &self.view) {
            match view.memory() {
                Ok(bytes) => {
                    let key = progress_key(&opened.key);
                    let draft = self.progress.entry(key).or_insert_with(|| {
                        Draft::restored(Vec::new(), kobo_comic::reader::MAX_MEMORY_BYTES)
                            .expect("bounded empty draft")
                    });
                    if draft.replace(bytes).is_err() {
                        self.notice =
                            Some("Reading position could not be prepared for saving.".into());
                    }
                }
                Err(error) => self.notice = Some(error),
            }
        }
        self.pump_progress(context);
    }

    fn pump_progress(&mut self, context: &mut Context) {
        if self.progress_active.is_none() {
            for (key, draft) in &mut self.progress {
                if let Some(write) = draft.begin() {
                    self.progress_active = Some((key.clone(), write.revision));
                    context.store().save(key, write.bytes);
                    break;
                }
            }
        }
        self.sync_progress_status();
    }

    fn sync_progress_status(&mut self) {
        if let (Some(opened), Some(view)) = (&self.opened, &mut self.view) {
            let state =
                self.progress
                    .get(&progress_key(&opened.key))
                    .map_or(SaveState::Saved, |draft| match draft.status() {
                        DraftStatus::Saved => SaveState::Saved,
                        DraftStatus::Unsaved | DraftStatus::Saving => SaveState::Pending,
                        DraftStatus::Failed(_) => SaveState::Failed,
                    });
            view.set_save_state(state);
        }
    }

    fn observe_progress(&mut self, context: &mut Context, result: &StoreResult) {
        let Some((key, revision)) = self.progress_active.as_ref() else {
            return;
        };
        let outcome = match result {
            StoreResult::Saved { key: saved } if saved == key => Ok(()),
            // The SDK associates even a denied response with its requested key.
            StoreResult::Denied(_) => {
                Err("Reading position was not saved. Free some space and retry.".into())
            }
            _ => return,
        };
        if let Some(draft) = self.progress.get_mut(key) {
            draft.finish(*revision, outcome.clone());
        }
        self.progress_active = None;
        let current = self.opened.as_ref().map(|opened| progress_key(&opened.key));
        self.progress.retain(|key, draft| {
            Some(key) == current.as_ref() || draft.status() != DraftStatus::Saved
        });
        if outcome.is_ok() {
            self.pump_progress(context);
        } else {
            self.sync_progress_status();
        }
    }

    fn retry_progress(&mut self, context: &mut Context) {
        for draft in self.progress.values_mut() {
            draft.retry();
        }
        self.pump_progress(context);
    }

    fn turn(&mut self, context: &mut Context, forward: bool) {
        if self
            .view
            .as_mut()
            .is_some_and(|view| view.turn(context, forward))
        {
            self.save_reading_state(context);
        }
    }

    fn catalog_link(&self, next: bool) -> Option<String> {
        self.catalog.as_ref().and_then(|feed| {
            if next { feed.next() } else { feed.previous() }.map(|link| link.href.clone())
        })
    }

    fn advance_upload(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some((upload, saving)) = &mut self.upload else {
            return false;
        };
        match upload.advance(context, result) {
            ShelfProgress::Done => {
                let saving = *saving;
                self.upload = None;
                match saving {
                    Saving::Partial => self.fetch_next_chunk(context),
                    Saving::Complete => self.finish_download(context),
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.upload = None;
                self.paused = true;
                self.notice =
                    Some("The download could not be saved. Free space, then retry.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn advance_shelf_load(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(download) = &mut self.shelf_load else {
            return false;
        };
        match download.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self.shelf_load.take().expect("active shelf load").take();
                if let Some(kept) = self.pending_open.take() {
                    let memory = self.pending_memory.take();
                    self.open_bytes(context, bytes, kept, memory.as_deref());
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.shelf_load = None;
                self.pending_open = None;
                self.notice = Some("This comic is missing from the reader.".to_owned());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }

    fn advance_partial_load(&mut self, context: &mut Context, result: &StoreResult) -> bool {
        let Some(download) = &mut self.partial_load else {
            return false;
        };
        match download.advance(context, result) {
            ShelfProgress::Done => {
                let bytes = self
                    .partial_load
                    .take()
                    .expect("active partial load")
                    .take();
                if let Some(pending) = &self.pending {
                    self.transfer = Some(transfer::Download::new(pending.url.clone(), bytes));
                    self.route = Route::Download;
                    self.paused = true;
                    self.notice = Some("A paused download is ready to continue.".to_owned());
                }
                true
            }
            ShelfProgress::Failed(_) => {
                self.partial_load = None;
                self.pending = None;
                context.store().save(PARTIAL_META, Vec::new());
                true
            }
            ShelfProgress::Moving { .. } => true,
            ShelfProgress::Elsewhere => false,
        }
    }
}

impl KoboApp for Panels {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(LIBRARY);
        context.store().load(PARTIAL_META);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if self.advance_upload(context, &result)
            || self.advance_shelf_load(context, &result)
            || self.advance_partial_load(context, &result)
        {
            self.show(context);
            return;
        }
        if let StoreResult::Loaded { key, value } = result {
            if key == LIBRARY {
                self.library = value.as_deref().map(decode_library).unwrap_or_default();
                self.loaded = true;
            } else if key == PARTIAL_META {
                if let Some(pending) = value.as_deref().and_then(decode_pending) {
                    self.pending = Some(pending);
                    let mut download =
                        ShelfDownload::new(PARTIAL_BLOB).at_most(transfer::MAX_COMIC);
                    download.start(context);
                    self.partial_load = Some(download);
                }
            } else if self
                .pending_open
                .as_ref()
                .is_some_and(|kept| key == progress_key(&kept.key))
            {
                self.pending_memory = value;
                self.start_shelf_load(context);
            }
        }
        self.show(context);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if self
            .progress_active
            .as_ref()
            .is_some_and(|(active, _)| active == key)
        {
            self.observe_progress(context, &result);
        }
        self.on_store(context, result);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.save_reading_state(context);
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive catalog and reader action dispatcher"
    )]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.route == Route::Reader {
            if let Some(view) = &mut self.view {
                match view.act(context, action) {
                    ComicOutcome::RetrySave => {
                        self.retry_progress(context);
                        self.show(context);
                        return;
                    }
                    ComicOutcome::Changed => {
                        self.save_reading_state(context);
                        self.show(context);
                        return;
                    }
                    ComicOutcome::Exit => {
                        self.save_reading_state(context);
                        if let Some(mut view) = self.view.take() {
                            view.close(context);
                        }
                        self.route = Route::Library;
                        self.show(context);
                        return;
                    }
                    ComicOutcome::Elsewhere => {}
                }
            }
        }
        if self.route == Route::Search {
            if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let entered = self.keyboard.take();
                entered.trim().clone_into(&mut self.query);
                self.route = Route::Catalog;
            }
            self.show(context);
            return;
        }
        if action == action_id("retry-reading-save") {
            self.retry_progress(context);
        } else if action == ActionId::BACK {
            match self.route {
                Route::Reader | Route::Download => {
                    self.save_reading_state(context);
                    self.route = Route::Library;
                }
                Route::Detail | Route::Search => self.route = Route::Catalog,
                Route::Catalog => {
                    if let Some((url, feed)) = self.history.pop() {
                        self.catalog_url = url;
                        self.catalog = Some(feed);
                    } else {
                        self.route = Route::Library;
                    }
                }
                Route::Library => {}
            }
        } else if action == action_id("load-sideload") {
            self.load_sideload(context);
        } else if action == action_id("browse-komga") {
            self.history.clear();
            self.query.clear();
            self.route = Route::Catalog;
            self.fetch_catalog(context, komga::CATALOG.to_owned());
        } else if action == action_id("retry-catalog") {
            self.fetch_catalog(context, self.catalog_url.clone());
        } else if action == action_id("search") {
            self.keyboard = Keyboard::with_text(&self.query);
            self.route = Route::Search;
        } else if action == action_id("download") {
            self.begin_download(context);
        } else if action == action_id("detail-rtl") || action == action_id("rtl") {
            self.rtl = !self.rtl;
            if let Some(opened) = &mut self.opened {
                opened.rtl = self.rtl;
                if let Some(kept) = self.library.iter_mut().find(|kept| kept.key == opened.key) {
                    kept.rtl = self.rtl;
                }
                context.store().save(LIBRARY, encode_library(&self.library));
            }
        } else if action == action_id("next") {
            self.turn(context, true);
        } else if action == action_id("previous") {
            self.turn(context, false);
        } else if action == action_id("pause-download") {
            self.cancel_download(context, false);
            self.notice = Some("Download paused.".to_owned());
        } else if action == action_id("retry") {
            self.paused = false;
            self.notice = None;
            self.fetch_next_chunk(context);
        } else if action == action_id("cancel-download") {
            self.cancel_download(context, true);
        } else if let Some(index) =
            (0..self.library.len()).find(|index| action == action_id(&format!("kept-{index}")))
        {
            let kept = self.library[index].clone();
            self.open_kept(context, &kept);
        } else if self.route == Route::Catalog {
            if action == action_id("catalog-next") || action == action_id("catalog-previous") {
                let next = action == action_id("catalog-next");
                if let Some(url) = self.catalog_link(next) {
                    if let Some(feed) = self.catalog.take() {
                        self.history.push((self.catalog_url.clone(), feed));
                    }
                    self.fetch_catalog(context, url);
                }
            } else if let Some(index) = self.catalog.as_ref().and_then(|feed| {
                (0..feed.navigation.len())
                    .find(|index| action == action_id(&format!("section-{index}")))
            }) {
                let url = self.catalog.as_ref().expect("catalog").navigation[index]
                    .href
                    .clone();
                if let Some(feed) = self.catalog.take() {
                    self.history.push((self.catalog_url.clone(), feed));
                }
                self.fetch_catalog(context, url);
            } else if let Some(publication) = self.catalog.as_ref().and_then(|feed| {
                feed.publications
                    .iter()
                    .enumerate()
                    .find(|(index, _)| action == action_id(&format!("volume-{index}")))
                    .map(|(_, publication)| publication.clone())
            }) {
                self.rtl = false;
                self.select_publication(context, publication);
            }
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, task: TaskId, outcome: TaskOutcome) {
        let Some((_, awaiting)) = self.task.take_if(|(known, _)| *known == task) else {
            return;
        };
        match (awaiting, outcome) {
            (Awaiting::Catalog, TaskOutcome::Completed(bytes)) => {
                match komga::parse(&bytes, &self.catalog_url) {
                    Ok(feed) => {
                        self.catalog = Some(feed);
                        self.notice = None;
                    }
                    Err(_) => self.notice = Some("This library page could not be read.".to_owned()),
                }
            }
            (Awaiting::Cover, TaskOutcome::Completed(bytes)) => self.set_cover(context, &bytes),
            (Awaiting::Comic, TaskOutcome::Completed(chunk)) => {
                let result = self
                    .transfer
                    .as_mut()
                    .expect("comic task has transfer")
                    .append(&chunk);
                if let Ok(done) = result {
                    self.save_transfer(context, done);
                } else {
                    self.paused = true;
                    self.notice =
                        Some("This comic is too large to keep on this reader.".to_owned());
                }
            }
            (_, TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential)) => {
                self.paused = true;
                self.notice = Some("Finish library sign-in on your computer.".to_owned());
            }
            (_, TaskOutcome::Failed(error)) => {
                self.paused = true;
                self.notice = Some(Failure::of(error).naming("home library"));
            }
            (_, TaskOutcome::Cancelled) => {}
        }
        self.show(context);
    }
}

fn clean_field(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn shelf_key(identity: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in identity.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("comic-{hash:016x}.cbz")
}

fn progress_key(key: &str) -> String {
    format!("place-{key}")
}

fn encode_library(library: &[Kept]) -> Vec<u8> {
    let mut output = String::new();
    for kept in library {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}",
            clean_field(&kept.key),
            clean_field(&kept.title),
            kept.pages,
            u8::from(kept.rtl)
        );
    }
    output.into_bytes()
}

fn decode_library(bytes: &[u8]) -> Vec<Kept> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(Kept {
                key: fields.next()?.to_owned(),
                title: fields.next()?.to_owned(),
                pages: fields.next()?.parse().ok()?,
                rtl: fields.next()? == "1",
            })
        })
        .collect()
}

fn encode_pending(pending: &Pending) -> Vec<u8> {
    format!(
        "{}\t{}\t{}",
        clean_field(&pending.key),
        clean_field(&pending.title),
        clean_field(&pending.url)
    )
    .into_bytes()
}

fn decode_pending(bytes: &[u8]) -> Option<Pending> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.is_empty() {
        return None;
    }
    let mut fields = text.split('\t');
    Some(Pending {
        key: fields.next()?.to_owned(),
        title: fields.next()?.to_owned(),
        url: fields.next()?.to_owned(),
    })
}

fn main() -> ExitCode {
    match kobo_sdk::run("panels", Panels::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("panels: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn failed_position_save_keeps_latest_page_and_retry_acknowledges_that_revision() {
        use kobo_sdk::{AppRunner, Context, KoboApp, StoreError, StoreResult};
        struct Harness(super::Panels);
        impl KoboApp for Harness {
            fn on_start(&mut self, context: &mut Context) {
                self.0.open_bytes(
                    context,
                    include_bytes!("../../../docs/quality/fixtures/original-pages.cbz").to_vec(),
                    super::Kept {
                        key: "fixture.cbz".into(),
                        title: "Rain".into(),
                        pages: 4,
                        rtl: false,
                    },
                    None,
                );
            }
            fn on_action(&mut self, context: &mut Context, action: kobo_sdk::ActionId) {
                self.0.on_action(context, action);
            }
            fn on_store(&mut self, context: &mut Context, result: StoreResult) {
                self.0.on_store(context, result);
            }
            fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
                self.0.on_save(context, key, result);
            }
        }
        let mut runner = AppRunner::new(Harness(super::Panels::default()));
        runner.start(); // Library save is still outstanding.
        runner.action(action_id("comic-next"));
        runner.action(action_id("comic-next")); // Coalesced while page 2 is being saved.
        let key = super::progress_key("fixture.cbz");
        runner.store_result(StoreResult::Denied(StoreError::TooFull)); // Library, not position.
        assert!(runner.app_mut().0.progress_active.is_some());
        runner.store_result(StoreResult::Denied(StoreError::TooFull)); // Actual position save.
        let app = &runner.app_mut().0;
        assert_eq!(app.view.as_ref().unwrap().reader().memory().page, 2);
        assert!(matches!(
            app.progress[&key].status(),
            super::DraftStatus::Failed(_)
        ));
        runner.action(action_id("comic-save-retry"));
        let app = &runner.app_mut().0;
        let latest = kobo_comic::reader::Memory::restore(
            Some(app.progress[&key].bytes()),
            app.view.as_ref().unwrap().reader().comic(),
        )
        .unwrap();
        assert_eq!(latest.page, 2);
        runner.store_result(StoreResult::Saved { key: key.clone() });
        assert_eq!(
            runner.app_mut().0.progress[&key].status(),
            super::DraftStatus::Saved
        );
    }
    use super::{
        decode_library, decode_pending, encode_library, encode_pending, shelf_key, Kept, Panels,
        Pending,
    };
    use kobo_sdk::action_id;
    use kobo_ui::{Chrome, CLARA_BW_METRICS};

    #[test]
    fn library_and_pending_transfer_round_trip() {
        let kept = vec![Kept {
            key: "comic.cbz".into(),
            title: "Volume 1".into(),
            pages: 192,
            rtl: true,
        }];
        assert_eq!(decode_library(&encode_library(&kept)), kept);
        let pending = Pending {
            key: "comic.cbz".into(),
            title: "Volume 1".into(),
            url: "https://library/one.cbz".into(),
        };
        assert_eq!(decode_pending(&encode_pending(&pending)), Some(pending));
    }

    #[test]
    fn shelf_keys_are_stable_and_do_not_expose_server_paths() {
        assert_eq!(shelf_key("book-1"), shelf_key("book-1"));
        assert_ne!(shelf_key("book-1"), shelf_key("book-2"));
        assert!(!shelf_key("https://private/library").contains("private"));
    }

    #[test]
    fn primary_library_controls_fit_the_actual_panel() {
        let app = Panels::default();
        let screen = app.library_screen();
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::default());
        assert!(layout.rect_of_action(action_id("load-sideload")).is_some());
        assert!(screen
            .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
            .issues
            .is_empty());
    }
}
