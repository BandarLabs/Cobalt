//! A local and self-hosted comics shelf with resumable downloads.

mod archive;
mod catalog;
mod downloads;
mod komga;
mod library;
mod previews;
mod server;
mod sideload;
use library::{Kept, Library};
mod transfer;

use kobo_bookview::comic::{ComicView, Outcome as ComicOutcome, SaveState};
use kobo_opds::{Feed, ImageSource, Publication};
use kobo_sdk::imports::{Format, Import, Stage as ImportStage};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, PictureHandle,
    Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId,
    TaskOutcome, TilePicture,
};
use kobo_state::draft::{Draft, Status as DraftStatus};
use std::collections::BTreeMap;
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
    SavingLibrary,
    Import,
    ImportHelp,
    Server,
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
struct Pending {
    key: String,
    title: String,
    url: String,
}

struct Panels {
    route: Route,
    view: Option<ComicView>,
    identity: Option<kobo_sdk::DeviceIdentity>,
    pending_memory: Option<Vec<u8>>,
    opened: Option<Kept>,
    rtl: bool,
    cover: Option<TilePicture>,
    notice: Option<String>,
    task: Option<(TaskId, Awaiting)>,
    catalog: Option<Feed>,
    server: server::Server,
    catalog_url: String,
    catalog_page: usize,
    history: Vec<(String, Feed, usize, String)>,
    selected: Option<Publication>,
    query: String,
    keyboard: Keyboard,
    library: Option<Library>,
    library_error: Option<String>,
    completed_cleanup: Option<String>,
    loaded: bool,
    library_page: usize,
    import_help_page: usize,
    transfer: Option<transfer::Download>,
    pending: Option<Pending>,
    upload: Option<(ShelfUpload, Saving)>,
    recovery: downloads::Recovery,
    shelf_load: Option<ShelfDownload>,
    local_load: Option<ShelfDownload>,
    import: Option<Import>,
    import_entry: Option<Kept>,
    partial_load: Option<ShelfDownload>,
    pending_open: Option<Kept>,
    paused: bool,
    progress: BTreeMap<String, Draft>,
    progress_active: Option<(String, u64)>,
    progress_snapshot: Option<Vec<u8>>,
    previews: previews::Previews,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            route: Route::Library,
            view: None,
            identity: None,
            pending_memory: None,
            opened: None,
            rtl: false,
            cover: None,
            notice: None,
            task: None,
            catalog: None,
            server: server::Server::default(),
            catalog_url: String::new(),
            catalog_page: 0,
            history: Vec::new(),
            selected: None,
            query: String::new(),
            keyboard: Keyboard::new(),
            library: None,
            library_error: None,
            completed_cleanup: None,
            loaded: false,
            library_page: 0,
            import_help_page: 0,
            transfer: None,
            pending: None,
            upload: None,
            recovery: downloads::Recovery::default(),
            shelf_load: None,
            local_load: None,
            import: None,
            import_entry: None,
            partial_load: None,
            pending_open: None,
            paused: false,
            progress: BTreeMap::new(),
            progress_active: None,
            progress_snapshot: None,
            previews: previews::Previews::default(),
        }
    }
}

impl Panels {
    fn library_entries(&self) -> &[Kept] {
        self.library.as_ref().map_or(&[], Library::entries)
    }
    fn remember(&mut self, context: &mut Context, kept: Kept) -> bool {
        if let Some(library) = &mut self.library {
            match library.remember(kept) {
                Ok(()) => {
                    self.library_error = None;
                    library.pump(context);
                    return true;
                }
                Err(error) => self.notice = Some(format!("This comic could not be added: {error}")),
            }
        }
        false
    }
    fn finish_library(&mut self, context: &mut Context) {
        if self
            .library
            .as_ref()
            .is_some_and(|library| library.status() == DraftStatus::Saved)
        {
            if self
                .completed_cleanup
                .as_ref()
                .is_some_and(|key| self.library_entries().iter().any(|comic| &comic.key == key))
                && self.import.as_ref().is_some_and(Import::is_available)
            {
                self.completed_cleanup = None;
                self.recovery.discard_requested = true;
                self.paused = true;
                self.drain_removal(context);
            }
            if self.route == Route::SavingLibrary {
                self.route = Route::Reader;
            }
        }
    }
    fn show(&mut self, context: &mut Context) {
        if self.route == Route::Library && self.loaded && self.pending_open.is_none() {
            let pages = self.library_pages(context);
            let page = self.library_page.min(pages.len().saturating_sub(1));
            let visible = pages
                .get(page)
                .into_iter()
                .flatten()
                .map(|&index| self.library_entries()[index].clone())
                .collect::<Vec<_>>();
            self.previews.prepare(context, &visible);
        }
        context.set_screen(
            self.screen(context)
                .with_own_back(self.route != Route::Library),
        );
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.route {
            Route::Library => self.library_screen(context),
            Route::Server => self.server.screen(),
            Route::ImportHelp => self.import_help_screen(),
            Route::Catalog => self.catalog_screen(context),
            Route::Search => self.search_screen(),
            Route::Detail => self.detail_screen(context),
            Route::Download => self.download_screen(),
            Route::Reader => self.reader_screen(context),
            Route::SavingLibrary => self.saving_library_screen(),
            Route::Import
                if self.import.as_ref().is_some_and(Import::is_available)
                    && self
                        .library
                        .as_ref()
                        .is_some_and(|library| library.status() != DraftStatus::Saved) =>
            {
                self.saving_library_screen()
            }
            Route::Import => self.import.as_ref().map_or_else(
                || {
                    ScreenBuilder::new("panels-import-loading")
                        .top_bar("Add comic")
                        .activity("Checking the file from your computer…", None)
                        .build()
                },
                Import::screen,
            ),
        }
    }

    fn with_notice(&self, mut screen: ScreenBuilder) -> ScreenBuilder {
        if let Some(note) = &self.notice {
            screen = screen.banner(BannerLevel::Attention, note);
        }
        screen
    }

    fn shelf_notice(&self) -> Option<&str> {
        self.library
            .as_ref()
            .and_then(|library| match library.status() {
                DraftStatus::Failed(error) => Some(error),
                _ => None,
            })
            .or(self.notice.as_deref())
    }
    fn library_pages(&self, context: &Context) -> Vec<Vec<usize>> {
        let summaries = self
            .library_entries()
            .iter()
            .map(|comic| self.previews.summary(comic))
            .collect::<Vec<_>>();
        let titles = self
            .library_entries()
            .iter()
            .map(|comic| context.clamped_cover_row(&comic.title, 2, true))
            .collect::<Vec<_>>();
        let rows = titles
            .iter()
            .zip(&summaries)
            .map(|(title, summary)| (title.as_str(), summary.as_str(), ""))
            .collect::<Vec<_>>();
        context.paginate_cover_rows_below_section(
            &rows,
            true,
            kobo_sdk::Position::AtTheFoot,
            self.shelf_notice(),
        )
    }
    fn library_screen(&self, context: &Context) -> Screen {
        if let Some(error) = &self.library_error {
            return ScreenBuilder::new("panels-library-error")
                .top_bar("Panels")
                .heading("Your comic list could not be opened")
                .text(error)
                .bottom_action("retry-library", "Try again")
                .build();
        }
        let mut screen = ScreenBuilder::new("panels-library").top_bar("Panels");
        if let Some(notice) = self.shelf_notice() {
            screen = screen.banner(BannerLevel::Attention, notice);
        }
        if !self.loaded {
            return screen.activity("Opening your shelf", None).build();
        }
        if self.library_entries().is_empty() {
            screen = screen
                .splash(
                    Some(Glyph::Reader),
                    "Your shelf is empty",
                    "Browse your home library or add a comic from your computer.",
                )
                .button("sample-comic", "Try a sample comic");
        } else {
            let pages = self.library_pages(context);
            let page = self.library_page.min(pages.len().saturating_sub(1));
            let visible = pages.get(page).map(Vec::as_slice).unwrap_or_default();
            screen = screen
                .section("On this reader")
                .rows(visible.iter().map(|&index| {
                    let comic = &self.library_entries()[index];
                    (
                        format!("kept-{}", comic.key),
                        context.clamped_cover_row(&comic.title, 2, true),
                        self.previews.summary(comic),
                        self.previews.lead(&comic.key),
                    )
                }))
                .page_turns("shelf-previous", "shelf-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len().max(1)).unwrap_or(u16::MAX),
                );
        }
        if self
            .progress
            .values()
            .any(|draft| matches!(draft.status(), DraftStatus::Failed(_)))
            || self
                .library
                .as_ref()
                .is_some_and(|library| matches!(library.status(), DraftStatus::Failed(_)))
        {
            screen
                .action_bar([
                    ("load-sideload", "Add comic"),
                    ("browse-komga", "Browse"),
                    ("retry-all-saves", "Retry"),
                ])
                .build()
        } else if self.pending.is_some() || self.recovery_busy() || !self.recovery.loaded {
            screen
                .action_bar([
                    ("load-sideload", "Add comic"),
                    ("browse-komga", "Browse"),
                    ("show-download", "Download"),
                ])
                .build()
        } else {
            screen
                .action_bar([("load-sideload", "Add comic"), ("browse-komga", "Browse")])
                .build()
        }
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

    fn detail_screen(&self, context: &Context) -> Screen {
        let Some(publication) = &self.selected else {
            return self.catalog_screen(context);
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
        let mut screen = ScreenBuilder::new("panels-download")
            .top_bar("Download")
            .text(catalog::preview(title, 60))
            .owns_back(true);
        if let Some(notice) = &self.notice {
            screen = screen.text(notice);
        } else {
            screen = screen.transfer(
                if self
                    .transfer
                    .as_ref()
                    .is_some_and(transfer::Download::checking)
                {
                    "Checking saved pages against your server"
                } else {
                    "Saving for offline reading"
                },
                received,
                None,
            );
        }
        if self.paused || self.notice.is_some() {
            screen = screen.action_bar([
                (
                    "retry",
                    if self
                        .transfer
                        .as_ref()
                        .is_some_and(|download| download.complete)
                    {
                        "Add to shelf"
                    } else {
                        "Retry"
                    },
                ),
                ("cancel-download", "Remove"),
            ]);
        } else {
            screen = screen.bottom_action("pause-download", "Pause");
        }
        screen.build()
    }

    fn saving_library_screen(&self) -> Screen {
        let screen = ScreenBuilder::new("panels-saving-library").top_bar("Add comic");
        match self.library.as_ref().map(Library::status) {
            Some(DraftStatus::Failed(error)) => screen
                .text(error)
                .bottom_action("retry-library", "Retry saving")
                .build(),
            _ => screen.activity("Adding comic to your shelf", None).build(),
        }
    }

    fn reader_screen(&self, context: &Context) -> Screen {
        self.view
            .as_ref()
            .map_or_else(|| self.library_screen(context), ComicView::screen)
    }

    fn fetch_catalog(&mut self, context: &mut Context, url: String) {
        if let Some((task, _)) = self.task.take() {
            context.cancel(task);
        }
        self.catalog = None;
        self.catalog_page = 0;
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

    fn open_kept(&mut self, context: &mut Context, kept: &Kept) {
        if self.library.is_none() || self.pending_open.is_some() || self.shelf_load.is_some() {
            return;
        }
        if self.progress.len() >= 64 && !self.progress.contains_key(&progress_key(&kept.key)) {
            self.notice =
                Some("Save the pending reading positions before opening another comic.".into());
            return;
        }
        self.pending_open = Some(kept.clone());
        self.notice = Some("Opening comic.".to_owned());
        if !self.previews.claim_position_read(&kept.key) {
            context.store().load(progress_key(&kept.key));
        }
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
    ) -> bool {
        let mut view = match ComicView::open(context, bytes, &kept.title) {
            Ok(view) => view,
            Err(error) => {
                self.notice = Some(error.to_string());
                self.route = Route::Library;
                return false;
            }
        };
        if self
            .identity
            .as_ref()
            .is_some_and(kobo_sdk::DeviceIdentity::colour_panel)
        {
            view.set_colour(context, true);
        }
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
        if !self.remember(context, kept.clone()) {
            self.route = Route::Library;
            return false;
        }
        if let Ok(picture) = view.cover_preview() {
            self.previews.staged =
                previews::encode(&picture).map(|bytes| (kept.key.clone(), bytes));
            self.previews.publish(context, &kept.key);
        }
        self.opened = Some(kept);
        self.library_page = 0;
        self.view = Some(view);
        self.route = Route::SavingLibrary;
        self.notice = None;
        self.finish_library(context);
        self.sync_progress_status();
        true
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
                    self.progress_snapshot = Some(write.bytes.clone());
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
        if let Some(bytes) = self.progress_snapshot.take() {
            if outcome.is_ok() {
                let comics = self.library_entries().to_vec();
                self.previews.saved_position(key, &bytes, &comics);
            }
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
}

impl KoboApp for Panels {
    fn on_start(&mut self, context: &mut Context) {
        context.device().read_identity();
        context.store().load(LIBRARY);
        self.recovery.metadata_loading = true;
        context.store().load(PARTIAL_META);
        context.store().load(server::KEY);
        self.show(context);
    }

    fn on_device_result(
        &mut self,
        context: &mut Context,
        request: kobo_sdk::DeviceRequest,
        result: kobo_sdk::DeviceResult,
    ) {
        if self
            .server
            .setup
            .on_device_result(&request, &result)
            .is_some()
        {
            self.show(context);
            return;
        }
        if request == kobo_sdk::DeviceRequest::ReadIdentity {
            self.identity = match result {
                kobo_sdk::DeviceResult::Identity(identity) => Some(identity),
                _ => None,
            };
            let colour = self
                .identity
                .as_ref()
                .is_some_and(kobo_sdk::DeviceIdentity::colour_panel);
            if let Some(view) = &mut self.view {
                view.set_colour(context, colour);
            }
            self.show(context);
        }
    }

    fn on_load(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if self.previews.loaded(context, key, &result) {
            self.show(context);
            return;
        }
        if key == server::KEY {
            self.server.load(&result);
            self.show(context);
            return;
        }
        if matches!(&result, StoreResult::Loaded { key: loaded, .. } if loaded == key) {
            self.on_store(context, result);
            return;
        }
        if key == LIBRARY {
            self.loaded = true;
            self.library_error = Some(
                "The comic list could not be read. Try again when storage is available.".into(),
            );
        } else if key == PARTIAL_META {
            self.recovery.metadata_loading = false;
            self.recovery.loaded = false;
            self.route = Route::Download;
            self.paused = true;
            self.notice = Some(
                "The paused download could not be checked. Retry when storage is available.".into(),
            );
        } else if self.pending_open.as_ref().is_some_and(|kept| {
            key == progress_key(&kept.key) || legacy_progress_key(&kept.key).as_deref() == Some(key)
        }) {
            self.pending_open = None;
            self.notice = Some(
                "The saved reading position could not be read. Try opening this comic again."
                    .into(),
            );
        }
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = result {
            if key == LIBRARY {
                if self.library.is_none() {
                    match Library::restore(value.as_deref()) {
                        Ok(mut library) => {
                            library.pump(context);
                            self.library = Some(library);
                            self.library_error = None;
                        }
                        Err(error) => self.library_error = Some(error.to_string()),
                    }
                }
                self.loaded = true;
            } else if key == PARTIAL_META {
                if self.recovery.discard_requested {
                    self.recovery.metadata_loading = false;
                    self.drain_removal(context);
                } else {
                    self.restore_download(context, value.as_deref());
                }
            } else if self.pending_open.as_ref().is_some_and(|kept| {
                key == progress_key(&kept.key)
                    || legacy_progress_key(&kept.key).as_deref() == Some(key.as_str())
            }) {
                if value.is_none()
                    && self
                        .pending_open
                        .as_ref()
                        .is_some_and(|kept| key == progress_key(&kept.key))
                {
                    if let Some(legacy) = self
                        .pending_open
                        .as_ref()
                        .and_then(|kept| legacy_progress_key(&kept.key))
                    {
                        context.store().load(legacy);
                        self.show(context);
                        return;
                    }
                }
                self.pending_memory = value;
                self.start_shelf_load(context);
            }
        }
        self.show(context);
    }

    fn on_shelf(&mut self, context: &mut Context, name: &str, result: StoreResult) {
        if self.removed_blob(name, &result) {
            self.show(context);
            return;
        }
        if self
            .local_load
            .as_ref()
            .is_some_and(|load| load.name() == name)
        {
            self.advance_local_load(context, &result);
            self.show(context);
            return;
        }
        if self
            .import
            .as_ref()
            .is_some_and(|import| import.receipt().digest == name)
        {
            let import = self.import.as_mut().expect("matching import");
            if import.stage() == ImportStage::Cancelled {
                self.import = None;
            } else {
                import.on_shelf(context, name, &result);
            }
            self.show(context);
            return;
        }
        if self
            .upload
            .as_ref()
            .is_some_and(|(upload, _)| upload.name() == name)
        {
            self.advance_upload(context, &result);
        } else if self
            .shelf_load
            .as_ref()
            .is_some_and(|load| load.name() == name)
        {
            self.advance_shelf_load(context, &result);
        } else if self
            .partial_load
            .as_ref()
            .is_some_and(|load| load.name() == name)
        {
            self.advance_partial_load(context, &result);
        }
        self.show(context);
    }

    fn on_save(&mut self, context: &mut Context, key: &str, result: StoreResult) {
        if key == PARTIAL_META {
            self.saved_metadata(context, &result);
            self.show(context);
            return;
        }
        if key == server::KEY {
            self.server.saved(context, &result);
            self.show(context);
            return;
        }
        if self
            .import
            .as_ref()
            .is_some_and(|import| import.receipt().digest == key)
        {
            let import = self.import.as_mut().expect("matching import");
            if import.stage() == ImportStage::Cancelled {
                self.import = None;
            } else {
                import.on_save(key, &result);
                if import.is_available() {
                    self.previews.publish(context, key);
                    if let Some(entry) = self.import_entry.clone() {
                        if self.remember(context, entry) {
                            self.finish_library(context);
                        } else {
                            self.import = None;
                            self.route = Route::Library;
                        }
                    }
                }
            }
            self.show(context);
            return;
        }
        if key == LIBRARY {
            if let Some(library) = &mut self.library {
                library.saved(context, &result);
            }
            self.finish_library(context);
            self.sync_progress_status();
        }
        if self
            .progress_active
            .as_ref()
            .is_some_and(|(active, _)| active == key)
        {
            self.observe_progress(context, &result);
        }
        self.show(context);
    }

    fn on_background(&mut self, context: &mut Context) {
        self.save_reading_state(context);
        self.server.pump(context);
        if let Some(library) = &mut self.library {
            library.pump(context);
        }
    }

    fn on_suspend(&mut self, context: &mut Context) {
        self.cancel_download(context, false);
        self.on_background(context);
    }

    fn can_suspend(&self) -> bool {
        self.library
            .as_ref()
            .is_some_and(|library| library.status() == DraftStatus::Saved)
            && self
                .progress
                .values()
                .all(|draft| draft.status() == DraftStatus::Saved)
            && self.server.can_suspend()
            && !self.recovery_busy()
            && self.import.as_ref().is_none_or(|import| {
                import.failure().is_none()
                    && matches!(
                        import.stage(),
                        ImportStage::Ready | ImportStage::Preview | ImportStage::Cancelled
                    )
            })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive catalog and reader action dispatcher"
    )]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.route == Route::ImportHelp {
            if action == action_id("import-help-next") {
                self.import_help_page = (self.import_help_page + 1).min(3);
            } else if action == ActionId::BACK {
                if self.import_help_page == 0 {
                    self.route = Route::Library;
                } else {
                    self.import_help_page -= 1;
                }
            } else if action == action_id("load-sideload") {
                self.load_sideload(context);
            } else if action == action_id("sample-comic") {
                self.load_sample();
            }
            self.show(context);
            return;
        }
        if self.route == Route::Server {
            if self.server.on_action(context, action) == Some(kobo_sdk::provider::Event::Closed) {
                self.route = Route::Library;
            }
            self.show(context);
            return;
        }
        if self.route == Route::Import {
            if action == ActionId::BACK
                || action == action_id("import-cancel")
                || action == action_id("import-replace")
            {
                self.cancel_import();
            } else if action == action_id("retry-library") {
                if let Some(library) = &mut self.library {
                    library.retry(context);
                }
            } else if let Some(import) = &mut self.import {
                if action == action_id("import-confirm") || action == action_id("import-retry") {
                    import.begin(context);
                } else if action == action_id("import-open")
                    && import.is_available()
                    && self
                        .library
                        .as_ref()
                        .is_some_and(|library| library.status() == DraftStatus::Saved)
                {
                    let kept = Kept {
                        key: import.receipt().digest.clone(),
                        title: import.receipt().title.clone(),
                        pages: 0,
                        rtl: false,
                    };
                    self.import = None;
                    self.import_entry = None;
                    self.route = Route::Library;
                    self.open_kept(context, &kept);
                }
            }
            self.show(context);
            return;
        }
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
                self.catalog_page = 0;
                self.route = Route::Catalog;
            }
            self.show(context);
            return;
        }
        if action == action_id("shelf-previous") {
            self.library_page = self.library_page.saturating_sub(1);
        } else if action == action_id("shelf-next") {
            self.library_page =
                (self.library_page + 1).min(self.library_pages(context).len().saturating_sub(1));
        } else if action == action_id("retry-all-saves") {
            self.retry_progress(context);
            if let Some(library) = &mut self.library {
                library.retry(context);
            }
        } else if action == action_id("retry-reading-save") {
            self.retry_progress(context);
        } else if action == ActionId::BACK {
            match self.route {
                Route::Download => {
                    self.cancel_download(context, false);
                    self.route = Route::Library;
                }
                Route::Reader | Route::SavingLibrary => {
                    self.save_reading_state(context);
                    self.route = Route::Library;
                }
                Route::Detail | Route::Search => self.route = Route::Catalog,
                Route::Catalog => {
                    if let Some((task, _)) = self.task.take() {
                        context.cancel(task);
                    }
                    if let Some((url, feed, page, query)) = self.history.pop() {
                        self.catalog_url = url;
                        self.catalog = Some(feed);
                        self.catalog_page = page;
                        self.query = query;
                    } else {
                        self.route = Route::Library;
                    }
                }
                Route::Library | Route::Import | Route::Server | Route::ImportHelp => {}
            }
        } else if action == action_id("retry-library") {
            if let Some(library) = &mut self.library {
                library.retry(context);
            } else {
                context.store().load(LIBRARY);
            }
        } else if action == action_id("sample-comic") {
            self.load_sample();
        } else if action == action_id("load-sideload") {
            self.load_sideload(context);
        } else if action == action_id("show-download") {
            self.route = Route::Download;
        } else if action == action_id("browse-komga") {
            self.cancel_download(context, false);
            self.history.clear();
            self.query.clear();
            self.route = Route::Server;
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
                let kept = opened.clone();
                self.remember(context, kept);
            }
        } else if action == action_id("next") {
            self.turn(context, true);
        } else if action == action_id("previous") {
            self.turn(context, false);
        } else if action == action_id("pause-download") {
            self.cancel_download(context, false);
            self.notice = Some("Download paused.".to_owned());
        } else if action == action_id("retry") {
            self.retry_download(context);
        } else if action == action_id("cancel-download") {
            self.cancel_download(context, true);
        } else if let Some(index) = (0..self.library_entries().len()).find(|index| {
            action == action_id(&format!("kept-{}", self.library_entries()[*index].key))
        }) {
            let kept = self.library_entries()[index].clone();
            self.open_kept(context, &kept);
        } else if self.route == Route::Catalog {
            if action == action_id("catalog-page-back") {
                self.catalog_page = self.catalog_page.saturating_sub(1);
            } else if action == action_id("catalog-page-next") {
                self.catalog_page = (self.catalog_page + 1)
                    .min(self.catalog_pages(context).len().saturating_sub(1));
            } else if action == action_id("catalog-next") || action == action_id("catalog-previous")
            {
                let next = action == action_id("catalog-next");
                if let Some(url) = self.catalog_link(next) {
                    self.follow_catalog(context, url);
                }
            } else if let Some(index) = self.catalog.as_ref().and_then(|feed| {
                (0..feed.navigation.len())
                    .find(|index| action == action_id(&format!("section-{index}")))
            }) {
                let url = self.catalog.as_ref().expect("catalog").navigation[index]
                    .href
                    .clone();
                self.follow_catalog(context, url);
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
        if let Some(event) = self.server.setup.on_task(task, &outcome) {
            if let kobo_sdk::provider::Event::Response(bytes) = event {
                let url = self.server.catalog_url();
                match komga::parse(&bytes, &url) {
                    Ok(feed) if self.server.setup.verified() && self.server.is_saved() => {
                        self.catalog = Some(feed);
                        self.catalog_url = url;
                        self.catalog_page = 0;
                        self.notice = None;
                        self.route = Route::Catalog;
                    }
                    _ => self.server.setup.invalid_response(),
                }
            }
            self.show(context);
            return;
        }
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
                match result {
                    Ok(transfer::Step::Checked) => self.fetch_next_chunk(context),
                    Ok(step) => self.save_transfer(context, step == transfer::Step::Complete),
                    Err(error) => {
                        self.paused = true;
                        self.notice = Some(error.into());
                    }
                }
            }
            (_, TaskOutcome::Failed(kobo_sdk::TaskError::NoCredential)) => {
                self.paused = true;
                self.notice =
                    Some("Open Browse and update Account details, then try again.".to_owned());
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

fn shelf_key(identity: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in identity.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("comic-{hash:016x}.cbz")
}

fn progress_key(key: &str) -> String {
    kobo_net::sha256::hex_digest(format!("cobalt.panels.position.v1\0{key}").as_bytes())
}

fn legacy_progress_key(key: &str) -> Option<String> {
    let legacy = format!("place-{key}");
    kobo_sdk::is_valid_key(&legacy).then_some(legacy)
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
mod tests;

#[cfg(test)]
mod recovery_tests;
