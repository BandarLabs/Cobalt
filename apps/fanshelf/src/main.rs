//! A tap-driven, offline-after-download reader for public AO3 works.
mod library;

use kobo_bookview::{BookView, Step};
use kobo_read::{Memory, Outcome};
use kobo_sdk::keyboard::{Keyboard, Pressed};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Header, KoboApp, Screen, ScreenBuilder,
    ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskError, TaskId, TaskOutcome,
};
use std::collections::VecDeque;
use std::process::ExitCode;

use library::{
    decode_tags, decode_works, encode_tags, encode_works, feed_url, parse_feed, parse_tag,
    parse_work_page, place_key, shelf_name, work_id, work_url, DownloadState, FeedWork,
    FollowedTag, ParsedWork, Work, MAX_TAGS, MAX_WORKS,
};

const WORKS_KEY: &str = "works.v2";
const LEGACY_WORKS_KEY: &str = "works";
const TAGS_KEY: &str = "tags.v1";
const CHUNK: u32 = 256 * 1024;
const PAGE_BYTES: u32 = 512 * 1024;
const FEED_BYTES: u32 = 512 * 1024;
const MAX_EPUB: usize = 12 * 1024 * 1024;
const ROWS_PER_PAGE: usize = 6;
const UA: &str = "kobo-fanshelf/0.2.0 (+https://github.com/BandarLabs/Cobalt)";
const LOCKED: &str = "This work requires an AO3 login, which this app doesn't do yet";
const REMOVED: &str = "removed from the archive";
const SLOW_DOWN: &str = "The archive asked us to slow down — try in a minute";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Shelf,
    Add,
    Work,
    Adult,
    Follow,
    AddTag,
    Feed,
    Updates,
    Reading,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Request {
    Lookup { id: String, adult: bool },
    Update { work: usize, adult: bool },
    Feed { tag: usize },
    Epub { work: usize, offset: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Active {
    Spacing(Request),
    Fetching(Request),
    Backoff(Request),
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AdultPurpose {
    Lookup(String),
    Update(usize),
}

#[allow(clippy::struct_excessive_bools)]
struct Fanshelf {
    view: View,
    keyboard: Keyboard,
    works: Vec<Work>,
    tags: Vec<FollowedTag>,
    feed: Vec<FeedWork>,
    open: Option<usize>,
    open_tag: Option<usize>,
    shelf_page: usize,
    tag_page: usize,
    feed_page: usize,
    updates_page: usize,
    task: Option<(TaskId, Active)>,
    queued: VecDeque<Request>,
    sent_request: bool,
    rate_attempt: u8,
    adult: Option<AdultPurpose>,
    bytes: Vec<u8>,
    upload: Option<ShelfUpload>,
    upload_work: Option<usize>,
    open_after_upload: bool,
    loading: Option<ShelfDownload>,
    loading_work: Option<usize>,
    book: BookView,
    place: Option<Memory>,
    message: Option<String>,
    works_loaded: bool,
    tags_loaded: bool,
    #[cfg(not(target_arch = "arm"))]
    demo: bool,
}

impl Default for Fanshelf {
    fn default() -> Self {
        Self {
            view: View::Shelf,
            keyboard: Keyboard::new(),
            works: Vec::new(),
            tags: Vec::new(),
            feed: Vec::new(),
            open: None,
            open_tag: None,
            shelf_page: 0,
            tag_page: 0,
            feed_page: 0,
            updates_page: 0,
            task: None,
            queued: VecDeque::new(),
            sent_request: false,
            rate_attempt: 0,
            adult: None,
            bytes: Vec::new(),
            upload: None,
            upload_work: None,
            open_after_upload: false,
            loading: None,
            loading_work: None,
            book: BookView::new(),
            place: None,
            message: None,
            works_loaded: false,
            tags_loaded: false,
            #[cfg(not(target_arch = "arm"))]
            demo: std::env::var_os("FANSHELF_DEMO").is_some(),
        }
    }
}

fn headers() -> Vec<Header> {
    vec![
        Header::new("User-Agent", UA),
        Header::new(
            "Accept",
            "text/html,application/atom+xml;q=0.9,application/epub+zip;q=0.8",
        ),
    ]
}

fn fetch(request: &Request, works: &[Work], tags: &[FollowedTag]) -> Option<Task> {
    let (url, offset, max_bytes) = match request {
        Request::Lookup { id, adult } => (work_url(id, *adult), 0, PAGE_BYTES),
        Request::Update { work, adult } => (work_url(&works.get(*work)?.id, *adult), 0, PAGE_BYTES),
        Request::Feed { tag } => (feed_url(tags.get(*tag)?), 0, FEED_BYTES),
        Request::Epub { work, offset } => (works.get(*work)?.epub.clone(), *offset, CHUNK),
    };
    Some(Task::Fetch {
        url,
        offset,
        max_bytes,
        credential: None,
        headers: headers(),
    })
}

fn display(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_owned();
    }
    let mut end = bytes;
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}…", text[..end].trim_end())
}

impl Fanshelf {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen());
    }

    fn ready(&self) -> bool {
        self.works_loaded && self.tags_loaded
    }

    #[cfg(not(target_arch = "arm"))]
    fn demo_enabled(&self) -> bool {
        self.demo
    }

    #[cfg(target_arch = "arm")]
    fn demo_enabled(&self) -> bool {
        false
    }

    fn current(&self) -> Option<&Work> {
        self.open.and_then(|index| self.works.get(index))
    }

    fn page_bounds(page: usize, total: usize) -> (usize, usize) {
        let start = page
            .min(total.saturating_sub(1) / ROWS_PER_PAGE)
            .saturating_mul(ROWS_PER_PAGE);
        (start, (start + ROWS_PER_PAGE).min(total))
    }

    fn paged(mut screen: ScreenBuilder, page: usize, total: usize) -> ScreenBuilder {
        if total <= ROWS_PER_PAGE {
            return screen;
        }
        let pages = total.div_ceil(ROWS_PER_PAGE);
        screen = screen.secondary(format!("Page {} of {pages}", page.min(pages - 1) + 1));
        let mut actions = Vec::new();
        if page > 0 {
            actions.push(("page-prev", "Previous"));
        }
        if page + 1 < pages {
            actions.push(("page-next", "Next"));
        }
        screen.buttons(actions)
    }

    fn screen(&self) -> Screen {
        match self.view {
            View::Shelf => self.shelf_screen(),
            View::Add => ScreenBuilder::new("fs-add")
                .top_bar("Add work")
                .heading("Add an AO3 work")
                .secondary("Enter its web address or numeric work ID.")
                .keyboard(&self.keyboard, "Open work")
                .owns_back(true)
                .build(),
            View::Work => self.work_screen(),
            View::Adult => ScreenBuilder::new("fs-adult")
                .top_bar("Adult content")
                .top_bar_action("shelf", "Shelf")
                .heading("AO3 adult-content notice")
                .text("AO3 says this work may contain adult content. Fanshelf will only request the adult view if you explicitly continue.")
                .buttons([("adult-cancel", "Go back"), ("adult-confirm", "Continue")])
                .owns_back(true)
                .build(),
            View::Follow => self.follow_screen(),
            View::AddTag => ScreenBuilder::new("fs-add-tag")
                .top_bar("Follow tag")
                .heading("Follow an AO3 tag")
                .secondary("Enter a tag name or paste its AO3 tag URL. Fanshelf reads only its Atom feed.")
                .keyboard(&self.keyboard, "Follow")
                .owns_back(true)
                .build(),
            View::Feed => self.feed_screen(),
            View::Updates => self.updates_screen(),
            View::Reading => self
                .book
                .screen(self.current().map_or("Fanshelf", |work| work.title.as_str()))
                .unwrap_or_else(|| {
                    ScreenBuilder::new("fs-reader")
                        .top_bar("Fanshelf")
                        .secondary("Opening EPUB…")
                        .build()
                }),
        }
    }

    fn shelf_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("fs-shelf")
            .top_bar("Fanshelf")
            .top_bar_action("add", "Add")
            .buttons([("follow", "Followed tags"), ("updates", "Updates")]);
        if !self.ready() {
            return screen.secondary("Loading shelf…").build();
        }
        if self.works.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "Your shelf is empty",
                "Add an AO3 work, review its rating and warnings, then download it.",
            );
        } else {
            let (start, end) = Self::page_bounds(self.shelf_page, self.works.len());
            screen = screen.rows(self.works[start..end].iter().enumerate().map(
                |(offset, work)| {
                    let index = start + offset;
                    let badge = match work.download {
                        DownloadState::UpdateAvailable => " · NEW",
                        DownloadState::Removed => " · removed from the archive",
                        DownloadState::Downloaded => " · offline",
                        DownloadState::NotDownloaded => " · not downloaded",
                    };
                    (
                        format!("work-{index}"),
                        display(&work.title, 74),
                        format!(
                            "{} · {}{}",
                            display(&work.author, 42),
                            work.chapters_label(),
                            badge
                        ),
                        Glyph::Book,
                    )
                },
            ));
            screen = Self::paged(screen, self.shelf_page, self.works.len());
        }
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        screen.build()
    }

    fn work_screen(&self) -> Screen {
        let Some(work) = self.current() else {
            return ScreenBuilder::new("fs-work")
                .top_bar("Work")
                .secondary(self.message.as_deref().unwrap_or("Looking up work…"))
                .owns_back(true)
                .build();
        };
        let mut screen = ScreenBuilder::new("fs-work")
            .top_bar("Work")
            .top_bar_action("shelf", "Shelf")
            .heading(display(&work.title, 110))
            .secondary(format!(
                "{} · {}",
                display(&work.author, 60),
                display(&work.fandom, 70)
            ))
            .text(format!("Rating: {}", display(&work.rating, 90)))
            .text(format!(
                "Archive warnings: {}",
                display(&work.warnings, 180)
            ))
            .secondary(format!(
                "Chapters {} · Updated {}",
                work.chapters_label(),
                if work.updated.is_empty() {
                    "unknown"
                } else {
                    &work.updated
                }
            ));
        if work.download == DownloadState::Removed {
            screen = screen.banner(BannerLevel::Attention, REMOVED);
        } else if self.task.is_some() || self.upload.is_some() || self.loading.is_some() {
            screen = screen.banner(
                BannerLevel::Info,
                self.message.as_deref().unwrap_or("Working…"),
            );
        } else if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        let buttons = if work.download == DownloadState::UpdateAvailable {
            vec![
                ("read", "Read"),
                ("redownload", "Re-download"),
                ("check", "Check updates"),
            ]
        } else if work.downloaded() {
            vec![("read", "Read"), ("check", "Check updates")]
        } else {
            vec![("download", "Download EPUB"), ("check", "Check updates")]
        };
        for (name, label) in buttons {
            screen = screen.button(name, label);
        }
        screen.owns_back(true).build()
    }

    fn follow_screen(&self) -> Screen {
        let mut screen = ScreenBuilder::new("fs-follow")
            .top_bar("Followed AO3 tags")
            .top_bar_action("add-tag", "Add")
            .top_bar_action("shelf", "Shelf");
        if self.tags.is_empty() {
            screen = screen.splash(
                Some(Glyph::Bookmark),
                "No followed tags",
                "Follow a tag to read AO3's structured Atom feed. Fanshelf never scrapes search results.",
            );
        } else {
            let (start, end) = Self::page_bounds(self.tag_page, self.tags.len());
            screen = screen.rows(
                self.tags[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, tag)| {
                        let index = start + offset;
                        (
                            format!("tag-{index}"),
                            display(&tag.name, 90),
                            "Newest works from feeds.atom".to_owned(),
                            Glyph::Bookmark,
                        )
                    }),
            );
            screen = Self::paged(screen, self.tag_page, self.tags.len());
        }
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        screen.owns_back(true).build()
    }

    fn feed_screen(&self) -> Screen {
        let title = self
            .open_tag
            .and_then(|index| self.tags.get(index))
            .map_or("Tag feed", |tag| tag.name.as_str());
        let mut screen = ScreenBuilder::new("fs-feed")
            .top_bar(display(title, 54))
            .top_bar_action("shelf", "Shelf")
            .secondary("AO3 Atom feed · newest entries");
        if self.task.is_some() && self.feed.is_empty() {
            screen = screen.skeleton(6);
        } else if self.feed.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "No feed entries",
                self.message
                    .as_deref()
                    .unwrap_or("AO3 returned no readable Atom entries for this tag."),
            );
        } else {
            let (start, end) = Self::page_bounds(self.feed_page, self.feed.len());
            screen = screen.rows(
                self.feed[start..end]
                    .iter()
                    .enumerate()
                    .map(|(offset, work)| {
                        let index = start + offset;
                        (
                            format!("feed-{index}"),
                            display(&work.title, 84),
                            format!(
                                "{} · {}",
                                display(&work.author, 44),
                                display(&work.updated, 24)
                            ),
                            Glyph::Book,
                        )
                    }),
            );
            screen = Self::paged(screen, self.feed_page, self.feed.len());
        }
        screen.owns_back(true).build()
    }

    fn updates_screen(&self) -> Screen {
        let wips = self
            .works
            .iter()
            .enumerate()
            .filter(|(_, work)| !work.complete)
            .collect::<Vec<_>>();
        let mut screen = ScreenBuilder::new("fs-updates")
            .top_bar("Manual updates")
            .top_bar_action("check-all", "Check all")
            .top_bar_action("shelf", "Shelf")
            .secondary("Nothing runs in the background. This button is the schedule.");
        if wips.is_empty() {
            screen = screen.splash(
                Some(Glyph::Check),
                "No works in progress",
                "Completed works stay on the shelf without update polling.",
            );
        } else {
            let (start, end) = Self::page_bounds(self.updates_page, wips.len());
            screen = screen.rows(wips[start..end].iter().map(|(index, work)| {
                let state = if work.download == DownloadState::UpdateAvailable {
                    "Unread update"
                } else if work.download == DownloadState::Removed {
                    REMOVED
                } else {
                    "Up to date at last manual check"
                };
                (
                    format!("update-{index}"),
                    display(&work.title, 76),
                    format!("{} · {state}", work.chapters_label()),
                    Glyph::Clock,
                )
            }));
            screen = Self::paged(screen, self.updates_page, wips.len());
        }
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        screen.owns_back(true).build()
    }

    fn save_works(&self, context: &mut Context) {
        context.store().save(WORKS_KEY, encode_works(&self.works));
    }

    fn save_tags(&self, context: &mut Context) {
        context.store().save(TAGS_KEY, encode_tags(&self.tags));
    }

    fn enqueue(&mut self, context: &mut Context, request: Request) {
        self.queued.push_back(request);
        self.start_next(context);
    }

    fn start_next(&mut self, context: &mut Context) {
        if self.task.is_some() {
            return;
        }
        let Some(request) = self.queued.pop_front() else {
            return;
        };
        if self.sent_request {
            if let Some(task) = context.spawn(Task::Sleep { seconds: 1 }) {
                self.task = Some((task, Active::Spacing(request)));
            }
        } else {
            self.spawn_fetch(context, request);
        }
    }

    fn spawn_fetch(&mut self, context: &mut Context, request: Request) {
        let Some(work) = fetch(&request, &self.works, &self.tags) else {
            self.message = Some("That item is no longer available in this list.".into());
            self.start_next(context);
            return;
        };
        if let Some(task) = context.spawn(work) {
            self.sent_request = true;
            self.task = Some((task, Active::Fetching(request)));
        } else {
            self.message = Some("Fanshelf could not start another request yet.".into());
        }
    }

    fn backoff(&mut self, context: &mut Context, request: Request, retry_after: u32) {
        self.rate_attempt = self.rate_attempt.saturating_add(1);
        let exponential = 1_u32
            .checked_shl(u32::from(self.rate_attempt.min(6)))
            .unwrap_or(60)
            .min(60);
        let delay = retry_after.max(exponential).clamp(1, 60 * 60);
        self.message = Some(SLOW_DOWN.into());
        if let Some(task) = context.spawn(Task::Sleep { seconds: delay }) {
            self.task = Some((task, Active::Backoff(request)));
        }
    }

    fn begin_lookup(&mut self, context: &mut Context, id: String, adult: bool) {
        self.view = View::Work;
        self.message = Some("Looking up metadata…".into());
        self.open = self.works.iter().position(|work| work.id == id);
        self.enqueue(context, Request::Lookup { id, adult });
    }

    fn begin_download(&mut self, context: &mut Context, work: usize, open_after: bool) {
        self.bytes.clear();
        self.open_after_upload = open_after;
        self.message = Some("Downloading EPUB…".into());
        self.enqueue(context, Request::Epub { work, offset: 0 });
    }

    fn finish_lookup(&mut self, context: &mut Context, id: String, adult: bool, body: &[u8]) {
        let text = String::from_utf8_lossy(body);
        match parse_work_page(&id, &text) {
            ParsedWork::Work(mut incoming) => {
                incoming.adult = adult;
                if let Some(index) = self.works.iter().position(|work| work.id == id) {
                    incoming.download = self.works[index].download;
                    self.works[index] = *incoming;
                    self.open = Some(index);
                } else if self.works.len() < MAX_WORKS {
                    self.works.push(*incoming);
                    self.open = Some(self.works.len() - 1);
                } else {
                    self.message = Some("The shelf is full at 96 works.".into());
                    return;
                }
                self.message = None;
                self.save_works(context);
            }
            ParsedWork::AdultInterstitial => {
                self.adult = Some(AdultPurpose::Lookup(id));
                self.view = View::Adult;
                self.message = None;
            }
            ParsedWork::Locked => self.message = Some(LOCKED.into()),
            ParsedWork::Missing => self.message = Some(REMOVED.into()),
            ParsedWork::Malformed => {
                self.message = Some("AO3 returned a page Fanshelf could not safely parse.".into());
            }
        }
    }

    fn finish_update(&mut self, context: &mut Context, index: usize, adult: bool, body: &[u8]) {
        let Some(existing) = self.works.get(index).cloned() else {
            return;
        };
        let text = String::from_utf8_lossy(body);
        match parse_work_page(&existing.id, &text) {
            ParsedWork::Work(mut incoming) => {
                incoming.adult = adult || existing.adult;
                let changed = incoming.chapters > existing.chapters
                    || (!incoming.updated.is_empty() && incoming.updated != existing.updated);
                incoming.download = if changed && existing.downloaded() {
                    DownloadState::UpdateAvailable
                } else {
                    existing.download
                };
                self.works[index] = *incoming;
                self.message = Some(if changed {
                    "A newer version is available.".into()
                } else {
                    "No new chapters found.".into()
                });
                self.save_works(context);
            }
            ParsedWork::AdultInterstitial => {
                self.adult = Some(AdultPurpose::Update(index));
                self.view = View::Adult;
                self.message = None;
            }
            ParsedWork::Locked => self.message = Some(LOCKED.into()),
            ParsedWork::Missing => {
                if existing.downloaded() {
                    self.works[index].download = DownloadState::Removed;
                    self.save_works(context);
                }
                self.message = Some(REMOVED.into());
            }
            ParsedWork::Malformed => {
                self.message = Some("AO3 returned a page Fanshelf could not safely parse.".into());
            }
        }
    }

    fn finish_feed(&mut self, body: &[u8]) {
        let text = String::from_utf8_lossy(body);
        self.feed = parse_feed(&text);
        self.feed_page = 0;
        self.message = self
            .feed
            .is_empty()
            .then(|| "AO3 returned no readable Atom entries for this tag.".into());
    }

    fn finish_epub(&mut self, context: &mut Context, work: usize, chunk: &[u8]) {
        if self.bytes.len().saturating_add(chunk.len()) > MAX_EPUB {
            self.bytes.clear();
            self.message = Some("This EPUB is too large to keep on this reader.".into());
            return;
        }
        let done = chunk.len() < CHUNK as usize;
        self.bytes.extend_from_slice(chunk);
        if !done {
            self.enqueue(
                context,
                Request::Epub {
                    work,
                    offset: u32::try_from(self.bytes.len()).unwrap_or(u32::MAX),
                },
            );
            return;
        }
        let Some(item) = self.works.get(work) else {
            return;
        };
        let mut upload = ShelfUpload::new(shelf_name(&item.id), self.bytes.clone());
        upload.start(context);
        self.upload = Some(upload);
        self.upload_work = Some(work);
        self.message = Some("Saving EPUB atomically…".into());
    }

    fn start_read(&mut self, context: &mut Context, work: usize) {
        let Some(item) = self.works.get(work) else {
            return;
        };
        context.store().load(place_key(&item.id));
        self.place = None;
        let mut loading = ShelfDownload::new(shelf_name(&item.id)).at_most(MAX_EPUB);
        loading.start(context);
        self.loading = Some(loading);
        self.loading_work = Some(work);
        self.message = Some("Opening EPUB…".into());
    }

    fn open_bytes(&mut self, context: &mut Context, work: usize, bytes: &[u8]) {
        let Some(item) = self.works.get(work) else {
            return;
        };
        match self.book.open_bytes(
            context,
            &shelf_name(&item.id),
            bytes,
            self.place.take().unwrap_or_default(),
        ) {
            Ok(()) => {
                self.open = Some(work);
                self.view = View::Reading;
                self.message = None;
            }
            Err(_) => self.message = Some("The downloaded EPUB could not be opened.".into()),
        }
    }

    fn save_place(&mut self, context: &mut Context) {
        let Some(item) = self.current() else { return };
        let Some(memory) = self.book.memory() else {
            return;
        };
        context.store().save(place_key(&item.id), memory.encode());
    }

    fn close_book(&mut self, context: &mut Context) {
        self.save_place(context);
        self.book.close(context);
        self.view = View::Work;
    }

    #[cfg(not(target_arch = "arm"))]
    fn seed_demo(&mut self) {
        if !self.demo || !self.ready() || !self.works.is_empty() || !self.tags.is_empty() {
            return;
        }
        self.works = vec![
            Work {
                id: "9001".into(),
                title: "The Clockwork Garden".into(),
                author: "North Star".into(),
                fandom: "Public Domain Fairy Tales".into(),
                rating: "Teen And Up Audiences".into(),
                warnings: "No Archive Warnings Apply".into(),
                summary: "A synthetic demonstration work.".into(),
                chapters: 13,
                total_chapters: None,
                complete: false,
                updated: "2026-09-01".into(),
                epub: "https://archiveofourown.org/downloads/9001/demo.epub".into(),
                download: DownloadState::UpdateAvailable,
                adult: false,
            },
            Work {
                id: "9002".into(),
                title: "Лунная библиотека".into(),
                author: "Paper Crane".into(),
                fandom: "Synthetic Library Stories".into(),
                rating: "General Audiences".into(),
                warnings: "No Archive Warnings Apply".into(),
                summary: String::new(),
                chapters: 1,
                total_chapters: Some(1),
                complete: true,
                updated: "2026-08-24".into(),
                epub: "https://archiveofourown.org/downloads/9002/demo.epub".into(),
                download: DownloadState::Downloaded,
                adult: false,
            },
        ];
        self.tags = vec![
            parse_tag("Public Domain Fairy Tales").unwrap(),
            parse_tag("Synthetic Library Stories").unwrap(),
        ];
        self.seed_demo_feed();
    }

    fn seed_demo_feed(&mut self) {
        #[cfg(not(target_arch = "arm"))]
        if self.demo {
            self.feed = vec![
                FeedWork {
                    id: "9003".into(),
                    title: "A Map Made of Starlight".into(),
                    author: "Juniper Vale".into(),
                    updated: "2026-09-02".into(),
                },
                FeedWork {
                    id: "9004".into(),
                    title: "The Borrowed Compass".into(),
                    author: "Rowan Ink".into(),
                    updated: "2026-09-01".into(),
                },
            ];
        }
    }
}

impl KoboApp for Fanshelf {
    fn on_start(&mut self, context: &mut Context) {
        context.store().load(WORKS_KEY);
        context.store().load(TAGS_KEY);
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Loaded { key, value } = &result {
            if key == WORKS_KEY {
                if let Some(value) = value {
                    self.works = decode_works(value);
                    self.works_loaded = true;
                } else {
                    context.store().load(LEGACY_WORKS_KEY);
                }
            } else if key == LEGACY_WORKS_KEY {
                self.works = value.as_deref().map(decode_works).unwrap_or_default();
                if !self.works.is_empty() {
                    self.save_works(context);
                }
                self.works_loaded = true;
            } else if key == TAGS_KEY {
                self.tags = value.as_deref().map(decode_tags).unwrap_or_default();
                self.tags_loaded = true;
            } else if self
                .current()
                .is_some_and(|work| place_key(&work.id) == *key)
            {
                let memory = value
                    .as_deref()
                    .map_or_else(Memory::default, Memory::decode);
                if !self.book.restore(context, memory.clone()) {
                    self.place = Some(memory);
                }
            }
        }
        #[cfg(not(target_arch = "arm"))]
        self.seed_demo();

        if let Some(progress) = self
            .upload
            .as_mut()
            .map(|upload| upload.advance(context, &result))
        {
            match progress {
                ShelfProgress::Done => {
                    self.upload = None;
                    if let Some(work) = self.upload_work.take() {
                        if let Some(item) = self.works.get_mut(work) {
                            item.download = DownloadState::Downloaded;
                        }
                        self.save_works(context);
                        if self.open_after_upload {
                            self.start_read(context, work);
                        } else {
                            self.message = Some(
                                "Updated EPUB saved; your reading position is preserved.".into(),
                            );
                        }
                    }
                    self.bytes.clear();
                }
                ShelfProgress::Failed(_) => {
                    self.upload = None;
                    self.upload_work = None;
                    self.bytes.clear();
                    self.message = Some(
                        "The EPUB could not be saved. The previous shelf copy is unchanged.".into(),
                    );
                }
                ShelfProgress::Elsewhere | ShelfProgress::Moving { .. } => {}
            }
        }
        if let Some(progress) = self
            .loading
            .as_mut()
            .map(|loading| loading.advance(context, &result))
        {
            match progress {
                ShelfProgress::Done => {
                    let bytes = self
                        .loading
                        .take()
                        .map(ShelfDownload::take)
                        .unwrap_or_default();
                    if let Some(work) = self.loading_work.take() {
                        self.open_bytes(context, work, &bytes);
                    }
                }
                ShelfProgress::Failed(_) => {
                    self.loading = None;
                    self.loading_work = None;
                    self.message = Some("This downloaded EPUB is no longer on the reader.".into());
                }
                ShelfProgress::Elsewhere | ShelfProgress::Moving { .. } => {}
            }
        }
        self.show(context);
    }

    #[allow(clippy::too_many_lines)]
    fn on_action(&mut self, context: &mut Context, action: ActionId) {
        if self.view == View::Reading {
            if let Some(outcome) = self.book.act(context, action) {
                match outcome {
                    Outcome::Close => self.close_book(context),
                    Outcome::Save => self.save_place(context),
                    Outcome::Light(level) => context.device().set_frontlight(level),
                    Outcome::Elsewhere | Outcome::Repaint => {}
                }
                self.show(context);
                return;
            }
        }
        if matches!(self.view, View::Add | View::AddTag) {
            if let Some(Pressed::Submitted) = self.keyboard.press(action) {
                let entered = self.keyboard.take();
                if self.view == View::Add {
                    if let Some(id) = work_id(&entered) {
                        self.begin_lookup(context, id, false);
                    } else {
                        self.message = Some("Enter an AO3 work address or work number.".into());
                    }
                } else if let Some(tag) = parse_tag(&entered) {
                    if self.tags.len() >= MAX_TAGS {
                        self.message = Some("Fanshelf can follow at most 24 tags.".into());
                    } else if !self.tags.iter().any(|known| known.slug == tag.slug) {
                        self.tags.push(tag);
                        self.save_tags(context);
                        self.view = View::Follow;
                        self.message = Some("Tag followed. Open it to fetch its Atom feed.".into());
                    }
                } else {
                    self.message = Some("Enter an AO3 tag name or tag URL.".into());
                }
            }
            self.show(context);
            return;
        }

        if action == action_id("add") {
            self.keyboard.clear();
            self.view = View::Add;
            self.message = None;
        } else if action == action_id("follow") {
            self.view = View::Follow;
            self.tag_page = 0;
            self.message = None;
        } else if action == action_id("updates") {
            self.view = View::Updates;
            self.updates_page = 0;
            self.message = None;
        } else if action == action_id("shelf") {
            self.view = View::Shelf;
            self.shelf_page = 0;
            self.message = None;
        } else if action == action_id("add-tag") {
            self.keyboard.clear();
            self.view = View::AddTag;
            self.message = None;
        } else if action == action_id("adult-confirm") {
            if let Some(purpose) = self.adult.take() {
                match purpose {
                    AdultPurpose::Lookup(id) => self.begin_lookup(context, id, true),
                    AdultPurpose::Update(work) => {
                        self.view = View::Work;
                        self.enqueue(context, Request::Update { work, adult: true });
                    }
                }
            }
        } else if action == action_id("adult-cancel") {
            self.adult = None;
            self.view = View::Shelf;
            self.message = Some("Adult view was not requested.".into());
        } else if let Some(index) =
            (0..self.works.len()).find(|index| action == action_id(&format!("work-{index}")))
        {
            self.open = Some(index);
            self.view = View::Work;
            self.message = None;
        } else if let Some(index) =
            (0..self.tags.len()).find(|index| action == action_id(&format!("tag-{index}")))
        {
            self.open_tag = Some(index);
            self.feed.clear();
            self.feed_page = 0;
            self.view = View::Feed;
            self.message = None;
            if self.demo_enabled() {
                self.seed_demo_feed();
            } else {
                self.enqueue(context, Request::Feed { tag: index });
            }
        } else if let Some(index) =
            (0..self.feed.len()).find(|index| action == action_id(&format!("feed-{index}")))
        {
            if let Some(item) = self.feed.get(index) {
                self.begin_lookup(context, item.id.clone(), false);
            }
        } else if let Some(index) =
            (0..self.works.len()).find(|index| action == action_id(&format!("update-{index}")))
        {
            self.open = Some(index);
            self.view = View::Work;
            let adult = self.works[index].adult;
            self.enqueue(context, Request::Update { work: index, adult });
        } else if action == action_id("check-all") {
            let requests = self
                .works
                .iter()
                .enumerate()
                .filter(|(_, work)| !work.complete)
                .map(|(work, item)| Request::Update {
                    work,
                    adult: item.adult,
                })
                .collect::<Vec<_>>();
            self.message = Some(format!(
                "Checking {} works, one request at a time…",
                requests.len()
            ));
            for request in requests {
                self.enqueue(context, request);
            }
        } else if action == action_id("page-prev") {
            match self.view {
                View::Shelf => self.shelf_page = self.shelf_page.saturating_sub(1),
                View::Follow => self.tag_page = self.tag_page.saturating_sub(1),
                View::Feed => self.feed_page = self.feed_page.saturating_sub(1),
                View::Updates => self.updates_page = self.updates_page.saturating_sub(1),
                _ => {}
            }
        } else if action == action_id("page-next") {
            match self.view {
                View::Shelf => self.shelf_page = self.shelf_page.saturating_add(1),
                View::Follow => self.tag_page = self.tag_page.saturating_add(1),
                View::Feed => self.feed_page = self.feed_page.saturating_add(1),
                View::Updates => self.updates_page = self.updates_page.saturating_add(1),
                _ => {}
            }
        } else if action == action_id("download") {
            if let Some(work) = self.open {
                self.begin_download(context, work, true);
            }
        } else if action == action_id("redownload") {
            if let Some(work) = self.open {
                self.begin_download(context, work, false);
            }
        } else if action == action_id("read") {
            if let Some(work) = self.open {
                self.start_read(context, work);
            }
        } else if action == action_id("check") {
            if let Some(work) = self.open {
                let adult = self.works[work].adult;
                self.message = Some("Checking AO3 now…".into());
                self.enqueue(context, Request::Update { work, adult });
            }
        } else if action == ActionId::BACK {
            match self.view {
                View::Work | View::Follow | View::Updates | View::Add | View::Adult => {
                    self.view = View::Shelf;
                }
                View::Feed | View::AddTag => self.view = View::Follow,
                View::Reading => self.close_book(context),
                View::Shelf => {}
            }
            self.message = None;
        }
        self.show(context);
    }

    fn on_task(&mut self, context: &mut Context, id: TaskId, outcome: TaskOutcome) {
        if self.book.woke(context, id, &outcome) != Step::Elsewhere {
            self.show(context);
            return;
        }
        let Some((_, active)) = self.task.take_if(|(known, _)| *known == id) else {
            return;
        };
        match active {
            Active::Spacing(request) | Active::Backoff(request) => {
                if matches!(outcome, TaskOutcome::Completed(_)) {
                    self.spawn_fetch(context, request);
                }
            }
            Active::Fetching(request) => match outcome {
                TaskOutcome::Completed(body) => {
                    self.rate_attempt = 0;
                    match request {
                        Request::Lookup { id, adult } => {
                            self.finish_lookup(context, id, adult, &body);
                        }
                        Request::Update { work, adult } => {
                            self.finish_update(context, work, adult, &body);
                        }
                        Request::Feed { .. } => self.finish_feed(&body),
                        Request::Epub { work, .. } => self.finish_epub(context, work, &body),
                    }
                    self.start_next(context);
                }
                TaskOutcome::Failed(TaskError::RateLimited(seconds)) => {
                    self.backoff(context, request, seconds);
                }
                TaskOutcome::Failed(TaskError::Unauthorized) => {
                    self.message = Some(LOCKED.into());
                    self.start_next(context);
                }
                TaskOutcome::Failed(TaskError::NotFound) => {
                    if let Request::Update { work, .. } = request {
                        if self.works.get(work).is_some_and(Work::downloaded) {
                            self.works[work].download = DownloadState::Removed;
                            self.save_works(context);
                        }
                    }
                    self.message = Some(REMOVED.into());
                    self.start_next(context);
                }
                TaskOutcome::Failed(error) => {
                    self.message = Some(match error {
                        TaskError::Offline => "Join Wi-Fi, then try again.".into(),
                        TaskError::TooLarge => {
                            "AO3 returned more data than Fanshelf's safety limit.".into()
                        }
                        _ => "AO3 did not answer this request.".into(),
                    });
                    self.start_next(context);
                }
                TaskOutcome::Cancelled => self.start_next(context),
            },
        }
        self.show(context);
    }

    fn on_exit(&mut self, context: &mut Context) {
        self.save_place(context);
    }
}

fn main() -> ExitCode {
    match kobo_sdk::run("fanshelf", Fanshelf::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fanshelf: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{AppRunner, Command, StoreRequest};
    use kobo_ui::{Chrome, Node, CLARA_BW_METRICS};

    fn work() -> Work {
        Work {
            id: "42".into(),
            title: "A safe synthetic work".into(),
            author: "Example Author".into(),
            fandom: "Public Domain Tales".into(),
            rating: "Mature".into(),
            warnings: "Graphic Depictions Of Violence".into(),
            summary: String::new(),
            chapters: 40,
            total_chapters: None,
            complete: false,
            updated: "2026-09-01".into(),
            epub: "https://archiveofourown.org/downloads/42/work.epub".into(),
            download: DownloadState::NotDownloaded,
            adult: false,
        }
    }

    fn node_label(node: &Node) -> Option<&str> {
        match node {
            Node::Heading { text, .. }
            | Node::Text { text, .. }
            | Node::Secondary { text, .. }
            | Node::Button { label: text, .. } => Some(text),
            _ => None,
        }
    }

    #[test]
    fn rating_and_warnings_precede_download_action() {
        let app = Fanshelf {
            works: vec![work()],
            open: Some(0),
            view: View::Work,
            ..Fanshelf::default()
        };
        let screen = app.screen();
        let labels = screen
            .nodes
            .iter()
            .filter_map(node_label)
            .collect::<Vec<_>>();
        let rating = labels
            .iter()
            .position(|text| text.starts_with("Rating:"))
            .unwrap();
        let warnings = labels
            .iter()
            .position(|text| text.starts_with("Archive warnings:"))
            .unwrap();
        let download = labels
            .iter()
            .position(|text| *text == "Download EPUB")
            .unwrap();
        assert!(rating < download && warnings < download);
    }

    #[test]
    fn every_network_request_has_the_distinct_user_agent() {
        let task = fetch(
            &Request::Lookup {
                id: "42".into(),
                adult: false,
            },
            &[],
            &[],
        )
        .unwrap();
        let Task::Fetch { headers, .. } = task else {
            panic!("not a fetch");
        };
        assert!(headers
            .iter()
            .any(|header| header.name == "User-Agent" && header.value == UA));
    }

    #[test]
    fn a_missing_v2_shelf_loads_and_republishes_the_original_shelf() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        let commands = runner.store_result(StoreResult::Loaded {
            key: WORKS_KEY.into(),
            value: None,
        });
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Load { key }) if key == LEGACY_WORKS_KEY
        )));
        let commands = runner.store_result(StoreResult::Loaded {
            key: LEGACY_WORKS_KEY.into(),
            value: Some(b"123|Saved work|work-123.epub".to_vec()),
        });
        assert_eq!(runner.app().works[0].id, "123");
        assert!(runner.app().works_loaded);
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Store(StoreRequest::Save { key, .. }) if key == WORKS_KEY
        )));
    }

    #[test]
    fn a_second_ao3_request_waits_one_second_and_never_overlaps() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        runner.app_mut().works = vec![work()];
        runner.app_mut().works_loaded = true;
        runner.app_mut().tags_loaded = true;
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Work;
        let commands = runner.action(action_id("check"));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: Task::Fetch { .. },
                ..
            }
        )));
        let first = runner.app().task.as_ref().unwrap().0;
        runner.app_mut().queued.push_back(Request::Feed { tag: 0 });
        let commands = runner.task_outcome(first, TaskOutcome::Completed(Vec::new()));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: Task::Sleep { seconds: 1 },
                ..
            }
        )));
        assert_eq!(
            runner.app().task.as_ref().map(|(_, active)| active),
            Some(&Active::Spacing(Request::Feed { tag: 0 }))
        );
    }

    #[test]
    fn retry_after_drives_conservative_backoff() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        runner.app_mut().works = vec![work()];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Work;
        let commands = runner.action(action_id("check"));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: Task::Fetch { .. },
                ..
            }
        )));
        let task = runner.app().task.as_ref().unwrap().0;
        let commands = runner.task_outcome(task, TaskOutcome::Failed(TaskError::RateLimited(17)));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: Task::Sleep { seconds: 17 },
                ..
            }
        )));
        assert_eq!(runner.app().message.as_deref(), Some(SLOW_DOWN));
    }

    #[test]
    fn adult_view_is_only_added_after_confirmation() {
        assert!(!work_url("42", false).contains("view_adult"));
        assert!(work_url("42", true).ends_with("?view_adult=true"));
        let app = Fanshelf {
            view: View::Adult,
            adult: Some(AdultPurpose::Lookup("42".into())),
            ..Fanshelf::default()
        };
        assert!(app.task.is_none(), "the interstitial started a request");
        let mut runner = AppRunner::new(app);
        let commands = runner.action(action_id("adult-confirm"));
        assert!(commands.iter().any(|command| matches!(
            command,
            Command::Spawn {
                work: Task::Fetch { url, .. },
                ..
            } if url.ends_with("?view_adult=true")
        )));
    }

    #[test]
    fn exact_locked_and_removed_messages_are_stable() {
        assert_eq!(
            LOCKED,
            "This work requires an AO3 login, which this app doesn't do yet"
        );
        assert_eq!(REMOVED, "removed from the archive");
    }

    #[test]
    fn bounded_catalogues_remain_reachable_through_paging() {
        let works = (0..7)
            .map(|index| Work {
                title: format!("Work {index}"),
                id: (100 + index).to_string(),
                ..work()
            })
            .collect();
        let app = Fanshelf {
            works,
            works_loaded: true,
            tags_loaded: true,
            view: View::Shelf,
            ..Fanshelf::default()
        };
        let mut runner = AppRunner::new(app);
        runner.action(action_id("page-next"));
        let titles = runner
            .app()
            .screen()
            .nodes
            .into_iter()
            .flat_map(|node| match node {
                Node::Rows { rows, .. } => {
                    rows.into_iter().map(|row| row.title).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(titles, ["Work 6"]);
    }

    #[test]
    fn primary_screens_fit_the_clara_panel() {
        let mut app = Fanshelf {
            works_loaded: true,
            tags_loaded: true,
            works: vec![work()],
            tags: vec![parse_tag("Public Domain Fairy Tales").unwrap()],
            feed: vec![FeedWork {
                id: "99".into(),
                title: "Feed work".into(),
                author: "Author".into(),
                updated: "2026-09-02".into(),
            }],
            ..Fanshelf::default()
        };
        for view in [
            View::Shelf,
            View::Work,
            View::Adult,
            View::Follow,
            View::Feed,
            View::Updates,
        ] {
            app.view = view;
            app.open = Some(0);
            app.open_tag = Some(0);
            assert!(
                app.screen()
                    .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                    .issues
                    .is_empty(),
                "{view:?} does not fit"
            );
        }
    }
}
