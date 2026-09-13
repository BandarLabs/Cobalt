//! Download, verified local reopening, and serialized library/reading-state writes.
use super::{
    books::{self, Book, Library},
    catalog,
};
use kobo_bookview::BookView;
use kobo_read::{Memory, Outcome};
use kobo_sdk::{
    action_id, ActionId, Context, Credential, Screen, ScreenBuilder, ShelfDownload, ShelfProgress,
    ShelfUpload, StoreResult, Task, TaskId, TaskOutcome,
};
const CHUNK: u32 = 256 * 1024;
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
    message: String,
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
        self.message =
            "The downloaded library could not be loaded. Retry to keep your saved books.".into();
    }
    pub fn screen(&self) -> Screen {
        if self.opened {
            if let Some(screen) = self
                .view
                .screen(self.book.as_ref().map_or("Book", |b| b.title.as_str()))
            {
                return screen;
            }
        }
        let mut s = ScreenBuilder::new("calibre-offline")
            .top_bar("Downloaded books")
            .owns_back(true);
        if !self.message.is_empty() {
            s = s.secondary(&self.message);
        }
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
            return s.button("offline-reload", "Retry loading").build();
        }
        if self.repair.is_some() {
            s = s.button("offline-repair", "Download again");
        }
        if self.save_failed {
            s = s.button("offline-save", "Retry saving");
        }
        if self.library.books.is_empty() {
            s = s.text("No downloaded books. Open a book in your library and choose Download.");
        }
        for (i, book) in self
            .library
            .books
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(2)
        {
            let title: String = book.title.chars().take(65).collect();
            s = s.rows([(
                format!("offline-{i}"),
                title,
                if self
                    .repair
                    .as_ref()
                    .is_some_and(|bad| bad.file == book.file)
                {
                    "Needs a new download".to_owned()
                } else if self.save_failed {
                    "Library changes not saved".to_owned()
                } else {
                    "Available offline".to_owned()
                },
                kobo_sdk::Glyph::Book,
            )]);
        }
        if self.offset > 0 {
            s = s.button("offline-prev", "Previous books");
        }
        if self.offset + 2 < self.library.books.len() {
            s = s.button("offline-next", "More books");
        }
        s.build()
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
            self.message = "Wait for the library to finish loading or saving.".into();
            return;
        }
        let Some(link) = publication.best_acquisition() else {
            self.message = "No supported download is offered for this book.".into();
            return;
        };
        if !catalog::allowed(root, &link.href) {
            self.message =
                "This download leaves your library. Its account cannot be sent to another server."
                    .into();
            return;
        }
        if link.length.is_some_and(|n| n > books::MAX_BYTES as u64) {
            self.message = "This book exceeds the 16 MB download limit.".into();
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
            self.message = "The downloaded library has reached its 64-book limit.".into();
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
            self.message =
                "Connect the library this book came from before downloading it again.".into();
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
        let Some(book) = &self.book else {
            return;
        };
        self.message = format!("Downloading · {} KB received", self.bytes.len() / 1024);
        self.download = c.spawn(Task::Fetch {
            url: book.url.clone(),
            offset: u32::try_from(self.bytes.len()).expect("bounded download"),
            max_bytes: CHUNK,
            credential: self.credential.as_ref().map(Credential::basic),
            headers: Vec::new(),
        });
        if self.download.is_none() {
            self.message = "The download could not start. Return to the book and retry.".into();
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
                    self.message = "This book exceeds the 16 MB download limit.".into();
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
                self.message = catalog::failure(*error);
                self.bytes.clear();
            }
            TaskOutcome::Cancelled => {
                self.message = "Download cancelled.".into();
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
            self.message = "The downloaded book is empty or has invalid details.".into();
            return;
        };
        if self
            .view
            .open_bytes(c, &book.file, &self.bytes, Memory::default())
            .is_err()
        {
            self.message = "This download is not a readable book. Check the library's file.".into();
            self.bytes.clear();
            return;
        }
        self.view.close(c);
        let mut upload = ShelfUpload::new(&book.file, std::mem::take(&mut self.bytes));
        upload.start(c);
        self.upload = Some(upload);
        self.book = Some(book);
        self.message = "Saving book…".into();
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
                self.message = "Library changes are not saved. The saved library is kept.".into();
            }
        }
    }
    fn open_local(&mut self, c: &mut Context, book: Book) {
        let mut local = ShelfDownload::new(&book.file).at_most(books::MAX_BYTES);
        local.start(c);
        self.local = Some(local);
        self.book = Some(book);
        self.message = "Opening saved book…".into();
    }
    fn loaded_result(&mut self, result: &StoreResult) -> bool {
        if let StoreResult::Loaded { key, value } = result {
            if key == books::KEY {
                if let Ok(library) = Library::restore(value.as_deref()) {
                    self.library = library;
                    self.saved = value.clone().unwrap_or_default();
                    self.load = LoadState::Ready;
                    self.message.clear();
                } else {
                    self.load = LoadState::Blocked;
                    self.message =
                        "The saved library could not be read. It has been left untouched.".into();
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
                    self.message = "Saved for offline reading.".into();
                    self.save(c);
                    return true;
                }
                StoreResult::Denied(_) => {
                    self.write = None;
                    self.save_failed = true;
                    self.message =
                        "Library changes are not saved. Free some space, then retry saving.".into();
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
                            self.message =
                                "The book file was saved, but the library could not be updated."
                                    .into();
                        }
                    }
                    return true;
                }
                ShelfProgress::Failed(_) => {
                    self.upload = None;
                    self.message =
                        "The book was not saved. Free some space and download again.".into();
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
                            self.message.clear();
                        } else {
                            self.repair = Some(book.clone());
                            self.message =
                                "The saved book is incomplete or unreadable. Download it again."
                                    .into();
                        }
                    }
                    return true;
                }
                ShelfProgress::Failed(_) => {
                    self.local = None;
                    self.repair.clone_from(&self.book);
                    self.message =
                        "The saved book could not be loaded. Try opening it again.".into();
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
                self.message = "Download cancelled.".into();
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
        } else if action == action_id("offline-prev") {
            self.offset = self.offset.saturating_sub(2);
        } else if action == action_id("offline-next") && self.offset + 2 < self.library.books.len()
        {
            self.offset += 2;
        } else if let Some(book) = self
            .library
            .books
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(2)
            .find(|(i, _)| action == action_id(&format!("offline-{i}")))
            .map(|(_, b)| b.clone())
        {
            self.open_local(c, book);
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
    #[test]
    fn shelf_recovery_states_fit_all_text_sizes_with_actual_fonts() {
        for text_scale in kobo_ui::TextScale::STEPS {
            let metrics = kobo_sdk::DisplayMetrics {
                text_scale,
                ..kobo_ui::CLARA_BW_METRICS
            };
            let _runner =
                kobo_sdk::AppRunner::with_metrics(super::super::Calibre::default(), metrics);
            let mut reading = ready();
            let book = Book::from_bytes(
                "A Walk by the River",
                "Cobalt sample",
                "https://library.test/river.txt",
                b"Text",
                false,
            )
            .unwrap();
            reading.library.insert(book.clone()).unwrap();
            for state in 0..4 {
                reading.repair = (state == 1).then(|| book.clone());
                reading.save_failed = state == 2;
                reading.load = if state == 3 {
                    LoadState::Blocked
                } else {
                    LoadState::Ready
                };
                reading.message =
                    "The saved book could not be opened. Your library is kept.".into();
                let diagnostics = reading
                    .screen()
                    .diagnostics(&metrics, &kobo_ui::Chrome::measuring(true));
                assert!(
                    diagnostics.issues.is_empty(),
                    "{state} {text_scale:?}: {:?}",
                    diagnostics.issues
                );
            }
        }
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
        assert!(reading.message.contains("unreadable"));
    }
}
