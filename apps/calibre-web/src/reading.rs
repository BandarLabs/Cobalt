//! Download, verified local reopening, and serialized library/reading-state writes.
use super::{
    books::{self, Book, Library},
    catalog,
};
use kobo_bookview::BookView;
use kobo_read::{Memory, Outcome};
use kobo_sdk::{
    action_id, ActionId, BannerLevel, Context, Credential, Position, RowEdge, Screen,
    ScreenBuilder, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId,
    TaskOutcome,
};
const CHUNK: u32 = 256 * 1024;
/// The damaged book's own button. "Download again" was cut to "Download…"
/// upright from Larger text, where a row button may take a third of the row,
/// and beside "Needs a new download" the one word says it whole.
const REPAIR_LABEL: &str = "Download";
/// A failure that stays until the owner retries it, and so keeps its retry
/// on screen while other news comes and goes above it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Retry {
    /// The list of downloaded books could not be read; nothing may be
    /// written over it until a load succeeds.
    Load,
    /// The list of downloaded books, or a reading position, was not written.
    Save,
}
impl Retry {
    fn action(self) -> (&'static str, &'static str) {
        match self {
            Self::Load => ("offline-reload", "Retry loading"),
            Self::Save => ("offline-save", "Retry saving"),
        }
    }
    /// Said after a later notice that has taken the place of the words the
    /// failure first came with. One line, because it shares the banner with
    /// that notice.
    fn standing(self) -> &'static str {
        match self {
            Self::Load => "The downloaded library could not be loaded.",
            Self::Save => "Library changes are not saved.",
        }
    }
}
/// What the last thing done came to, said at the level its meaning calls
/// for, so a saved book and a refused one no longer read alike.
#[derive(Clone, Debug, PartialEq)]
struct Notice {
    level: BannerLevel,
    text: String,
    /// Set when this notice is the one that reports a standing failure, so
    /// the retry goes on these words rather than on a second banner.
    retry: Option<Retry>,
}
#[derive(Default, PartialEq)]
enum LoadState {
    #[default]
    Loading,
    Ready,
    Blocked,
}
#[derive(Default)]
pub struct Reading {
    pub visible: bool,
    library: Library,
    load: LoadState,
    notice: Option<Notice>,
    offset: usize,
    book: Option<Book>,
    repair: Option<Book>,
    view: BookView,
    opened: bool,
    download: Option<TaskId>,
    bytes: Vec<u8>,
    credential: Option<String>,
    upload: Option<ShelfUpload>,
    local: Option<ShelfDownload>,
    write: Option<Vec<u8>>,
    saved: Vec<u8>,
    save_failed: bool,
}
impl Reading {
    pub fn start(&mut self, c: &mut Context) {
        self.load = LoadState::Loading;
        c.store().load(books::KEY);
    }
    pub fn load_failed(&mut self) {
        self.load = LoadState::Blocked;
        self.fail(
            Retry::Load,
            "The downloaded library could not be loaded. Your saved books are kept.",
        );
    }
    fn say(&mut self, level: BannerLevel, text: impl Into<String>) {
        self.notice = Some(Notice {
            level,
            text: text.into(),
            retry: None,
        });
    }
    fn fail(&mut self, retry: Retry, text: impl Into<String>) {
        self.notice = Some(Notice {
            level: BannerLevel::Error,
            text: text.into(),
            retry: Some(retry),
        });
    }
    /// The failure that waits on the owner, if any. None while work is in
    /// flight, because every tap but Cancel is ignored until it ends.
    fn standing(&self) -> Option<Retry> {
        if self.load == LoadState::Loading || self.busy() {
            None
        } else if self.load == LoadState::Blocked {
            Some(Retry::Load)
        } else if self.save_failed {
            Some(Retry::Save)
        } else {
            None
        }
    }
    /// One banner: the notice, and the standing failure with its retry. When
    /// the notice is that failure's own report it carries the retry as it
    /// is. When other news has come since, the failure is said after it, so
    /// Retry saving sits directly under the words it answers and never under
    /// words about something else alone. A second banner for the failure
    /// cost the last book its place on a sideways panel at the largest size.
    fn notices(&self, s: ScreenBuilder) -> ScreenBuilder {
        let standing = self.standing();
        match (&self.notice, standing) {
            (Some(notice), Some(retry)) if notice.retry == Some(retry) => {
                let (name, label) = retry.action();
                s.banner(notice.level, &notice.text)
                    .banner_action(name, label)
            }
            (notice, Some(retry)) => {
                let (name, label) = retry.action();
                let text = notice.as_ref().map_or_else(
                    || retry.standing().to_owned(),
                    |notice| format!("{} {}", notice.text, retry.standing()),
                );
                s.banner(BannerLevel::Error, text)
                    .banner_action(name, label)
            }
            (Some(notice), None) => s.banner(notice.level, &notice.text),
            (None, None) => s,
        }
    }
    pub fn screen(&self, c: &Context) -> Screen {
        if self.opened {
            if let Some(screen) = self
                .view
                .screen(self.book.as_ref().map_or("Book", |b| b.title.as_str()))
            {
                return screen;
            }
        }
        let s = self.notices(
            ScreenBuilder::new("calibre-offline")
                .top_bar("Downloaded books")
                .owns_back(true),
        );
        if self.load == LoadState::Loading {
            return s.secondary("Loading downloaded books…").build();
        }
        if self.busy() {
            if self.download.is_some() {
                return s.button("offline-cancel", "Cancel download").build();
            }
            return s.secondary("Finishing the saved file…").build();
        }
        if self.load == LoadState::Blocked {
            return s.build();
        }
        let rows = self.rows(c);
        let pages = self.pages(c, &rows);
        let shown = self.shown(&pages);
        let mut s = self.shelf_prefix(s).rows(pages[shown].iter().map(|&index| {
            (
                format!("offline-{index}"),
                rows[index].0.clone(),
                rows[index].1.clone(),
                kobo_sdk::Glyph::Book,
            )
        }));
        if let Some(damaged) = self.damaged().filter(|index| pages[shown].contains(index)) {
            s = s.row_button(format!("offline-{damaged}"), "offline-repair", REPAIR_LABEL);
        }
        if pages.len() > 1 {
            s = s.page_turns("offline-prev", "offline-next").page_position(
                u16::try_from(shown + 1).unwrap_or(u16::MAX),
                u16::try_from(pages.len()).unwrap_or(u16::MAX),
            );
        }
        s.build()
    }
    /// The book that needs a new download. Its Download button sits on its
    /// own row, beside the words "Needs a new download", rather than as a
    /// full-width button above the list that pushed the books off a sideways
    /// panel under a notice.
    fn damaged(&self) -> Option<usize> {
        let repair = self.repair.as_ref()?;
        self.library
            .books
            .iter()
            .position(|book| book.file == repair.file)
    }
    /// What sits between the notices and the books: the sentence an empty
    /// shelf gets. Built once to draw and once to measure.
    fn shelf_prefix(&self, mut s: ScreenBuilder) -> ScreenBuilder {
        if self.library.books.is_empty() {
            s = s.text("No downloaded books. Open a book in your library and choose Download.");
        }
        s
    }
    /// Each book's title, cut to two lines at the width its row really
    /// has, and what is true of its file. A title is the library's own
    /// words, so a third line is possible, and one book of three lines under
    /// a banner was taller than a sideways panel at the largest size.
    fn rows(&self, c: &Context) -> Vec<(String, String)> {
        let damaged = self.damaged();
        self.library
            .books
            .iter()
            .enumerate()
            .map(|(index, book)| {
                let summary = if damaged == Some(index) {
                    "Needs a new download"
                } else if self.save_failed {
                    "Library changes not saved"
                } else {
                    "Available offline"
                };
                let title: String = book.title.chars().take(65).collect();
                (
                    c.clamped_row_with_edge(&title, 2, false, Self::edge(damaged, index)),
                    summary.to_owned(),
                )
            })
            .collect()
    }
    fn edge(damaged: Option<usize>, index: usize) -> RowEdge<'static> {
        if damaged == Some(index) {
            RowEdge::new().with_button(REPAIR_LABEL)
        } else {
            RowEdge::new()
        }
    }
    /// Books per screen, measured under the notices as they are drawn, with
    /// a retry's action line counted. A fixed two left a sideways panel at
    /// the larger text sizes with its second book and More books under the
    /// edge once a banner carried its retry.
    fn pages(&self, c: &Context, rows: &[(String, String)]) -> Vec<Vec<usize>> {
        let damaged = self.damaged();
        let rows: Vec<(&str, &str, RowEdge)> = rows
            .iter()
            .enumerate()
            .map(|(index, (title, summary))| {
                (title.as_str(), summary.as_str(), Self::edge(damaged, index))
            })
            .collect();
        let prefix =
            self.shelf_prefix(self.notices(
                ScreenBuilder::new("calibre-offline-measure").top_bar("Downloaded books"),
            ))
            .build();
        let pages = c.paginate_rows_with_edges_under(&rows, false, Position::AtTheFoot, &prefix);
        if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        }
    }
    /// The page that holds the first book shown last time.
    fn shown(&self, pages: &[Vec<usize>]) -> usize {
        pages
            .iter()
            .position(|page| page.contains(&self.offset))
            .unwrap_or(0)
    }
    fn busy(&self) -> bool {
        self.download.is_some()
            || self.upload.is_some()
            || self.local.is_some()
            || self.write.is_some()
    }
    pub fn download(
        &mut self,
        c: &mut Context,
        publication: &kobo_opds::Publication,
        root: &str,
        credential: Option<String>,
    ) {
        self.visible = true;
        if self.load != LoadState::Ready || self.load == LoadState::Blocked || self.busy() {
            self.say(
                BannerLevel::Attention,
                "Wait for the library to finish loading or saving.",
            );
            return;
        }
        let Some(link) = publication.best_acquisition() else {
            self.say(
                BannerLevel::Error,
                "No supported download is offered for this book.",
            );
            return;
        };
        if !catalog::allowed(root, &link.href) {
            self.say(
                BannerLevel::Error,
                "This download leaves your library. Its account cannot be sent to another server.",
            );
            return;
        }
        if link.length.is_some_and(|n| n > books::MAX_BYTES as u64) {
            self.say(
                BannerLevel::Error,
                "This book exceeds the 16 MB download limit.",
            );
            return;
        }
        if let Some(book) = self
            .library
            .books
            .iter()
            .find(|b| b.url == link.href)
            .cloned()
        {
            self.open_local(c, book);
            return;
        }
        if self.library.books.len() == books::MAX_BOOKS {
            self.say(
                BannerLevel::Error,
                "The downloaded library has reached its 64-book limit.",
            );
            return;
        }
        self.book = Some(Book {
            title: publication.title.clone(),
            authors: publication.authors.join(", "),
            url: link.href.clone(),
            file: if link
                .media_type
                .as_deref()
                .is_some_and(|m| m.starts_with("text/plain"))
            {
                "txt"
            } else {
                "epub"
            }
            .into(),
            digest: String::new(),
            size: 0,
            memory: String::new(),
        });
        self.credential = credential;
        self.bytes.clear();
        self.fetch(c);
    }
    pub fn repair(&mut self, c: &mut Context, root: &str, credential: Option<String>) {
        if self.busy() || self.load != LoadState::Ready {
            return;
        }
        let Some(mut book) = self.repair.clone() else {
            return;
        };
        if !catalog::allowed(root, &book.url) {
            self.say(
                BannerLevel::Error,
                "Connect the library this book came from before downloading it again.",
            );
            return;
        }
        book.file = if std::path::Path::new(&book.file)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
        {
            "epub"
        } else {
            "txt"
        }
        .into();
        self.book = Some(book);
        self.credential = credential;
        self.bytes.clear();
        self.fetch(c);
    }
    fn fetch(&mut self, c: &mut Context) {
        let Some(url) = self.book.as_ref().map(|book| book.url.clone()) else {
            return;
        };
        self.say(
            BannerLevel::Info,
            format!("Downloading · {} KB received", self.bytes.len() / 1024),
        );
        self.download = c.spawn(Task::Fetch {
            url,
            offset: u32::try_from(self.bytes.len()).expect("bounded download"),
            max_bytes: CHUNK,
            credential: self.credential.as_ref().map(Credential::basic),
            headers: Vec::new(),
        });
        if self.download.is_none() {
            self.say(
                BannerLevel::Error,
                "The download could not start. Return to the book and retry.",
            );
        }
    }
    pub fn task(&mut self, c: &mut Context, id: TaskId, outcome: &TaskOutcome) -> bool {
        if self.download != Some(id) {
            return self.view.woke(c, id, outcome) != kobo_bookview::Step::Elsewhere;
        }
        self.download = None;
        match outcome {
            TaskOutcome::Completed(bytes) => {
                if bytes.len() > CHUNK as usize
                    || self.bytes.len().saturating_add(bytes.len()) > books::MAX_BYTES
                {
                    self.say(
                        BannerLevel::Error,
                        "This book exceeds the 16 MB download limit.",
                    );
                    self.bytes.clear();
                    return true;
                }
                self.bytes.extend_from_slice(bytes);
                if bytes.len() == CHUNK as usize {
                    self.fetch(c);
                } else {
                    self.finish_download(c);
                }
            }
            TaskOutcome::Failed(error) => {
                let url = self.book.as_ref().map_or("", |book| book.url.as_str());
                let problem = catalog::problem(url, *error);
                self.say(problem.level(), problem.message);
                self.bytes.clear();
            }
            TaskOutcome::Cancelled => {
                self.say(BannerLevel::Info, "Download cancelled.");
                self.bytes.clear();
            }
        }
        true
    }
    fn finish_download(&mut self, c: &mut Context) {
        let Some(pending) = &self.book else {
            return;
        };
        let Ok(book) = Book::from_bytes(
            &pending.title,
            &pending.authors,
            &pending.url,
            &self.bytes,
            pending.file == "epub",
        ) else {
            self.say(
                BannerLevel::Error,
                "The downloaded book is empty or has invalid details.",
            );
            return;
        };
        if self
            .view
            .open_bytes(c, &book.file, &self.bytes, Memory::default())
            .is_err()
        {
            self.say(
                BannerLevel::Error,
                "This download is not a readable book. Check the library's file.",
            );
            self.bytes.clear();
            return;
        }
        self.view.close(c);
        let mut upload = ShelfUpload::new(&book.file, std::mem::take(&mut self.bytes));
        upload.start(c);
        self.upload = Some(upload);
        self.book = Some(book);
        self.say(BannerLevel::Info, "Saving book…");
    }
    fn save(&mut self, c: &mut Context) {
        if self.write.is_some()
            || self.save_failed
            || self.load == LoadState::Blocked
            || self.load != LoadState::Ready
        {
            return;
        }
        match self.library.encode() {
            Ok(bytes) if bytes != self.saved => {
                c.store().save(books::KEY, bytes.clone());
                self.write = Some(bytes);
            }
            Ok(_) => {}
            Err(_) => {
                self.save_failed = true;
                self.fail(
                    Retry::Save,
                    "Library changes are not saved. The saved library is kept.",
                );
            }
        }
    }
    fn open_local(&mut self, c: &mut Context, book: Book) {
        let mut local = ShelfDownload::new(&book.file).at_most(books::MAX_BYTES);
        local.start(c);
        self.local = Some(local);
        self.book = Some(book);
        self.say(BannerLevel::Info, "Opening saved book…");
    }
    fn loaded_result(&mut self, result: &StoreResult) -> bool {
        if let StoreResult::Loaded { key, value } = result {
            if key == books::KEY {
                if let Ok(library) = Library::restore(value.as_deref()) {
                    self.library = library;
                    self.saved = value.clone().unwrap_or_default();
                    self.load = LoadState::Ready;
                    self.notice = None;
                } else {
                    self.load = LoadState::Blocked;
                    self.fail(
                        Retry::Load,
                        "The saved library could not be read. It has been left untouched.",
                    );
                }
                return true;
            }
        }
        false
    }
    pub fn store(&mut self, c: &mut Context, result: &StoreResult) -> bool {
        if self.loaded_result(result) {
            return true;
        }
        if self.write.is_some() {
            match result {
                StoreResult::Saved { key } if key == books::KEY => {
                    self.saved = self.write.take().unwrap();
                    self.say(BannerLevel::Success, "Saved for offline reading.");
                    self.save(c);
                    return true;
                }
                StoreResult::Denied(_) => {
                    self.write = None;
                    self.save_failed = true;
                    self.fail(
                        Retry::Save,
                        "Library changes are not saved. Free some space, then retry saving.",
                    );
                    if let Some(reader) = self.view.reader_mut() {
                        reader.report("Reading changes are not saved. Close the book and choose Retry saving.");
                    }
                    return true;
                }
                _ => {}
            }
        }
        if let Some(upload) = &mut self.upload {
            match upload.advance(c, result) {
                ShelfProgress::Done => {
                    self.upload = None;
                    if let Some(book) = self.book.clone() {
                        if self.library.insert(book).is_ok() {
                            self.repair = None;
                            self.save(c);
                        } else {
                            self.save_failed = true;
                            self.fail(
                                Retry::Save,
                                "The book file was saved, but the library could not be updated.",
                            );
                        }
                    }
                    return true;
                }
                ShelfProgress::Failed(_) => {
                    self.upload = None;
                    self.say(
                        BannerLevel::Error,
                        "The book was not saved. Free some space and download again.",
                    );
                    return true;
                }
                ShelfProgress::Moving { .. } => return true,
                ShelfProgress::Elsewhere => {}
            }
        }
        if let Some(local) = &mut self.local {
            match local.advance(c, result) {
                ShelfProgress::Done => {
                    let bytes = self.local.take().unwrap().take();
                    if let Some(book) = &self.book {
                        if book.verifies(&bytes)
                            && self
                                .view
                                .open_bytes(
                                    c,
                                    &book.file,
                                    &bytes,
                                    Memory::decode(book.memory.as_bytes()),
                                )
                                .is_ok()
                        {
                            self.opened = true;
                            self.notice = None;
                        } else {
                            self.repair = Some(book.clone());
                            self.say(
                                BannerLevel::Error,
                                "The saved book is incomplete or unreadable. Download it again.",
                            );
                        }
                    }
                    return true;
                }
                ShelfProgress::Failed(_) => {
                    self.local = None;
                    self.repair.clone_from(&self.book);
                    self.say(
                        BannerLevel::Error,
                        "The saved book could not be loaded. Try opening it again.",
                    );
                    return true;
                }
                ShelfProgress::Moving { .. } => return true,
                ShelfProgress::Elsewhere => {}
            }
        }
        false
    }
    pub fn action(&mut self, c: &mut Context, action: ActionId) {
        if self.opened {
            match self.view.act(c, action) {
                None if action == ActionId::BACK => {
                    self.keep_position(c);
                    self.view.close(c);
                    self.opened = false;
                }
                Some(Outcome::Close) => {
                    self.keep_position(c);
                    self.view.close(c);
                    self.opened = false;
                }
                Some(Outcome::Save) => self.keep_position(c),
                Some(Outcome::Light(level)) => {
                    c.device().set_frontlight(level);
                    self.keep_position(c);
                }
                _ => {}
            }
            return;
        }
        if action == action_id("offline-cancel") {
            if let Some(task) = self.download.take() {
                c.cancel(task);
                self.bytes.clear();
                self.say(BannerLevel::Info, "Download cancelled.");
            }
            return;
        }
        if self.busy() {
            return;
        }
        if action == ActionId::BACK {
            self.visible = false;
        } else if action == action_id("offline-reload") {
            self.load = LoadState::Loading;
            self.start(c);
        } else if action == action_id("offline-save") {
            self.save_failed = false;
            self.save(c);
        } else {
            let rows = self.rows(c);
            let pages = self.pages(c, &rows);
            let shown = self.shown(&pages);
            if action == action_id("offline-prev") {
                if let Some(earlier) = shown.checked_sub(1) {
                    self.offset = pages[earlier].first().copied().unwrap_or(0);
                }
            } else if action == action_id("offline-next") {
                if let Some(later) = pages.get(shown + 1) {
                    self.offset = later.first().copied().unwrap_or(0);
                }
            } else if let Some(book) = pages[shown]
                .iter()
                .find(|&&i| action == action_id(&format!("offline-{i}")))
                .and_then(|&i| self.library.books.get(i).cloned())
            {
                self.open_local(c, book);
            }
        }
    }
    fn keep_position(&mut self, c: &mut Context) {
        if let (Some(book), Some(memory)) = (&self.book, self.view.memory()) {
            if let Some(saved) = self.library.books.iter_mut().find(|b| b.file == book.file) {
                saved.memory = String::from_utf8(memory.encode()).unwrap_or_default();
            }
        }
        self.save(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Reading {
        Reading {
            load: LoadState::Ready,
            ..Reading::default()
        }
    }
    fn text_book() -> kobo_opds::Publication {
        kobo_opds::parse(br#"{"metadata":{"title":"Library"},"publications":[{"metadata":{"title":"River","author":"Sample"},"links":[{"rel":"http://opds-spec.org/acquisition/open-access","href":"/river.txt","type":"text/plain"}]}]}"#, "https://library.test/opds").unwrap().publications.remove(0)
    }
    #[test]
    fn library_publishes_only_after_file_ack_and_retries_failed_metadata() {
        let mut reading = ready();
        let mut c = Context::default();
        reading.download(&mut c, &text_book(), "https://library.test/opds", None);
        let task = reading.download.unwrap();
        let bytes = b"A walk by the river.\n\nThe path follows the water.";
        reading.task(&mut c, task, &TaskOutcome::Completed(bytes.to_vec()));
        assert!(reading.library.books.is_empty());
        assert!(reading.upload.is_some());
        let file = reading.book.as_ref().unwrap().file.clone();
        reading.store(
            &mut c,
            &StoreResult::ShelfWritten {
                name: file.clone(),
                size: u32::try_from(bytes.len()).unwrap(),
            },
        );
        assert_eq!(reading.library.books.len(), 1);
        assert!(reading.write.is_some());
        reading.store(&mut c, &StoreResult::Denied(kobo_sdk::StoreError::TooFull));
        assert!(reading.save_failed);
        assert!(reading.saved.is_empty());
        reading.action(&mut c, action_id("offline-save"));
        let snapshot = reading.write.clone().unwrap();
        reading.store(
            &mut c,
            &StoreResult::Saved {
                key: books::KEY.into(),
            },
        );
        assert_eq!(reading.saved, snapshot);
        assert!(!reading.save_failed);
        let mut reopened = Reading::default();
        reopened.store(
            &mut c,
            &StoreResult::Loaded {
                key: books::KEY.into(),
                value: Some(snapshot),
            },
        );
        reopened.action(&mut c, action_id("offline-0"));
        reopened.store(
            &mut c,
            &StoreResult::ShelfRead {
                name: file,
                offset: 0,
                size: u32::try_from(bytes.len()).unwrap(),
                bytes: bytes.to_vec(),
            },
        );
        assert!(reopened.opened);
        assert!(reopened.download.is_none());
    }
    #[test]
    fn damaged_book_repair_preserves_old_index_until_replacement_is_acknowledged() {
        let mut reading = ready();
        let mut c = Context::default();
        let old = Book::from_bytes(
            "River",
            "Sample",
            "https://library.test/river.txt",
            b"Old book text",
            false,
        )
        .unwrap();
        reading.library.insert(old.clone()).unwrap();
        reading.open_local(&mut c, old.clone());
        reading.store(
            &mut c,
            &StoreResult::ShelfRead {
                name: old.file.clone(),
                offset: 0,
                size: 3,
                bytes: b"Bad".to_vec(),
            },
        );
        assert!(reading.repair.is_some());
        reading.repair(&mut c, "https://other.test/opds", None);
        assert!(reading.download.is_none());
        reading.repair(&mut c, "https://library.test/opds", None);
        let task = reading.download.unwrap();
        let bytes = b"A repaired book from the same library.";
        reading.task(&mut c, task, &TaskOutcome::Completed(bytes.to_vec()));
        assert_eq!(reading.library.books[0], old);
        let file = reading.book.as_ref().unwrap().file.clone();
        reading.store(
            &mut c,
            &StoreResult::ShelfWritten {
                name: file.clone(),
                size: u32::try_from(bytes.len()).unwrap(),
            },
        );
        assert_eq!(reading.library.books.len(), 1);
        assert_eq!(reading.library.books[0].file, file);
        assert!(reading.repair.is_none());
        assert!(reading.write.is_some());
    }
    /// Every notice the shelf can show, alone or under a standing failure,
    /// over a page of two books with More books under it: the most this
    /// screen ever holds.
    #[test]
    fn shelf_recovery_states_fit_all_text_sizes_with_actual_fonts() {
        for (text_scale, orientation) in kobo_ui::TextScale::STEPS.into_iter().flat_map(|scale| {
            [
                kobo_ui::Orientation::Portrait,
                kobo_ui::Orientation::Landscape,
            ]
            .map(|orientation| (scale, orientation))
        }) {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..kobo_ui::CLARA_BW_METRICS
            }
            .oriented(orientation);
            let runner =
                kobo_sdk::AppRunner::with_metrics(super::super::Calibre::default(), metrics);
            let context = runner.context();
            for books in [1, 3] {
                let mut reading = ready();
                let mut first = None;
                for index in 0..books {
                    let book = Book::from_bytes(
                        "A Walk by the River and Back Again by the Longer Path",
                        "Cobalt sample",
                        &format!("https://library.test/river-{index}.txt"),
                        format!("Text {index}").as_bytes(),
                        false,
                    )
                    .unwrap();
                    first.get_or_insert_with(|| book.clone());
                    reading.library.insert(book).unwrap();
                }
                let book = first.unwrap();
                for state in 0..6 {
                    reading.repair = matches!(state, 0 | 2).then(|| book.clone());
                    reading.save_failed = matches!(state, 1 | 2);
                    reading.load = if state == 3 {
                        LoadState::Blocked
                    } else {
                        LoadState::Ready
                    };
                    match state {
                        0 => reading.say(
                            BannerLevel::Error,
                            "The saved book is incomplete or unreadable. Download it again.",
                        ),
                        1 => reading.fail(
                            Retry::Save,
                            "Library changes are not saved. Free some space, then retry saving.",
                        ),
                        2 => reading.say(
                            BannerLevel::Error,
                            "This download leaves your library. Its account cannot be sent to another server.",
                        ),
                        3 => reading.fail(
                            Retry::Load,
                            "The saved library could not be read. It has been left untouched.",
                        ),
                        4 => reading.say(BannerLevel::Success, "Saved for offline reading."),
                        _ => reading.notice = None,
                    }
                    let pages = reading.pages(&context, &reading.rows(&context));
                    assert_eq!(
                        pages.concat(),
                        (0..books).collect::<Vec<_>>(),
                        "every book is on a page"
                    );
                    for page in &pages {
                        reading.offset = page.first().copied().unwrap_or(0);
                        let diagnostics = reading
                            .screen(&context)
                            .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                        assert!(
                            diagnostics.issues.is_empty(),
                            "{books} books, state {state}, page {page:?}, {text_scale:?} {orientation:?}: {:?}",
                            diagnostics.issues
                        );
                    }
                    reading.offset = 0;
                }
            }
        }
    }
    /// A banner's level, words, and the action and label it carries.
    type Drawn = (BannerLevel, String, Option<(ActionId, String)>);
    fn banners(screen: &Screen) -> Vec<Drawn> {
        screen
            .nodes
            .iter()
            .filter_map(|node| match node {
                kobo_sdk::Node::Banner {
                    level,
                    text,
                    action,
                    ..
                } => Some((
                    *level,
                    text.clone(),
                    action
                        .as_ref()
                        .map(|action| (action.action, action.label.clone())),
                )),
                _ => None,
            })
            .collect()
    }
    fn has_button(screen: &Screen, name: &str) -> bool {
        screen.nodes.iter().any(|node| {
            matches!(node, kobo_sdk::Node::Button { action, .. } if *action == action_id(name))
        })
    }
    #[test]
    fn a_saved_book_says_so_as_a_success_and_a_refused_write_is_an_error_carrying_retry_saving() {
        let mut reading = ready();
        let mut c = Context::default();
        reading.download(&mut c, &text_book(), "https://library.test/opds", None);
        let task = reading.download.unwrap();
        let bytes = b"A walk by the river.";
        reading.task(&mut c, task, &TaskOutcome::Completed(bytes.to_vec()));
        let file = reading.book.as_ref().unwrap().file.clone();
        reading.store(
            &mut c,
            &StoreResult::ShelfWritten {
                name: file,
                size: u32::try_from(bytes.len()).unwrap(),
            },
        );
        reading.store(&mut c, &StoreResult::Denied(kobo_sdk::StoreError::TooFull));
        let screen = reading.screen(&c);
        assert_eq!(
            banners(&screen),
            [(
                BannerLevel::Error,
                "Library changes are not saved. Free some space, then retry saving.".to_owned(),
                Some((action_id("offline-save"), "Retry saving".to_owned())),
            )]
        );
        assert!(!has_button(&screen, "offline-save"), "no loose retry");
        reading.action(&mut c, action_id("offline-save"));
        reading.store(
            &mut c,
            &StoreResult::Saved {
                key: books::KEY.into(),
            },
        );
        assert_eq!(
            banners(&reading.screen(&c)),
            [(
                BannerLevel::Success,
                "Saved for offline reading.".to_owned(),
                None
            )]
        );
    }
    #[test]
    fn a_downloaded_library_that_cannot_be_read_is_an_error_carrying_retry_loading() {
        let mut reading = Reading::default();
        let mut c = Context::default();
        reading.store(
            &mut c,
            &StoreResult::Loaded {
                key: books::KEY.into(),
                value: Some(b"corrupt".to_vec()),
            },
        );
        let screen = reading.screen(&c);
        assert_eq!(
            banners(&screen),
            [(
                BannerLevel::Error,
                "The saved library could not be read. It has been left untouched.".to_owned(),
                Some((action_id("offline-reload"), "Retry loading".to_owned())),
            )]
        );
        assert!(!has_button(&screen, "offline-reload"), "no loose retry");
        reading.action(&mut c, action_id("offline-reload"));
        assert!(reading.load == LoadState::Loading);
        assert!(
            banners(&reading.screen(&c))
                .iter()
                .all(|(.., action)| action.is_none()),
            "no retry while the load it would repeat is in flight"
        );
    }
    #[test]
    fn later_news_over_an_unsaved_library_ends_with_the_failure_so_retry_saving_sits_under_it() {
        let mut reading = ready();
        let mut c = Context::default();
        let book = Book::from_bytes(
            "River",
            "",
            "https://library.test/river.txt",
            b"Text",
            false,
        )
        .unwrap();
        reading.library.insert(book).unwrap();
        reading.save_failed = true;
        reading.fail(
            Retry::Save,
            "Library changes are not saved. Free some space, then retry saving.",
        );
        reading.download(&mut c, &text_book(), "https://other.test/opds", None);
        assert_eq!(
            banners(&reading.screen(&c)),
            [(
                BannerLevel::Error,
                "This download leaves your library. Its account cannot be sent to another server. \
                 Library changes are not saved."
                    .to_owned(),
                Some((action_id("offline-save"), "Retry saving".to_owned())),
            )]
        );
    }
    #[test]
    fn the_damaged_book_carries_its_download_on_its_own_row_and_a_page_holds_what_fits() {
        let metrics = kobo_ui::CLARA_BW_METRICS;
        let runner = kobo_sdk::AppRunner::with_metrics(super::super::Calibre::default(), metrics);
        let mut c = runner.context();
        let mut reading = ready();
        for index in 0..20 {
            let book = Book::from_bytes(
                "River",
                "",
                &format!("https://library.test/river-{index}.txt"),
                format!("Text {index}").as_bytes(),
                false,
            )
            .unwrap();
            reading.library.insert(book).unwrap();
        }
        reading.repair = Some(reading.library.books[1].clone());
        let screen = reading.screen(&c);
        let Some(kobo_sdk::Node::Rows { rows, .. }) = screen
            .nodes
            .iter()
            .find(|node| matches!(node, kobo_sdk::Node::Rows { .. }))
        else {
            panic!("the shelf lists its books");
        };
        assert!(rows.len() > 2, "a fixed two left most of the panel empty");
        let damaged = rows
            .iter()
            .find(|row| row.action == action_id("offline-1"))
            .expect("the damaged book is on the first page");
        assert_eq!(
            damaged.button.as_ref().map(|button| button.action),
            Some(action_id("offline-repair"))
        );
        assert!(
            !has_button(&screen, "offline-repair"),
            "no full-width button"
        );
        let first = reading.pages(&c, &reading.rows(&c));
        reading.action(&mut c, action_id("offline-next"));
        assert_eq!(
            Some(reading.offset),
            first.get(1).and_then(|page| page.first().copied())
        );
        reading.action(&mut c, action_id("offline-prev"));
        assert_eq!(reading.offset, 0);
    }
    #[test]
    fn original_epub_opens_with_restored_reading_position() {
        let bytes = include_bytes!("../fixtures/river.epub");
        let book = Book::from_bytes(
            "A Walk by the River",
            "Cobalt sample",
            "https://library.test/river.epub",
            bytes,
            true,
        )
        .unwrap();
        let mut reading = ready();
        let mut c = Context::default();
        let _runner = kobo_sdk::AppRunner::with_metrics(
            super::super::Calibre::default(),
            kobo_ui::CLARA_BW_METRICS,
        );
        reading.library.insert(book.clone()).unwrap();
        reading.open_local(&mut c, book.clone());
        reading.store(
            &mut c,
            &StoreResult::ShelfRead {
                name: book.file.clone(),
                offset: 0,
                size: u32::try_from(bytes.len()).unwrap(),
                bytes: bytes.to_vec(),
            },
        );
        assert!(reading.opened);
        let memory = Memory {
            at: 20,
            ..Memory::default()
        };
        assert!(reading.view.restore(&c, memory));
        reading.keep_position(&mut c);
        let encoded = reading.write.clone().unwrap();
        reading.store(
            &mut c,
            &StoreResult::Saved {
                key: books::KEY.into(),
            },
        );
        let mut reopened = Reading::default();
        reopened.store(
            &mut c,
            &StoreResult::Loaded {
                key: books::KEY.into(),
                value: Some(encoded),
            },
        );
        reopened.action(&mut c, action_id("offline-0"));
        reopened.store(
            &mut c,
            &StoreResult::ShelfRead {
                name: book.file,
                offset: 0,
                size: u32::try_from(bytes.len()).unwrap(),
                bytes: bytes.to_vec(),
            },
        );
        assert!(reopened.opened);
        assert_eq!(
            reopened.view.memory().unwrap().at,
            reading.view.memory().unwrap().at
        );
        assert!(reopened.view.memory().unwrap().at > 0);
        assert!(reopened.download.is_none());
        reopened.action(&mut c, ActionId::BACK);
        assert!(
            !reopened.opened,
            "top-bar Back must return to downloaded books"
        );
    }
    #[test]
    fn failed_file_write_does_not_publish_and_cancel_fences_late_download() {
        let mut reading = ready();
        let mut c = Context::default();
        reading.download(&mut c, &text_book(), "https://library.test/opds", None);
        let task = reading.download.unwrap();
        reading.action(&mut c, action_id("offline-cancel"));
        reading.task(
            &mut c,
            task,
            &TaskOutcome::Completed(b"Late bytes".to_vec()),
        );
        assert!(reading.upload.is_none());
        assert!(reading.library.books.is_empty());
        reading.download(&mut c, &text_book(), "https://library.test/opds", None);
        let task = reading.download.unwrap();
        reading.task(
            &mut c,
            task,
            &TaskOutcome::Completed(b"A short text book.".to_vec()),
        );
        reading.store(&mut c, &StoreResult::Denied(kobo_sdk::StoreError::TooFull));
        assert!(reading.library.books.is_empty());
        assert!(reading.write.is_none());
    }
    #[test]
    fn corrupt_library_blocks_new_writes_and_changed_file_fails_verification() {
        let mut reading = Reading::default();
        let mut c = Context::default();
        reading.store(
            &mut c,
            &StoreResult::Loaded {
                key: books::KEY.into(),
                value: Some(b"corrupt".to_vec()),
            },
        );
        reading.download(&mut c, &text_book(), "https://library.test/opds", None);
        assert!(reading.load == LoadState::Blocked);
        assert!(reading.download.is_none());
        assert!(reading.write.is_none());
        let mut reading = ready();
        let book = Book::from_bytes(
            "River",
            "",
            "https://library.test/river.txt",
            b"Text",
            false,
        )
        .unwrap();
        reading.library.insert(book.clone()).unwrap();
        reading.open_local(&mut c, book.clone());
        reading.store(
            &mut c,
            &StoreResult::ShelfRead {
                name: book.file,
                offset: 0,
                size: 4,
                bytes: b"Fake".to_vec(),
            },
        );
        assert!(!reading.opened);
        assert!(reading
            .notice
            .as_ref()
            .is_some_and(|notice| notice.text.contains("unreadable")));
    }
}
