//! A tap-driven, offline-after-download reader for public AO3 works.
#[cfg(test)]
mod collection_tests;
mod library;

use kobo_bookview::{BookView, Step};
use kobo_read::{Memory, Outcome};
use kobo_sdk::keyboard::{TextEntry, Typing};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Glyph, Header, KoboApp, QuoteRole, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskError, TaskId,
    TaskOutcome,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::process::ExitCode;

use library::{
    decode_reading, decode_tags, decode_works, encode_reading, encode_tags, encode_works, feed_url,
    parse_feed, parse_tag, parse_work_page, place_key, shelf_name, work_id, work_url,
    DownloadState, FeedWork, FollowedTag, ParsedWork, Work, MAX_TAGS, MAX_WORKS,
};

const WORKS_KEY: &str = "works.v2";
const READING_KEY: &str = "reading.v1";
/// Works whose new chapters arrived and have not been opened since.
const FRESH_KEY: &str = "fresh.v1";
const LEGACY_WORKS_KEY: &str = "works";
const TAGS_KEY: &str = "tags.v1";
const CHUNK: u32 = 256 * 1024;
const PAGE_BYTES: u32 = 512 * 1024;
const FEED_BYTES: u32 = 512 * 1024;
const MAX_EPUB: usize = 12 * 1024 * 1024;
const UA: &str = "kobo-fanshelf/0.2.0 (+https://github.com/BandarLabs/Cobalt)";
const LOCKED: &str =
    "Locked to AO3 members. Fanshelf has no account sign-in, so it cannot download this work.";
const REMOVED: &str = "removed from the archive";
const SLOW_DOWN: &str = "AO3 asked Fanshelf to slow down. It waits, then carries on by itself.";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Shelf,
    Add,
    Work,
    Adult,
    Follow,
    AddTag,
    Fandoms,
    Manage,
    Feed,
    Updates,
    Reading,
    /// A work's whole summary, for a panel and text size where the work's
    /// own page could only show the start of it.
    Summary,
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

/// One pass of Update all, counted so it can end on a single sentence.
///
/// Checking and downloading used to live on two screens: Updates found new
/// chapters and Manage fetched them, so an owner who checked and then took
/// the reader onto a train had a list of works marked new and none of the
/// chapters. A pass now downloads what it finds, and the message at the end
/// says what is on the reader rather than what could have been.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Sweep {
    /// Works still waiting for their check.
    pending: BTreeSet<usize>,
    checked: usize,
    failed: usize,
    /// Works with new chapters this pass is downloading.
    found: BTreeSet<usize>,
    saved: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum AdultPurpose {
    Lookup(String),
    Update(usize),
}

#[allow(clippy::struct_excessive_bools)]
struct Fanshelf {
    view: View,
    /// A work's address or number, typed on the runtime's keyboard with the
    /// marks an address needs beside the letters.
    work_entry: TextEntry,
    /// A tag's name or address. Tag names have spaces in them, so this one
    /// keeps the space bar.
    tag_entry: TextEntry,
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
    /// Works waiting for their EPUB, in the order they were asked for.
    ///
    /// One at a time, because the chunks of one download are gathered in
    /// `bytes` and a second download started beside it would splice its
    /// chapters into the first.
    downloads: VecDeque<(usize, bool)>,
    /// The work whose EPUB is being fetched or written, until it is saved.
    downloading: Option<usize>,
    sweep: Option<Sweep>,
    /// What the shelf says each stored file weighs, once it has said.
    shelf_sizes: Option<BTreeMap<String, u32>>,
    upload: Option<ShelfUpload>,
    upload_work: Option<usize>,
    open_after_upload: bool,
    loading: Option<ShelfDownload>,
    loading_work: Option<usize>,
    book: BookView,
    place: Option<Memory>,
    message: Option<String>,
    reading: std::collections::BTreeSet<String>,
    /// Works downloaded with new chapters nobody has opened yet.
    ///
    /// Kept apart from the download state because the download now follows
    /// the check by itself: without this, the mark that told an owner which
    /// works had moved on would be gone before they ever saw it.
    fresh: BTreeSet<String>,
    filter: Option<String>,
    fandom_page: usize,
    confirm_remove: bool,
    /// Whether the work page's menu is open.
    work_menu: bool,
    /// Which page of the whole summary is showing.
    summary_page: usize,
    works_loaded: bool,
    tags_loaded: bool,
    #[cfg(not(target_arch = "arm"))]
    demo: bool,
}

impl Default for Fanshelf {
    fn default() -> Self {
        Self {
            view: View::Shelf,
            work_entry: TextEntry::new().address(),
            tag_entry: TextEntry::new(),
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
            downloads: VecDeque::new(),
            downloading: None,
            sweep: None,
            shelf_sizes: None,
            upload: None,
            upload_work: None,
            open_after_upload: false,
            loading: None,
            loading_work: None,
            book: BookView::new(),
            place: None,
            message: None,
            reading: std::collections::BTreeSet::new(),
            fresh: BTreeSet::new(),
            filter: None,
            fandom_page: 0,
            confirm_remove: false,
            work_menu: false,
            summary_page: 0,
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

/// "1 work" or "3 works".
fn plural(count: usize) -> String {
    if count == 1 {
        "1 work".to_owned()
    } else {
        format!("{count} works")
    }
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

/// The reader's clock, on the reader's own day.
fn reader_clock() -> Box<dyn kobo_sdk::clock::Clock> {
    Box::new(kobo_sdk::clock::SystemClock::reader())
}

fn now_seconds() -> u64 {
    reader_clock()
        .now()
        .map_or(0, |snapshot| snapshot.unix_millis / 1000)
}

/// A short month-day stamp for update-check lines; the year is omitted
/// because checks are always recent.
fn format_checked(epoch: u64) -> String {
    let Some(date) = reader_clock().now().ok().and_then(|reader| {
        kobo_sdk::clock::Snapshot {
            unix_millis: epoch.checked_mul(1000)?,
            monotonic_millis: 0,
            utc_offset_minutes: reader.utc_offset_minutes,
        }
        .date()
    }) else {
        return "unknown".to_owned();
    };
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][usize::from(date.month.saturating_sub(1)) % 12];
    format!("{month} {}", date.day)
}

/// One line naming the update state: never checked, current as of the last
/// check, new chapters waiting, or new chapters downloaded and not yet opened.
///
/// `fresh` is the last of those. Without it, a check that had just downloaded
/// new chapters read "No new chapters as of" under a banner announcing them,
/// because the copy on the reader was, by then, current.
fn update_label(work: &Work, fresh: bool) -> String {
    if work.download == DownloadState::UpdateAvailable {
        format!("New chapters found {}", format_checked(work.last_checked))
    } else if work.last_checked == 0 {
        "Updates never checked".to_owned()
    } else if fresh {
        format!(
            "New chapters downloaded {}",
            format_checked(work.last_checked)
        )
    } else {
        format!(
            "No new chapters as of {}",
            format_checked(work.last_checked)
        )
    }
}

impl Fanshelf {
    fn show(&self, context: &mut Context) {
        context.set_screen(self.screen(context));
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

    /// Measure exactly the rows and prefix we draw, then keep page controls in
    /// the reserved bottom band rather than after a potentially full list.
    fn paged(
        context: &Context,
        mut screen: ScreenBuilder,
        page: usize,
        mut rows: Vec<(String, String, String, Glyph)>,
    ) -> ScreenBuilder {
        for (_, title, _, _) in &mut rows {
            *title = context.clamped_row(title, 2, true);
        }
        let borrowed = rows
            .iter()
            .map(|(_, title, summary, _)| (title.as_str(), summary.as_str()))
            .collect::<Vec<_>>();
        let prefix = screen.clone().build();
        let single =
            context.paginate_rows_under(&borrowed, false, kobo_sdk::Position::Elsewhere, &prefix);
        if single.len() <= 1 {
            return screen.rows(rows);
        }
        let pages =
            context.paginate_rows_under(&borrowed, true, kobo_sdk::Position::AtTheFoot, &prefix);
        let page = page.min(pages.len().saturating_sub(1));
        screen = screen.rows(pages[page].iter().map(|&index| rows[index].clone()));
        screen
            .page_turns("page-prev", "page-next")
            .page_position(
                u16::try_from(page + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            )
            .action_bar([("page-prev", "Previous"), ("page-next", "Next")])
    }

    fn screen(&self, context: &Context) -> Screen {
        match self.view {
            View::Shelf => self.shelf_screen(context),
            View::Add => self.entry_screen(
                ScreenBuilder::new("fs-add").top_bar("Add work"),
                &self.work_entry,
                "Add an AO3 work",
                "Work address or number, such as 4242",
                "Open work",
            ),
            View::Work => self.work_screen(context),
            View::Summary => self.summary_screen(context),
            View::Adult => ScreenBuilder::new("fs-adult")
                .top_bar("Adult content")
                .top_bar_action("shelf", "Shelf")
                .heading("AO3 adult-content notice")
                .text("AO3 says this work may contain adult content. Fanshelf will only request the adult view if you explicitly continue.")
                .buttons([("adult-cancel", "Go back"), ("adult-confirm", "Continue")])
                .owns_back(true)
                .build(),
            View::Follow => self.follow_screen(context),
            View::Fandoms => self.fandoms_screen(context),
            View::Manage => self.manage_screen(context),
            View::AddTag => self.entry_screen(
                ScreenBuilder::new("fs-add-tag").top_bar("Follow tag"),
                &self.tag_entry,
                "Follow an AO3 tag",
                "Tag name or tag address",
                "Follow",
            ),
            View::Feed => self.feed_screen(context),
            View::Updates => self.updates_screen(context),
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

    /// A screen that takes typed text on the reader's own keyboard, and says
    /// why the last attempt was refused.
    ///
    /// The reason takes the place of the guidance under the prompt. The
    /// runtime's field covers this application's screen, so a banner drawn
    /// here would sit under the keys unseen, and a mistyped address would
    /// come back looking like a button that did nothing.
    fn entry_screen(
        &self,
        screen: ScreenBuilder,
        entry: &TextEntry,
        prompt: &str,
        hint: &str,
        submit: &str,
    ) -> Screen {
        let hint = self.message.as_deref().unwrap_or(hint);
        screen
            .owns_back(true)
            .text_entry_with_hint(entry, prompt, hint, submit)
            .build()
    }

    /// What was typed on the Add or Follow screen, taken at its word.
    ///
    /// Text that cannot be read is put back where it was typed, so a single
    /// wrong letter is one correction rather than the whole address again.
    fn submitted(&mut self, context: &mut Context, entered: String) {
        if self.view == View::Add {
            if let Some(id) = work_id(&entered) {
                self.begin_lookup(context, id, false);
            } else {
                self.message = Some(
                    "That is not an AO3 work address or number. Try the number in the work's address, such as 4242."
                        .into(),
                );
                self.work_entry.open_with(entered);
            }
            return;
        }
        let Some(tag) = parse_tag(&entered) else {
            self.message = Some("Type a tag name, or paste the tag's AO3 address.".into());
            self.tag_entry.open_with(entered);
            return;
        };
        if self.tags.iter().any(|known| known.slug == tag.slug) {
            self.view = View::Follow;
            self.message = Some("You already follow this tag.".into());
        } else if self.tags.len() >= MAX_TAGS {
            self.message = Some(format!("Fanshelf can follow {MAX_TAGS} tags at most."));
            self.tag_entry.open_with(entered);
        } else {
            self.tags.push(tag);
            self.save_tags(context);
            self.view = View::Follow;
            self.message = Some("Tag followed. Open it to see its newest works.".into());
        }
    }

    fn badge(
        work: &Work,
        reading: &std::collections::BTreeSet<String>,
        fresh: &BTreeSet<String>,
    ) -> &'static str {
        match work.download {
            DownloadState::UpdateAvailable => " · update waiting",
            DownloadState::Downloaded if fresh.contains(&work.id) => " · new chapters",
            DownloadState::Removed => " · removed from the archive",
            _ if !work.complete && work.last_checked == 0 => " · updates unchecked",
            DownloadState::Downloaded if reading.contains(&work.id) => " · reading",
            DownloadState::Downloaded => " · offline",
            DownloadState::NotDownloaded => " · not downloaded",
        }
    }

    /// Distinct fandoms on the shelf, each with its work count.
    fn fandoms(&self) -> Vec<(String, usize)> {
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for work in &self.works {
            *counts.entry(work.fandom.clone()).or_default() += 1;
        }
        counts.into_iter().collect()
    }

    /// Shelf indexes in display order, narrowed to the chosen fandom.
    fn visible(&self) -> Vec<usize> {
        self.works
            .iter()
            .enumerate()
            .filter(|(_, work)| self.filter.as_ref().is_none_or(|f| *f == work.fandom))
            .map(|(index, _)| index)
            .collect()
    }

    fn fandoms_screen(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("fs-fandoms")
            .top_bar("Fandoms")
            .top_bar_action("shelf", "Shelf");
        let fandoms = self.fandoms();
        if fandoms.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "No works on the shelf",
                "Fandoms appear once works are added.",
            );
        } else {
            let rows = fandoms
                .iter()
                .enumerate()
                .map(|(index, (fandom, count))| {
                    (
                        format!("fandom-{index}"),
                        display(fandom, 74),
                        format!("{count} work{}", if *count == 1 { "" } else { "s" }),
                        Glyph::Book,
                    )
                })
                .collect();
            screen = Self::paged(context, screen, self.fandom_page, rows);
        }
        screen.owns_back(true).build()
    }

    fn shelf_screen(&self, context: &Context) -> Screen {
        let title = self.filter.clone().unwrap_or_else(|| "Fanshelf".to_owned());
        let mut screen = ScreenBuilder::new("fs-shelf")
            .top_bar(display(&title, 60))
            .top_bar_action("add", "Add");
        screen = if self.filter.is_some() {
            screen.top_bar_action("all", "All")
        } else {
            screen.top_bar_action("filter", "Filter")
        };
        let compact = screen.clone().buttons([
            ("follow", "Followed tags"),
            ("updates", "Updates"),
            ("manage", "Manage"),
        ]);
        screen = if compact
            .clone()
            .build()
            .layout_for(&context.metrics())
            .nodes
            .iter()
            .any(|node| node.text_lines.len() > 1)
        {
            // At larger type sizes three equal columns break even short
            // labels mid-word. Measure this prefix too before placing rows.
            screen
                .button("follow", "Followed tags")
                .buttons([("updates", "Updates"), ("manage", "Manage")])
        } else {
            compact
        };
        if !self.ready() {
            return screen.secondary("Loading shelf…").build();
        }
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        if let Some(offline) = self.offline_line().filter(|_| !self.works.is_empty()) {
            screen = screen.secondary(offline);
        }
        if self.works.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "Your shelf is empty",
                "Add an AO3 work, review its rating and warnings, then download it.",
            );
        } else {
            let visible = self.visible();
            if visible.is_empty() {
                screen = screen.splash(
                    Some(Glyph::Book),
                    "No works in this fandom",
                    "All shows the whole shelf.",
                );
            } else {
                let rows = visible
                    .iter()
                    .map(|index| {
                        let work = &self.works[*index];
                        let badge = Self::badge(work, &self.reading, &self.fresh);
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
                    })
                    .collect();
                screen = Self::paged(context, screen, self.shelf_page, rows);
            }
        }
        screen.build()
    }

    fn updates_waiting(&self) -> usize {
        self.works
            .iter()
            .filter(|work| work.download == DownloadState::UpdateAvailable)
            .count()
    }

    fn downloaded_count(&self) -> usize {
        self.works
            .iter()
            .filter(|work| {
                matches!(
                    work.download,
                    DownloadState::Downloaded | DownloadState::UpdateAvailable
                )
            })
            .count()
    }

    /// Works with a copy on this reader, and what those copies weigh.
    ///
    /// A work AO3 has since removed still counts: its copy is the one place
    /// it can still be read, which is the best reason there is to keep it.
    fn offline(&self) -> (usize, u64) {
        let sizes = self.shelf_sizes.as_ref();
        self.works
            .iter()
            .filter(|work| work.downloaded())
            .fold((0, 0), |(count, bytes), work| {
                let size = sizes
                    .and_then(|sizes| sizes.get(&shelf_name(&work.id)))
                    .copied()
                    .unwrap_or(0);
                (count + 1, bytes + u64::from(size))
            })
    }

    /// The home screen's line about what opens with the radio off.
    ///
    /// Held back until the shelf has said what its files weigh, so the line
    /// never claims a size it has not measured.
    fn offline_line(&self) -> Option<String> {
        self.shelf_sizes.as_ref()?;
        let (count, bytes) = self.offline();
        Some(kobo_bookview::offline::ready_offline(
            count, bytes, "work", "works",
        ))
    }

    fn manage_screen(&self, context: &Context) -> Screen {
        let downloaded = self.downloaded_count();
        let mut screen = ScreenBuilder::new("fs-manage")
            .top_bar("Manage shelf")
            .top_bar_action("shelf", "Shelf");
        if self.confirm_remove {
            return screen
                .splash(
                    Some(Glyph::Book),
                    format!("Remove {downloaded} downloaded copies?"),
                    "Works stay on the shelf and can be downloaded again. Reading places are kept.",
                )
                .buttons([("manage-cancel", "Go back"), ("remove-copies", "Remove")])
                .owns_back(true)
                .build();
        }
        if self.works.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "Nothing to manage",
                "Downloaded copies appear here once works are added.",
            );
        } else {
            let (_, bytes) = self.offline();
            let detail = if self.shelf_sizes.is_some() && downloaded > 0 {
                format!(
                    "{downloaded} on the reader, {}",
                    kobo_bookview::offline::size(bytes)
                )
            } else {
                format!("{downloaded} on the reader")
            };
            screen = screen.rows([(
                "manage-remove".to_owned(),
                context.clamped_row("Remove downloaded copies", 2, false),
                detail,
                Glyph::Book,
            )]);
        }
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        screen.owns_back(true).build()
    }

    /// Whether a screen draws whole on this panel at its text size.
    fn fits(context: &Context, screen: &Screen) -> bool {
        screen
            .diagnostics(&context.metrics(), &kobo_sdk::Chrome::measuring(true))
            .issues
            .iter()
            .all(|issue| {
                !matches!(
                    issue.kind,
                    kobo_sdk::LayoutIssueKind::ContentOverflow { .. }
                        | kobo_sdk::LayoutIssueKind::TextOverflow
                        | kobo_sdk::LayoutIssueKind::Clipped
                        | kobo_sdk::LayoutIssueKind::InteractiveOffscreen
                )
            })
    }

    /// The work's page: what it is, what AO3 says about it, and what to do.
    ///
    /// The summary is the author's own pitch and the thing most owners read
    /// before deciding to download, so it sits under the byline. It is the
    /// longest field here, so it is the one shortened when the page would
    /// otherwise run off the panel at a large text size, and it is dropped
    /// before the rating or the warnings ever are. Whenever it is cut, the
    /// menu offers the whole of it on a page of its own, so nothing the
    /// author wrote is out of reach at any size.
    fn work_screen(&self, context: &Context) -> Screen {
        let Some(work) = self.current() else {
            return ScreenBuilder::new("fs-work")
                .top_bar("Work")
                .secondary(self.message.as_deref().unwrap_or("Looking up work…"))
                .owns_back(true)
                .build();
        };
        let summary = work.summary.trim();
        let full_title = display(&work.title, 110);
        let mut budget = summary.len();
        let mut title_budget = full_title.len();
        loop {
            let shown = display(summary, budget);
            let title = display(&full_title, title_budget);
            let cut = shown != summary || title != full_title;
            let screen = self.work_screen_with(work, &title, &shown, cut);
            if Self::fits(context, &screen) || (budget == 0 && title_budget <= 24) {
                return screen;
            }
            // Down to the first few words before giving the line up, so the
            // page says what the work is about wherever a line is left. Only
            // a title too long for the panel at this size is cut after that,
            // and the whole of it heads the summary pages.
            if budget > 80 {
                budget = budget * 3 / 4;
            } else if budget > 30 {
                budget -= 25;
            } else if budget > 0 {
                budget = 0;
            } else {
                title_budget = (title_budget * 3 / 4).max(24);
            }
        }
    }

    fn work_screen_with(&self, work: &Work, title: &str, summary: &str, cut: bool) -> Screen {
        // Checking is a step an owner takes now and then, and the Updates tab
        // checks every work at once, so it waits in the menu. On the page it
        // was a third full-width button, and at the largest text sizes that
        // was the room the summary and the warnings needed.
        let mut menu = vec![("check", "Check for new chapters")];
        if cut {
            menu.push(("summary", "Whole summary"));
        }
        let mut screen = ScreenBuilder::new("fs-work")
            .top_bar("Work")
            .top_bar_overflow("more", self.work_menu, menu)
            .top_bar_action("shelf", "Shelf")
            .heading(title)
            .secondary(format!(
                "{} · {}",
                display(&work.author, 60),
                display(&work.fandom, 70)
            ));
        if !summary.is_empty() && summary != "…" {
            screen = screen.text(summary);
        }
        screen = screen
            .text(format!("Rating: {}", display(&work.rating, 90)))
            .text(format!("Warnings: {}", display(&work.warnings, 180)))
            .secondary(format!(
                "Chapters {} · Updated {} · {}",
                work.chapters_label(),
                if work.updated.is_empty() {
                    "unknown"
                } else {
                    &work.updated
                },
                update_label(work, self.fresh.contains(&work.id))
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
        // Pinned to the foot of the panel, where they stay whatever the page
        // above them holds: stacked under the facts they were the first thing
        // a long title or a large text size pushed off the panel.
        screen = if work.download == DownloadState::UpdateAvailable {
            screen.action_bar([("read", "Read"), ("redownload", "Download new chapters")])
        } else if work.downloaded() {
            screen.bottom_action("read", "Read")
        } else {
            screen.bottom_action("download", "Download EPUB")
        };
        screen.owns_back(true).build()
    }

    /// The whole summary, a page at a time, under the work's whole title.
    fn summary_screen(&self, context: &Context) -> Screen {
        let pages = self.summary_pages(context);
        let page = self.summary_page.min(pages.len().saturating_sub(1));
        let mut screen = self.summary_head(page == 0);
        for (_, depth, role, paragraph) in pages.get(page).into_iter().flatten() {
            screen = match role {
                QuoteRole::Body => screen.quote(*depth, paragraph),
                QuoteRole::Byline => screen.byline(*depth, paragraph),
            };
        }
        if pages.len() > 1 {
            screen = screen
                .page_turns("summary-previous", "summary-next")
                .page_position(
                    u16::try_from(page + 1).unwrap_or(u16::MAX),
                    u16::try_from(pages.len()).unwrap_or(u16::MAX),
                );
        }
        screen.build()
    }

    /// What sits above the summary: the top bar, and on the first page the
    /// work's title.
    fn summary_head(&self, first: bool) -> ScreenBuilder {
        let screen = ScreenBuilder::new("fs-summary")
            .top_bar("Summary")
            .owns_back(true);
        match self.current() {
            Some(work) if first => screen.heading(display(&work.title, 110)),
            _ => screen,
        }
    }

    fn summary_pages(&self, context: &Context) -> Vec<Vec<(u32, u8, QuoteRole, String)>> {
        let summary = self.current().map_or("", |work| work.summary.trim());
        let paragraphs = summary
            .split('\n')
            .map(str::trim)
            .filter(|paragraph| !paragraph.is_empty())
            .map(|paragraph| (0, 0, QuoteRole::Body, paragraph))
            .collect::<Vec<_>>();
        context.paginate_tagged_under(&paragraphs, false, &self.summary_head(true).build())
    }

    fn follow_screen(&self, context: &Context) -> Screen {
        let mut screen = ScreenBuilder::new("fs-follow")
            .top_bar("Followed AO3 tags")
            .top_bar_action("add-tag", "Add")
            .top_bar_action("shelf", "Shelf");
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        if self.tags.is_empty() {
            screen = screen.splash(
                Some(Glyph::Bookmark),
                "No followed tags",
                "Follow a tag to see its newest works.",
            );
        } else {
            let rows = self
                .tags
                .iter()
                .enumerate()
                .map(|(index, tag)| {
                    (
                        format!("tag-{index}"),
                        display(&tag.name, 90),
                        "Newest works in this tag".to_owned(),
                        Glyph::Bookmark,
                    )
                })
                .collect();
            screen = Self::paged(context, screen, self.tag_page, rows);
        }
        screen.owns_back(true).build()
    }

    fn feed_screen(&self, context: &Context) -> Screen {
        let title = self
            .open_tag
            .and_then(|index| self.tags.get(index))
            .map_or("Tag feed", |tag| tag.name.as_str());
        let mut screen = ScreenBuilder::new("fs-feed")
            .top_bar(display(title, 54))
            .top_bar_action("shelf", "Shelf")
            .secondary("Newest works in this tag on AO3");
        if self.task.is_some() && self.feed.is_empty() {
            screen = screen.skeleton(6);
        } else if self.feed.is_empty() {
            screen = screen.splash(
                Some(Glyph::Book),
                "No feed entries",
                self.message
                    .as_deref()
                    .unwrap_or("AO3 listed no works for this tag."),
            );
        } else {
            let rows = self
                .feed
                .iter()
                .enumerate()
                .map(|(index, work)| {
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
                })
                .collect();
            screen = Self::paged(context, screen, self.feed_page, rows);
        }
        screen.owns_back(true).build()
    }

    fn updates_screen(&self, context: &Context) -> Screen {
        let wips = self
            .works
            .iter()
            .enumerate()
            .filter(|(_, work)| !work.complete)
            .collect::<Vec<_>>();
        let mut screen = ScreenBuilder::new("fs-updates")
            .top_bar("Updates")
            .top_bar_action("update-all", "Update all")
            .top_bar_action("shelf", "Shelf");
        if let Some(message) = &self.message {
            screen = screen.banner(BannerLevel::Info, message);
        }
        let waiting = self.updates_waiting();
        if waiting > 0 && self.sweep.is_none() {
            screen = screen.secondary(format!(
                "New chapters waiting for {}. Update all downloads them.",
                plural(waiting)
            ));
        }
        if wips.is_empty() {
            screen = screen.splash(
                Some(Glyph::Check),
                "No works in progress",
                "Works in progress appear here.",
            );
        } else {
            let rows = wips
                .iter()
                .map(|(index, work)| {
                    let state = if work.download == DownloadState::UpdateAvailable {
                        "Update waiting to download"
                    } else if self.fresh.contains(&work.id) {
                        "New chapters, not opened yet"
                    } else if work.download == DownloadState::Removed {
                        REMOVED
                    } else if work.last_checked == 0 {
                        "Never checked"
                    } else {
                        "No new chapters at the last check"
                    };
                    (
                        format!("update-{index}"),
                        display(&work.title, 76),
                        format!("{} · {state}", work.chapters_label()),
                        Glyph::Clock,
                    )
                })
                .collect();
            screen = Self::paged(context, screen, self.updates_page, rows);
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
            self.request_failed(context, &request);
            self.start_next(context);
            return;
        };
        if let Some(task) = context.spawn(work) {
            self.sent_request = true;
            self.task = Some((task, Active::Fetching(request)));
        } else {
            self.message = Some("Try again in a moment.".into());
            self.request_failed(context, &request);
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

    /// Asks for a work's EPUB, after whatever download is already running.
    fn begin_download(&mut self, context: &mut Context, work: usize, open_after: bool) {
        if self.downloading == Some(work) || self.downloads.iter().any(|(w, _)| *w == work) {
            return;
        }
        self.downloads.push_back((work, open_after));
        self.next_download(context);
    }

    fn next_download(&mut self, context: &mut Context) {
        if self.downloading.is_some() || self.upload.is_some() {
            return;
        }
        let Some((work, open_after)) = self.downloads.pop_front() else {
            self.settle_sweep();
            return;
        };
        self.downloading = Some(work);
        self.bytes.clear();
        self.open_after_upload = open_after;
        self.message = Some("Downloading EPUB…".into());
        self.enqueue(context, Request::Epub { work, offset: 0 });
    }

    /// The running download is over, saved or not. The next one starts.
    fn download_ended(&mut self, context: &mut Context) {
        self.downloading = None;
        self.bytes.clear();
        self.next_download(context);
    }

    /// Checks every work in progress and downloads what has new chapters.
    ///
    /// A work already marked as having new chapters is downloaded straight
    /// away rather than checked again: the EPUB AO3 hands over is the latest
    /// one either way.
    fn update_all(&mut self, context: &mut Context) {
        let mut sweep = Sweep::default();
        let mut requests = Vec::new();
        let mut waiting = Vec::new();
        for (index, work) in self.works.iter().enumerate() {
            if work.download == DownloadState::UpdateAvailable {
                sweep.found.insert(index);
                waiting.push(index);
            } else if !work.complete && work.download != DownloadState::Removed {
                sweep.pending.insert(index);
                requests.push(Request::Update {
                    work: index,
                    adult: work.adult,
                });
            }
        }
        if requests.is_empty() && waiting.is_empty() {
            self.message = Some("No works in progress to update.".into());
            return;
        }
        self.message = Some(if requests.is_empty() {
            format!("Downloading new chapters for {}…", plural(waiting.len()))
        } else {
            format!("Checking {} for new chapters…", plural(requests.len()))
        });
        self.sweep = Some(sweep);
        for request in requests {
            self.enqueue(context, request);
        }
        for index in waiting {
            self.begin_download(context, index, false);
        }
    }

    /// Ends a pass of Update all with one sentence, once nothing is left.
    fn settle_sweep(&mut self) {
        let Some(sweep) = &self.sweep else {
            return;
        };
        let downloading = self
            .downloading
            .is_some_and(|work| sweep.found.contains(&work))
            || self
                .downloads
                .iter()
                .any(|(work, _)| sweep.found.contains(work));
        if !sweep.pending.is_empty() || downloading {
            return;
        }
        let mut said = if sweep.checked == 0 {
            String::new()
        } else {
            format!("Checked {}. ", plural(sweep.checked))
        };
        said.push_str(&match (sweep.found.len(), sweep.saved) {
            (0, _) => "No new chapters.".to_owned(),
            (found, saved) if saved == found => {
                format!("New chapters downloaded for {}.", plural(found))
            }
            (found, saved) => format!(
                "New chapters downloaded for {saved} of {}. Update all tries the rest again.",
                plural(found)
            ),
        });
        if sweep.failed > 0 {
            let _ = write!(said, " {} could not be checked.", plural(sweep.failed));
        }
        self.message = Some(said);
        self.sweep = None;
    }

    /// An Update request for `work` finished, with or without an answer.
    fn checked(&mut self, work: usize, answered: bool) {
        if let Some(sweep) = &mut self.sweep {
            if sweep.pending.remove(&work) {
                if answered {
                    sweep.checked += 1;
                } else {
                    sweep.failed += 1;
                }
            }
        }
        self.settle_sweep();
    }

    fn finish_lookup(&mut self, context: &mut Context, id: String, adult: bool, body: &[u8]) {
        let text = String::from_utf8_lossy(body);
        match parse_work_page(&id, &text) {
            ParsedWork::Work(mut incoming) => {
                incoming.adult = adult;
                incoming.last_checked = now_seconds();
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
                self.message =
                    Some("Couldn't read AO3's answer. Nothing on the shelf changed.".into());
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
                incoming.last_checked = now_seconds();
                let changed = incoming.chapters > existing.chapters
                    || (!incoming.updated.is_empty() && incoming.updated != existing.updated);
                incoming.download = if changed && existing.downloaded() {
                    DownloadState::UpdateAvailable
                } else {
                    existing.download
                };
                let downloaded = existing.downloaded();
                self.works[index] = *incoming;
                self.message = Some(if changed && downloaded {
                    "New chapters found. Downloading them now.".into()
                } else if changed {
                    "New chapters found. Download the work to read them offline.".into()
                } else {
                    "No new chapters found.".into()
                });
                self.save_works(context);
                if changed && downloaded {
                    if let Some(sweep) = &mut self.sweep {
                        sweep.found.insert(index);
                    }
                    self.begin_download(context, index, false);
                }
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
                self.message =
                    Some("Couldn't read AO3's answer. Nothing on the shelf changed.".into());
            }
        }
    }

    /// A downloaded EPUB is on the shelf. The work is marked, and the next
    /// download in line starts.
    fn upload_saved(&mut self, context: &mut Context) {
        self.upload = None;
        if let Some(work) = self.upload_work.take() {
            if let Some(item) = self.works.get_mut(work) {
                item.download = DownloadState::Downloaded;
            }
            if let Some(sweep) = self
                .sweep
                .as_mut()
                .filter(|sweep| sweep.found.contains(&work))
            {
                sweep.saved += 1;
            }
            if !self.open_after_upload {
                if let Some(item) = self.works.get(work) {
                    if self.fresh.insert(item.id.clone()) {
                        context.store().save(FRESH_KEY, encode_reading(&self.fresh));
                    }
                }
            }
            self.save_works(context);
            if self.open_after_upload {
                self.start_read(context, work);
            } else {
                self.message = Some("New chapters downloaded. Your reading place is kept.".into());
            }
        }
        context.shelf().list();
        self.download_ended(context);
    }

    /// What the shelf says its files weigh, for the offline line.
    fn took_shelf_list(&mut self, files: &[(String, u32)]) {
        self.shelf_sizes = Some(files.iter().cloned().collect());
        #[cfg(not(target_arch = "arm"))]
        self.weigh_demo();
    }

    /// The demonstration shelf has no files behind it, so its copies are
    /// given the weight a short novel would have. Called from both sides,
    /// because the shelf's answer and the demonstration works arrive in
    /// either order.
    #[cfg(not(target_arch = "arm"))]
    fn weigh_demo(&mut self) {
        if !self.demo {
            return;
        }
        if let Some(sizes) = &mut self.shelf_sizes {
            for work in self.works.iter().filter(|work| work.downloaded()) {
                sizes.entry(shelf_name(&work.id)).or_insert(420 * 1024);
            }
        }
    }

    /// A request that ended without an answer, as far as what waits on it.
    fn request_failed(&mut self, context: &mut Context, request: &Request) {
        match request {
            Request::Update { work, .. } => self.checked(*work, false),
            Request::Epub { work, .. } if self.downloading == Some(*work) => {
                self.download_ended(context);
            }
            _ => {}
        }
    }

    fn finish_feed(&mut self, body: &[u8]) {
        let text = String::from_utf8_lossy(body);
        self.feed = parse_feed(&text);
        self.feed_page = 0;
        self.message = self
            .feed
            .is_empty()
            .then(|| "AO3 listed no works for this tag.".into());
    }

    fn finish_epub(&mut self, context: &mut Context, work: usize, chunk: &[u8]) {
        if self.bytes.len().saturating_add(chunk.len()) > MAX_EPUB {
            self.message = Some("This EPUB is too large to keep on this reader.".into());
            self.download_ended(context);
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
            self.download_ended(context);
            return;
        };
        let mut upload = ShelfUpload::new(shelf_name(&item.id), self.bytes.clone());
        upload.start(context);
        self.upload = Some(upload);
        self.upload_work = Some(work);
        self.message = Some("Saving EPUB…".into());
    }

    fn start_read(&mut self, context: &mut Context, work: usize) {
        let Some(item) = self.works.get(work) else {
            return;
        };
        if self.fresh.remove(&item.id) {
            context.store().save(FRESH_KEY, encode_reading(&self.fresh));
        }
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
        if self.reading.insert(item.id.clone()) {
            context
                .store()
                .save(READING_KEY, encode_reading(&self.reading));
        }
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
                last_checked: 1_789_617_600,
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
                last_checked: 1_789_617_600,
            },
            Work {
                id: "9003".into(),
                title: "A Field Guide to Small Hours".into(),
                author: "North Star".into(),
                fandom: "Synthetic Library Stories".into(),
                rating: "General Audiences".into(),
                warnings: "No Archive Warnings Apply".into(),
                summary: "A synthetic work in progress.".into(),
                chapters: 4,
                total_chapters: None,
                complete: false,
                updated: "2026-09-10".into(),
                epub: "https://archiveofourown.org/downloads/9003/demo.epub".into(),
                download: DownloadState::NotDownloaded,
                adult: false,
                last_checked: 0,
            },
        ];
        self.tags = vec![
            parse_tag("Public Domain Fairy Tales").unwrap(),
            parse_tag("Synthetic Library Stories").unwrap(),
        ];
        self.reading = ["9002".to_owned()].into_iter().collect();
        self.seed_demo_feed();
        self.weigh_demo();
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
        context.store().load(READING_KEY);
        context.store().load(FRESH_KEY);
        context.shelf().list();
        self.show(context);
    }

    fn on_store(&mut self, context: &mut Context, result: StoreResult) {
        if let StoreResult::Shelf(files) = &result {
            self.took_shelf_list(files);
        }
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
            } else if key == FRESH_KEY {
                self.fresh = value.as_deref().map_or_else(BTreeSet::new, decode_reading);
            } else if key == READING_KEY {
                self.reading = value
                    .as_deref()
                    .map_or_else(std::collections::BTreeSet::new, decode_reading);
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
                ShelfProgress::Done => self.upload_saved(context),
                ShelfProgress::Failed(_) => {
                    self.upload = None;
                    self.upload_work = None;
                    self.message = Some(
                        "The EPUB could not be saved. The previous shelf copy is unchanged.".into(),
                    );
                    self.download_ended(context);
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
        // Back is the way out of a typing screen, so it is not offered to the
        // field first: the field has no use for it, and the screen it was
        // pressed on would stay put.
        if matches!(self.view, View::Add | View::AddTag) && action != ActionId::BACK {
            let entry = if self.view == View::Add {
                &mut self.work_entry
            } else {
                &mut self.tag_entry
            };
            match entry.handle(action) {
                Some(Typing::Submitted(entered)) => self.submitted(context, entered),
                // Backing out of the reader's keyboard is leaving the screen
                // it was opened from, as Back always was.
                Some(Typing::Cancelled | Typing::Saved | Typing::Refused(_)) => {
                    self.view = if self.view == View::Add {
                        View::Shelf
                    } else {
                        View::Follow
                    };
                    self.message = None;
                }
                Some(Typing::Changed) | None => {}
            }
            self.show(context);
            return;
        }

        if self.view == View::Work && action == action_id("more") {
            self.work_menu = true;
            self.show(context);
            return;
        }
        // A tap outside the open menu arrives as Back, and only closes it.
        if self.work_menu && action == ActionId::BACK {
            self.work_menu = false;
            self.show(context);
            return;
        }
        self.work_menu = false;
        if action == action_id("summary") {
            self.summary_page = 0;
            self.view = View::Summary;
        } else if action == action_id("summary-previous") {
            self.summary_page = self.summary_page.saturating_sub(1);
        } else if action == action_id("summary-next") {
            let last = self.summary_pages(context).len().saturating_sub(1);
            self.summary_page = self.summary_page.saturating_add(1).min(last);
        } else if action == action_id("add") {
            self.work_entry.open();
            self.view = View::Add;
            self.message = None;
        } else if action == action_id("filter") {
            self.fandom_page = 0;
            self.view = View::Fandoms;
            self.message = None;
        } else if action == action_id("all") {
            self.filter = None;
            self.shelf_page = 0;
            self.message = None;
        } else if let Some(fandom) = (0..self.fandoms().len())
            .find(|index| action == action_id(&format!("fandom-{index}")))
            .and_then(|index| self.fandoms().get(index).map(|(fandom, _)| fandom.clone()))
        {
            self.filter = Some(fandom);
            self.shelf_page = 0;
            self.view = View::Shelf;
            self.message = None;
        } else if action == action_id("manage") {
            self.confirm_remove = false;
            self.view = View::Manage;
            self.message = None;
        } else if action == action_id("manage-remove") {
            self.confirm_remove = true;
            self.message = None;
        } else if action == action_id("manage-cancel") {
            self.confirm_remove = false;
            self.message = None;
        } else if action == action_id("remove-copies") {
            let mut removed = 0;
            for work in &mut self.works {
                if matches!(
                    work.download,
                    DownloadState::Downloaded | DownloadState::UpdateAvailable
                ) {
                    context.shelf().remove(shelf_name(&work.id));
                    work.download = DownloadState::NotDownloaded;
                    removed += 1;
                }
            }
            self.confirm_remove = false;
            if removed > 0 {
                self.save_works(context);
                context.shelf().list();
            }
            self.message = Some(format!(
                "Removed {removed} copies. Works stay on the shelf."
            ));
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
            self.tag_entry.open();
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
        } else if action == action_id("update-all") {
            self.update_all(context);
        } else if action == action_id("page-prev") || action == action_id("page-next") {
            // Clamp from the page actually shown, including after a collection
            // shrinks. Repeated turns at either boundary must remain harmless.
            let (page, total) = self
                .screen(context)
                .page_turns
                .and_then(|turns| turns.position)
                .unwrap_or((1, 1));
            let current = usize::from(page.saturating_sub(1));
            let next = if action == action_id("page-prev") {
                current.saturating_sub(1)
            } else {
                current
                    .saturating_add(1)
                    .min(usize::from(total.saturating_sub(1)))
            };
            match self.view {
                View::Shelf => self.shelf_page = next,
                View::Follow => self.tag_page = next,
                View::Fandoms => self.fandom_page = next,
                View::Feed => self.feed_page = next,
                View::Updates => self.updates_page = next,
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
                View::Work
                | View::Follow
                | View::Updates
                | View::Add
                | View::Adult
                | View::Fandoms
                | View::Manage => {
                    self.view = View::Shelf;
                }
                View::Feed | View::AddTag => self.view = View::Follow,
                View::Summary => self.view = View::Work,
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
                } else {
                    self.request_failed(context, &request);
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
                            self.checked(work, true);
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
                    self.request_failed(context, &request);
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
                    self.request_failed(context, &request);
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
                    self.request_failed(context, &request);
                    self.start_next(context);
                }
                TaskOutcome::Cancelled => {
                    self.request_failed(context, &request);
                    self.start_next(context);
                }
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

    #[test]
    fn a_mistyped_work_comes_back_in_the_field_with_the_reason_under_the_prompt() {
        let mut runner = AppRunner::new(Fanshelf::default());
        let opened = runner.action(action_id("add"));
        let Some(kobo_sdk::DeviceRequest::TypeText { kind, .. }) =
            kobo_sdk::keyboard::testing::asked(&opened)
        else {
            panic!("Add did not ask for the reader's keyboard");
        };
        assert_eq!(*kind, kobo_sdk::FieldKind::Address);
        let refused = runner.type_text("nonsense");
        let Some(kobo_sdk::DeviceRequest::TypeText { hint, initial, .. }) =
            kobo_sdk::keyboard::testing::asked(&refused)
        else {
            panic!("the field did not come back for a correction");
        };
        assert!(
            hint.starts_with("That is not an AO3 work address or number."),
            "{hint}"
        );
        assert_eq!(initial, "nonsense");
        runner.cancel_text();
        assert_eq!(runner.app().view, View::Shelf);
        assert_eq!(runner.app().message, None);
    }

    #[test]
    fn a_tag_is_typed_with_a_space_bar_and_backing_out_returns_to_the_followed_tags() {
        let mut runner = AppRunner::new(Fanshelf {
            view: View::Follow,
            ..Fanshelf::default()
        });
        let opened = runner.action(action_id("add-tag"));
        let Some(kobo_sdk::DeviceRequest::TypeText { kind, .. }) =
            kobo_sdk::keyboard::testing::asked(&opened)
        else {
            panic!("Add did not ask for the reader's keyboard");
        };
        assert_eq!(*kind, kobo_sdk::FieldKind::Text);
        runner.cancel_text();
        assert_eq!(runner.app().view, View::Follow);
    }

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
            last_checked: 0,
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
        let screen = app.screen(&AppRunner::new(Fanshelf::default()).context());
        let layout = screen.layout_with(&CLARA_BW_METRICS, &Chrome::measuring(true));
        let top_of = |prefix: &str| {
            layout
                .nodes
                .iter()
                .find(|node| {
                    node.text_lines
                        .first()
                        .is_some_and(|line| line.starts_with(prefix))
                })
                .map(|node| node.rect.y)
                .unwrap()
        };
        let download = layout.rect_of_action(action_id("download")).unwrap();
        assert!(top_of("Rating:") < download.y && top_of("Warnings:") < download.y);
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
    fn manage_counts_track_download_states() {
        let mut app = Fanshelf {
            works: vec![work()],
            ..Fanshelf::default()
        };
        assert_eq!(app.updates_waiting(), 0);
        assert_eq!(app.downloaded_count(), 0);
        app.works[0].download = DownloadState::UpdateAvailable;
        assert_eq!(app.updates_waiting(), 1);
        assert_eq!(app.downloaded_count(), 1);
        app.works[0].download = DownloadState::Downloaded;
        assert_eq!(app.updates_waiting(), 0);
        assert_eq!(app.downloaded_count(), 1);
        app.works[0].download = DownloadState::Removed;
        assert_eq!(app.downloaded_count(), 0);
    }

    #[test]
    fn shelf_filter_groups_by_fandom_and_restores_the_whole_shelf() {
        let mut app = Fanshelf::default();
        let mut fairy = work();
        fairy.fandom = "Fairy Tales".into();
        let mut stars = work();
        stars.id = "42".into();
        stars.fandom = "Star Stories".into();
        app.works = vec![fairy, stars];
        assert_eq!(
            app.fandoms(),
            [
                ("Fairy Tales".to_owned(), 1),
                ("Star Stories".to_owned(), 1),
            ]
        );
        assert_eq!(app.visible(), [0, 1]);
        app.filter = Some("Star Stories".to_owned());
        assert_eq!(app.visible(), [1]);
        app.filter = Some("Unknown".to_owned());
        assert!(app.visible().is_empty());
        app.filter = None;
        assert_eq!(app.visible(), [0, 1]);
    }

    #[test]
    fn shelf_badge_prefers_update_then_never_checked_then_reading() {
        let mut work = work();
        let mut reading = std::collections::BTreeSet::new();
        let mut fresh = BTreeSet::new();
        work.download = DownloadState::UpdateAvailable;
        assert_eq!(
            Fanshelf::badge(&work, &reading, &fresh),
            " · update waiting"
        );
        work.download = DownloadState::NotDownloaded;
        work.last_checked = 0;
        assert_eq!(
            Fanshelf::badge(&work, &reading, &fresh),
            " · updates unchecked"
        );
        work.complete = true;
        work.download = DownloadState::Downloaded;
        assert_eq!(Fanshelf::badge(&work, &reading, &fresh), " · offline");
        reading.insert(work.id.clone());
        assert_eq!(Fanshelf::badge(&work, &reading, &fresh), " · reading");
        fresh.insert(work.id.clone());
        assert_eq!(Fanshelf::badge(&work, &reading, &fresh), " · new chapters");
    }

    /// The archive's page for work 42, with `chapters` chapters posted.
    fn page(chapters: u16) -> Vec<u8> {
        format!(
            r#"<html><body><h2 class="title heading">A safe synthetic work</h2>
            <a rel="author" href="/users/x">Example Author</a>
            <dd class="chapters">{chapters}/?</dd>
            <dd class="updated">2026-10-0{}</dd>
            <a href="/downloads/42/work.epub?updated_at={chapters}">EPUB</a>
            </body></html>"#,
            chapters % 9 + 1
        )
        .into_bytes()
    }

    fn spawned(commands: &[Command]) -> Option<TaskId> {
        commands.iter().find_map(|command| match command {
            Command::Spawn { task, .. } => Some(*task),
            _ => None,
        })
    }

    /// Answers whatever is running, spacing sleeps included, until a fetch
    /// for `wanted` is the one in flight.
    fn run_until(runner: &mut AppRunner<Fanshelf>, wanted: &Request) {
        for _ in 0..8 {
            match runner.app().task.clone() {
                Some((_, Active::Fetching(request))) if request == *wanted => return,
                Some((task, Active::Spacing(_) | Active::Backoff(_))) => {
                    runner.task_outcome(task, TaskOutcome::Completed(Vec::new()));
                }
                other => panic!("waiting for {wanted:?}, found {other:?}"),
            }
        }
        panic!("{wanted:?} was never asked for");
    }

    fn answer(runner: &mut AppRunner<Fanshelf>, body: Vec<u8>) -> Vec<Command> {
        let task = runner.app().task.as_ref().unwrap().0;
        runner.task_outcome(task, TaskOutcome::Completed(body))
    }

    #[test]
    fn checking_a_downloaded_work_downloads_its_new_chapters_without_another_tap() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        let mut downloaded = work();
        downloaded.download = DownloadState::Downloaded;
        runner.app_mut().works = vec![downloaded];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Work;
        runner.action(action_id("check"));
        answer(&mut runner, page(41));
        assert_eq!(runner.app().downloading, Some(0));
        let epub = Request::Epub { work: 0, offset: 0 };
        run_until(&mut runner, &epub);
        answer(&mut runner, b"a small epub".to_vec());
        assert!(runner.app().upload.is_some(), "the chapters were not saved");
        runner.store_result(StoreResult::ShelfWritten {
            name: shelf_name("42"),
            size: 12,
        });
        let app = runner.app();
        assert_eq!(app.works[0].download, DownloadState::Downloaded);
        assert_eq!(app.downloading, None);
        assert!(app.fresh.contains("42"), "new chapters lost their mark");
    }

    #[test]
    fn checking_a_work_that_was_never_downloaded_downloads_nothing() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        runner.app_mut().works = vec![work()];
        runner.app_mut().open = Some(0);
        runner.app_mut().view = View::Work;
        runner.action(action_id("check"));
        let commands = answer(&mut runner, page(41));
        assert!(spawned(&commands).is_none());
        assert_eq!(runner.app().downloading, None);
        assert_eq!(
            runner.app().message.as_deref(),
            Some("New chapters found. Download the work to read them offline.")
        );
    }

    #[test]
    fn update_all_checks_every_work_in_progress_and_ends_on_what_it_downloaded() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        let mut moved = work();
        moved.download = DownloadState::Downloaded;
        let mut still = work();
        still.id = "43".into();
        still.chapters = 41;
        still.updated = "2026-10-06".into();
        still.download = DownloadState::Downloaded;
        runner.app_mut().works = vec![moved, still];
        runner.app_mut().view = View::Updates;
        runner.action(action_id("update-all"));
        assert_eq!(
            runner.app().message.as_deref(),
            Some("Checking 2 works for new chapters…")
        );
        answer(&mut runner, page(41));
        run_until(
            &mut runner,
            &Request::Update {
                work: 1,
                adult: false,
            },
        );
        answer(&mut runner, page(41));
        run_until(&mut runner, &Request::Epub { work: 0, offset: 0 });
        answer(&mut runner, b"a small epub".to_vec());
        runner.store_result(StoreResult::ShelfWritten {
            name: shelf_name("42"),
            size: 12,
        });
        assert_eq!(runner.app().sweep, None);
        assert_eq!(
            runner.app().message.as_deref(),
            Some("Checked 2 works. New chapters downloaded for 1 work.")
        );
    }

    #[test]
    fn a_second_download_waits_for_the_first_rather_than_sharing_its_buffer() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        let mut second = work();
        second.id = "43".into();
        runner.app_mut().works = vec![work(), second];
        let mut context = runner.context();
        let app = runner.app_mut();
        app.begin_download(&mut context, 0, false);
        app.begin_download(&mut context, 1, false);
        app.begin_download(&mut context, 1, false);
        assert_eq!(app.downloading, Some(0));
        assert_eq!(app.downloads, VecDeque::from([(1, false)]));
        app.bytes = b"half of the first".to_vec();
        app.request_failed(&mut context, &Request::Epub { work: 0, offset: 0 });
        assert_eq!(app.downloading, Some(1));
        assert!(app.bytes.is_empty());
    }

    #[test]
    fn the_shelf_says_what_is_ready_offline_once_the_reader_has_measured_it() {
        let mut runner = AppRunner::new(Fanshelf::default());
        runner.start();
        runner.app_mut().works_loaded = true;
        runner.app_mut().tags_loaded = true;
        let mut kept = work();
        kept.download = DownloadState::Downloaded;
        runner.app_mut().works = vec![kept, work()];
        assert_eq!(runner.app().offline_line(), None);
        runner.store_result(StoreResult::Shelf(vec![(
            shelf_name("42"),
            3 * 1024 * 1024,
        )]));
        assert_eq!(
            runner.app().offline_line().as_deref(),
            Some("Ready offline: 1 work, 3.0 MB")
        );
        let screen = runner.app().screen(&runner.context());
        assert!(screen
            .nodes
            .iter()
            .filter_map(node_label)
            .any(|text| text.starts_with("Ready offline")));
    }

    #[test]
    fn a_work_page_shows_the_summary_and_shortens_it_before_it_runs_off_the_panel() {
        let mut long = work();
        long.summary = "A lantern keeper finds a door in the library that opens onto every \
            night she has ever forgotten. "
            .repeat(6)
            .trim_end()
            .to_owned();
        for metrics in collection_tests::metrics() {
            let app = Fanshelf {
                works: vec![long.clone()],
                open: Some(0),
                view: View::Work,
                ..Fanshelf::default()
            };
            let runner = AppRunner::with_metrics(Fanshelf::default(), metrics);
            let screen = app.screen(&runner.context());
            assert!(
                Fanshelf::fits(&runner.context(), &screen),
                "{}% runs off the panel",
                metrics.text_scale.percent()
            );
            let labels = screen
                .nodes
                .iter()
                .filter_map(node_label)
                .collect::<Vec<_>>();
            assert!(screen
                .layout_with(&metrics, &Chrome::measuring(true))
                .rect_of_action(action_id("download"))
                .is_some());
            if metrics.text_scale.percent() == 100 {
                assert!(
                    labels
                        .iter()
                        .any(|text| text.starts_with("A lantern keeper")),
                    "the summary is missing at the default size"
                );
            }
        }
    }

    /// Every word of a work's summary is somewhere an owner can read it, at
    /// every text size held either way: on the work's page, or, where the page
    /// had to cut it, on the summary pages its menu opens. The demonstration
    /// shelf is used because its first work carries every line the page has,
    /// an update waiting included, and a long summary is added to it.
    #[test]
    fn a_work_summary_is_always_within_reach_and_every_page_fits_at_every_size_held_either_way() {
        let mut shelf = Fanshelf {
            demo: true,
            works_loaded: true,
            tags_loaded: true,
            ..Fanshelf::default()
        };
        shelf.seed_demo();
        let mut works = shelf.works.clone();
        let mut long = works[0].clone();
        long.summary = "A lantern keeper finds a door in the library that opens onto every \
            night she has ever forgotten. "
            .repeat(8)
            .trim_end()
            .to_owned();
        works.push(long);
        for metrics in collection_tests::metrics() {
            for orientation in [
                kobo_ui::Orientation::Portrait,
                kobo_ui::Orientation::Landscape,
            ] {
                let metrics = metrics.oriented(orientation);
                let mut runner = AppRunner::with_metrics(
                    Fanshelf {
                        works: works.clone(),
                        works_loaded: true,
                        tags_loaded: true,
                        ..Fanshelf::default()
                    },
                    metrics,
                );
                for (index, work) in works.iter().enumerate() {
                    let at = format!("{metrics:?}, {}", work.title);
                    runner.app_mut().view = View::Shelf;
                    runner.action(action_id(&format!("work-{index}")));
                    let context = runner.context();
                    let page = runner.app().screen(&context);
                    assert!(
                        Fanshelf::fits(&context, &page),
                        "{at}: the work page runs off"
                    );
                    let shown = format!("{page:?}");
                    assert!(
                        shown.contains("Read") || shown.contains("Download EPUB"),
                        "{at}"
                    );
                    runner.action(action_id("more"));
                    let menu = format!("{:?}", runner.app().screen(&runner.context()));
                    assert!(menu.contains("Check for new chapters"), "{at}");
                    let summary = work.summary.trim();
                    if shown.contains(summary) {
                        assert!(!menu.contains("Whole summary"), "{at}");
                        runner.action(ActionId::BACK);
                        continue;
                    }
                    assert!(
                        menu.contains("Whole summary"),
                        "{at}: the summary is cut and out of reach"
                    );
                    runner.action(action_id("summary"));
                    let mut read = String::new();
                    loop {
                        let screen = runner.app().screen(&runner.context());
                        assert!(
                            Fanshelf::fits(&runner.context(), &screen),
                            "{at}: a summary page runs off"
                        );
                        for node in &screen.nodes {
                            if let Node::Quote { text, .. } = node {
                                read.push_str(text);
                                read.push(' ');
                            }
                        }
                        let before = runner.app().summary_page;
                        runner.action(action_id("summary-next"));
                        if runner.app().summary_page == before {
                            break;
                        }
                    }
                    let split = |text: &str| {
                        text.split_whitespace()
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(split(&read), split(summary), "{at}");
                    runner.action(ActionId::BACK);
                    assert_eq!(runner.app().view, View::Work, "{at}");
                }
            }
        }
    }

    #[test]
    fn update_label_distinguishes_never_checked_current_and_waiting() {
        let mut work = work();
        assert_eq!(update_label(&work, false), "Updates never checked");
        work.last_checked = 1_789_617_600;
        assert!(update_label(&work, false).starts_with("No new chapters as of "));
        assert!(update_label(&work, true).starts_with("New chapters downloaded "));
        work.download = DownloadState::UpdateAvailable;
        assert!(update_label(&work, false).starts_with("New chapters found "));
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
            "Locked to AO3 members. Fanshelf has no account sign-in, so it cannot download this work."
        );
        assert_eq!(REMOVED, "removed from the archive");
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
            View::Fandoms,
            View::Manage,
        ] {
            app.view = view;
            app.open = Some(0);
            app.open_tag = Some(0);
            assert!(
                app.screen(&AppRunner::new(Fanshelf::default()).context())
                    .diagnostics(&CLARA_BW_METRICS, &Chrome::default())
                    .issues
                    .is_empty(),
                "{view:?} does not fit"
            );
        }
    }
}
