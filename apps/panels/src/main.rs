//! A local and self-hosted comics shelf with resumable downloads.

mod archive;
mod komga;
mod transfer;

use kobo_opds::{Feed, ImageSource, Publication};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Failure, Glyph, KoboApp, PictureHandle,
    Screen, ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId,
    TaskOutcome, TilePicture,
};
use std::fmt::Write as _;
use std::process::ExitCode;

const SIDELOAD: &str = "volume.cbz";
const LIBRARY: &str = "library";
const PARTIAL_META: &str = "partial";
const PARTIAL_BLOB: &str = "partial.cbz";
const PAGE: PictureHandle = PictureHandle(1);
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
    Sideload,
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
    bytes: Option<Vec<u8>>,
    comic: Option<archive::Comic>,
    opened: Option<Kept>,
    page: usize,
    rtl: bool,
    picture: Option<TilePicture>,
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
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            route: Route::Library,
            bytes: None,
            comic: None,
            opened: None,
            page: 0,
            rtl: false,
            picture: None,
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
        let Some(comic) = &self.comic else {
            return self.library_screen();
        };
        let title = self
            .opened
            .as_ref()
            .map_or("Comic", |opened| opened.title.as_str());
        let mut screen = self.with_notice(
            ScreenBuilder::new("panels-reader")
                .top_bar(title)
                .top_bar_action("rtl", if self.rtl { "RTL" } else { "LTR" })
                .secondary(format!("Page {} of {}", self.page + 1, comic.pages.len())),
        );
        if let Some(picture) = self.picture {
            screen = screen.unframed_picture(picture, 154);
        } else {
            screen = screen.activity("Opening page", None);
        }
        screen.page_turns("previous", "next").build()
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
        self.task = context
            .spawn(Task::ReadFile {
                path: SIDELOAD.to_owned(),
            })
            .map(|task| (task, Awaiting::Sideload));
        self.notice = Some("Opening added comic.".to_owned());
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
        let Ok(comic) = archive::inspect(&download.received) else {
            self.notice = Some("The downloaded comic could not be opened.".to_owned());
            self.pending = Some(pending);
            self.transfer = Some(download);
            self.paused = true;
            return;
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
        self.bytes = Some(download.received);
        self.comic = Some(comic);
        self.opened = Some(kept);
        self.page = 0;
        self.picture = None;
        self.route = Route::Reader;
        self.notice = None;
        self.display_page(context);
    }

    fn open_kept(&mut self, context: &mut Context, kept: &Kept) {
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

    fn open_bytes(&mut self, context: &mut Context, bytes: Vec<u8>, kept: Kept, page: usize) {
        if let Ok(comic) = archive::inspect(&bytes) {
            self.page = page.min(comic.pages.len().saturating_sub(1));
            self.rtl = kept.rtl;
            self.opened = Some(kept);
            self.bytes = Some(bytes);
            self.comic = Some(comic);
            self.picture = None;
            self.route = Route::Reader;
            self.notice = None;
            self.display_page(context);
        } else {
            self.notice = Some("This saved comic can no longer be opened.".to_owned());
            self.route = Route::Library;
        }
    }

    fn display_page(&mut self, context: &mut Context) {
        let (Some(bytes), Some(comic)) = (&self.bytes, &self.comic) else {
            return;
        };
        match archive::page(bytes, comic, self.page) {
            Ok(picture) => {
                let picture = match picture.fit_enlarging(976, 1120) {
                    Ok(fitted) => fitted,
                    Err(_) => picture,
                };
                self.picture = context.put_picture(
                    PAGE,
                    picture.width(),
                    picture.height(),
                    picture.grey().to_vec(),
                );
                self.notice = self
                    .picture
                    .is_none()
                    .then_some("This page is too large to display.".to_owned());
            }
            Err(_) => {
                self.notice = Some("This page could not be opened. You can skip it.".to_owned());
            }
        }
    }

    fn save_reading_state(&self, context: &mut Context) {
        if let Some(opened) = &self.opened {
            context.store().save(
                progress_key(&opened.key),
                self.page.to_string().into_bytes(),
            );
        }
    }

    fn turn(&mut self, context: &mut Context, forward: bool) {
        let Some(comic) = &self.comic else {
            return;
        };
        let forward = if self.rtl { !forward } else { forward };
        if forward && self.page + 1 < comic.pages.len() {
            self.page += 1;
        } else if !forward && self.page > 0 {
            self.page -= 1;
        } else {
            return;
        }
        self.picture = None;
        self.notice = None;
        self.save_reading_state(context);
        self.display_page(context);
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
                    self.open_bytes(context, bytes, kept, self.page);
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
                self.page = value
                    .as_deref()
                    .and_then(|bytes| std::str::from_utf8(bytes).ok())
                    .and_then(|text| text.parse().ok())
                    .unwrap_or(0);
                self.start_shelf_load(context);
            }
        }
        self.show(context);
    }

    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.route == Route::Search {
            if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let entered = self.keyboard.take();
                entered.trim().clone_into(&mut self.query);
                self.route = Route::Catalog;
            }
            self.show(context);
            return;
        }
        if action == ActionId::BACK {
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
            (Awaiting::Sideload, TaskOutcome::Completed(bytes)) => {
                let title = "Added comic".to_owned();
                match archive::inspect(&bytes) {
                    Ok(comic) => {
                        self.open_bytes(
                            context,
                            bytes,
                            Kept {
                                key: SIDELOAD.to_owned(),
                                title,
                                pages: comic.pages.len(),
                                rtl: self.rtl,
                            },
                            0,
                        );
                    }
                    Err(_) => self.notice = Some("The added comic could not be opened.".to_owned()),
                }
            }
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
