//! Web articles kept whole on the device, and what is on the device said the
//! same way in every application.
//!
//! "Saved" used to mean three different things. Hacker News kept a mark and
//! nothing else, so a saved story opened to a network error on the train.
//! Preprints kept the words of a paper and fetched its figures again on every
//! reading. Daily Brief kept the headlines and not the stories behind them.
//! Each was a reasonable first step, and together they taught an owner that
//! "saved" could not be trusted.
//!
//! [`Keeper`] is the one meaning: an article is fetched, cut to its main
//! content, its pictures fetched beside it, and the whole of it written to the
//! shelf as one file. Only once that file and the index naming it have both
//! been acknowledged does the article count as kept, so a story marked
//! "Offline" is one that opens with the radio off.
//!
//! One file per article rather than one store key per piece, because an
//! application holds at most [`kobo_sdk::MAX_STORE_KEYS`] keys and the
//! previous design spent one on every article ever opened and one on every
//! picture, until the store refused to write the list of what had been read.

use crate::{picture_url, MAX_PICTURES, MAX_PICTURE_BYTES};
use kobo_sdk::{
    Context, Failure, ShelfDownload, ShelfProgress, ShelfUpload, StoreResult, Task, TaskId,
    TaskOutcome,
};
use std::collections::{BTreeMap, VecDeque};

/// The line an application's home screen carries about what it has kept.
///
/// An owner who takes a reader onto a train wants to know one thing before the
/// radio goes: what will still open. Every application answers with this
/// line, worded and rounded the same way, so the answer reads the same in
/// Feeds as it does in Gutenbird.
///
/// `one` and `many` are the noun for the things kept, such as `("story",
/// "stories")`. Nothing kept is said plainly rather than as "0 stories, 0 KB",
/// because a count of nothing is a sentence somebody has to work out.
#[must_use]
pub fn ready_offline(count: usize, bytes: u64, one: &str, many: &str) -> String {
    if count == 0 {
        return "Nothing is ready offline yet.".to_owned();
    }
    let noun = if count == 1 { one } else { many };
    format!("Ready offline: {count} {noun}, {}", size(bytes))
}

/// A size the way a person says it.
///
/// Whole kilobytes under a megabyte, one decimal place under ten megabytes,
/// and whole megabytes above that. Nobody choosing what to delete before a
/// trip is helped by a third digit.
#[must_use]
pub fn size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        return format!("{} KB", bytes.div_ceil(KB).max(1));
    }
    let tenths = (bytes * 10 + MB / 2) / MB;
    if tenths < 100 {
        format!("{}.{} MB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", (bytes + MB / 2) / MB)
    }
}

/// The most of a page that is fetched to keep.
///
/// A long feature is a few hundred kilobytes of markup and a paper with four
/// hundred references is most of a megabyte. Past this the page is mostly
/// somebody else's scripts, which the cut throws away anyway.
pub const PAGE_BYTES: u32 = 768 * 1024;

/// The most one kept article may weigh, pictures included.
///
/// Opening one reads the whole file into memory and hands each picture to the
/// reader, which holds its own copy until the picture is drawn, so this is
/// paid twice for a moment on a device with half a gigabyte for everything.
/// Eight megabytes holds a paper's four two-megabyte figures. Pictures past it
/// are left out and the page reads their captions instead.
pub const ARTICLE_BYTES: usize = 8 * 1024 * 1024;

/// The most articles one application keeps.
///
/// The index naming them is one store value, and this keeps it far inside the
/// store's ceiling on one value.
pub const MAX_KEPT: usize = 400;

/// The most all of one application's kept articles may weigh together.
///
/// Every application shares one card. A quarter of a gigabyte is the shelf's
/// own ceiling, and a reading application that took all of it would leave
/// nothing for the books.
pub const KEPT_BYTES: u64 = 96 * 1024 * 1024;

const MAGIC: &str = "kept-article-v1\n";
const INDEX_MAGIC: &str = "kept-index-v1\n";

/// What [`Keeper::keep_page`] and friends say when the device is the problem
/// rather than the network.
pub const NO_ROOM: &str =
    "This reader has no room to keep more for offline reading. Remove something kept, then try again.";

/// One article as it was kept: the cut markup, the address it came from, and
/// its pictures under the names the markup gives them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Kept {
    pub origin: String,
    pub html: String,
    pub pictures: Vec<(String, Vec<u8>)>,
}

impl Kept {
    /// The file this article is written to the shelf as.
    ///
    /// Plain lengths and bytes rather than an archive format. Nothing but this
    /// module reads it, it never leaves the device, and a format with no
    /// compression and no directory cannot be made to inflate into more than
    /// it was.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            self.html.len()
                + self
                    .pictures
                    .iter()
                    .map(|(_, bytes)| bytes.len())
                    .sum::<usize>(),
        );
        out.extend_from_slice(MAGIC.as_bytes());
        out.extend_from_slice(self.origin.replace('\n', "").as_bytes());
        out.push(b'\n');
        out.extend_from_slice(format!("{}\n", self.html.len()).as_bytes());
        out.extend_from_slice(self.html.as_bytes());
        for (name, bytes) in &self.pictures {
            if name.contains('\n') {
                continue;
            }
            out.extend_from_slice(name.as_bytes());
            out.push(b'\n');
            out.extend_from_slice(format!("{}\n", bytes.len()).as_bytes());
            out.extend_from_slice(bytes);
        }
        out
    }

    /// Reads a kept article back, or nothing when the file is not one whole.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut rest = bytes.strip_prefix(MAGIC.as_bytes())?;
        let origin = line(&mut rest)?;
        let html = std::str::from_utf8(sized(&mut rest)?).ok()?.to_owned();
        let mut pictures = Vec::new();
        while !rest.is_empty() {
            let name = line(&mut rest)?;
            pictures.push((name, sized(&mut rest)?.to_vec()));
        }
        Some(Self {
            origin,
            html,
            pictures,
        })
    }
}

fn line(rest: &mut &[u8]) -> Option<String> {
    let end = rest.iter().position(|byte| *byte == b'\n')?;
    let text = std::str::from_utf8(&rest[..end]).ok()?.to_owned();
    *rest = &rest[end + 1..];
    Some(text)
}

fn sized<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let length: usize = line(rest)?.parse().ok()?;
    let bytes = rest.get(..length)?;
    *rest = &rest[length..];
    Some(bytes)
}

/// What one answer meant to the keeper.
#[derive(Debug)]
pub enum Heard {
    /// Not the keeper's answer. Show it to whatever else is waiting.
    Elsewhere,
    /// The keeper's, and nothing an owner can see has changed.
    Quiet,
    /// What is kept changed: the index arrived, or an article finished or was
    /// removed and the device acknowledged it. Redraw anything that counts
    /// kept articles.
    Changed,
    /// A kept article was read back and can be opened.
    Opened(String, Kept),
    /// A kept article could not be read back. It has been taken off the
    /// index, so keeping it again is the way back.
    Unreadable(String),
    /// Keeping this article did not work, and this is why.
    Failed(String, &'static str),
}

#[derive(Debug)]
enum Source {
    Link(String),
    Markup { origin: String, html: String },
}

#[derive(Debug)]
struct Job {
    id: String,
    source: Source,
}

#[derive(Debug, Default)]
struct Running {
    id: String,
    origin: String,
    html: String,
    /// The page or picture fetch in flight.
    task: Option<TaskId>,
    /// Still waiting for the page itself.
    page: Option<String>,
    wanted: VecDeque<(String, String)>,
    fetching: Option<String>,
    pictures: Vec<(String, Vec<u8>)>,
    weight: usize,
    upload: Option<ShelfUpload>,
    size: u64,
}

/// Articles kept whole on the shelf, one file each, with an index of them in
/// the store.
///
/// The application routes four kinds of answer here and acts on what comes
/// back: store loads and saves by key, shelf answers by name, and finished
/// tasks. Work happens one article and one request at a time, because the
/// application's own fetches matter more than an article being put away for
/// later, and the runtime runs four at most.
#[derive(Debug)]
pub struct Keeper {
    key: String,
    prefix: String,
    /// Every article with a whole file on the shelf, and its size.
    index: BTreeMap<String, u64>,
    /// The index as the store last acknowledged it.
    saved: BTreeMap<String, u64>,
    loaded: bool,
    /// The index could not be read or written. Nothing more is written until
    /// [`Keeper::retry`] succeeds, so a file the device could not read is
    /// never replaced with a shorter one.
    pub failed: bool,
    writing: Option<Vec<u8>>,
    queue: VecDeque<Job>,
    job: Option<Running>,
    reading: Option<(String, ShelfDownload)>,
}

/// The store key a [`Keeper`] made with [`Default`] keeps its index under.
pub const INDEX_KEY: &str = "kept-v1";

/// The shelf prefix a [`Keeper`] made with [`Default`] names its files with.
pub const FILE_PREFIX: &str = "kept";

impl Default for Keeper {
    fn default() -> Self {
        Self::new(INDEX_KEY, FILE_PREFIX)
    }
}

impl Keeper {
    /// A keeper whose index lives under `key` and whose files are named from
    /// `prefix`. Both must be valid store names: lower-case letters, digits,
    /// dots, dashes and underscores.
    #[must_use]
    pub fn new(key: &str, prefix: &str) -> Self {
        Self {
            key: key.to_owned(),
            prefix: prefix.to_owned(),
            index: BTreeMap::new(),
            saved: BTreeMap::new(),
            loaded: false,
            failed: false,
            writing: None,
            queue: VecDeque::new(),
            job: None,
            reading: None,
        }
    }

    /// Asks for the index. Call once, at start.
    pub fn start(&self, context: &mut Context) {
        context.store().load(&self.key);
    }

    /// Whether the index has been read.
    #[must_use]
    pub const fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Whether this article is whole on the device and acknowledged.
    #[must_use]
    pub fn is_kept(&self, id: &str) -> bool {
        self.saved.contains_key(id)
    }

    /// Whether this article is waiting to be kept or being kept now.
    #[must_use]
    pub fn is_keeping(&self, id: &str) -> bool {
        self.job.as_ref().is_some_and(|job| job.id == id)
            || self.queue.iter().any(|job| job.id == id)
    }

    /// Whether anything is waiting to be kept or being kept now.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.job.is_some() || !self.queue.is_empty()
    }

    /// What one kept article weighs, once the device has acknowledged it.
    #[must_use]
    pub fn size(&self, id: &str) -> Option<u64> {
        self.saved.get(id).copied()
    }

    /// How many articles are kept.
    #[must_use]
    pub fn count(&self) -> usize {
        self.saved.len()
    }

    /// What every kept article weighs together.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.saved.values().sum()
    }

    /// The identities of every kept article.
    pub fn kept(&self) -> impl Iterator<Item = &str> {
        self.saved.keys().map(String::as_str)
    }

    /// The home screen's line about what is kept, in the shared wording.
    #[must_use]
    pub fn summary(&self, one: &str, many: &str) -> String {
        ready_offline(self.count(), self.bytes(), one, many)
    }

    /// Fetches the page at `url`, cuts it to its main content, fetches its
    /// pictures, and keeps the lot under `id`.
    ///
    /// Answers whether anything was queued: an article already kept or
    /// already on its way is left alone.
    pub fn keep_page(&mut self, context: &mut Context, id: &str, url: &str) -> bool {
        self.enqueue(context, id, Source::Link(url.to_owned()))
    }

    /// Keeps markup the application already has, such as an article carried
    /// whole in a feed, with the pictures it names fetched beside it.
    ///
    /// The markup is kept as given rather than cut, because a feed's article
    /// body has no page around it to cut away.
    pub fn keep_markup(
        &mut self,
        context: &mut Context,
        id: &str,
        origin: &str,
        html: &str,
    ) -> bool {
        self.enqueue(
            context,
            id,
            Source::Markup {
                origin: origin.to_owned(),
                html: html.to_owned(),
            },
        )
    }

    fn enqueue(&mut self, context: &mut Context, id: &str, source: Source) -> bool {
        // Refused at the door when the device is full, rather than queued
        // behind a room that is never going to come free: an article left
        // waiting would read "Saving" for ever.
        if id.is_empty() || self.index.contains_key(id) || self.is_keeping(id) || self.is_full() {
            return false;
        }
        self.queue.push_back(Job {
            id: id.to_owned(),
            source,
        });
        self.pump(context);
        true
    }

    /// Reads a kept article back. The answer arrives as [`Heard::Opened`] or
    /// [`Heard::Unreadable`].
    ///
    /// One at a time: opening a second article abandons the first read.
    pub fn open(&mut self, context: &mut Context, id: &str) -> bool {
        if !self.index.contains_key(id) {
            return false;
        }
        let mut read =
            ShelfDownload::new(self.file(id)).at_most(ARTICLE_BYTES + PAGE_BYTES as usize);
        read.start(context);
        self.reading = Some((id.to_owned(), read));
        true
    }

    /// Whether a kept article is being read back.
    #[must_use]
    pub fn is_opening(&self) -> bool {
        self.reading.is_some()
    }

    /// Stops waiting for an article being read back.
    pub fn stop_opening(&mut self) {
        self.reading = None;
    }

    /// Takes an article off the device, or stops it being kept.
    pub fn remove(&mut self, context: &mut Context, id: &str) {
        self.queue.retain(|job| job.id != id);
        if self.job.as_ref().is_some_and(|job| job.id == id) {
            if let Some(task) = self.job.take().and_then(|job| job.task) {
                context.cancel(task);
            }
        }
        if self.index.remove(id).is_some() {
            // The index first, then the file. A device that loses power in
            // between is left with a file nothing names, which costs space,
            // rather than a name with no file, which costs an owner the
            // article they were promised.
            self.flush(context);
            context.shelf().remove(self.file(id));
        }
        self.pump(context);
    }

    /// Takes off the device every article `keep` answers no for.
    ///
    /// For an application that keeps whatever is current, such as today's
    /// brief, so yesterday's stories do not pile up for ever.
    pub fn retain(&mut self, context: &mut Context, keep: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self
            .index
            .keys()
            .chain(self.queue.iter().map(|job| &job.id))
            .filter(|id| !keep(id))
            .cloned()
            .collect();
        for id in gone {
            self.remove(context, &id);
        }
    }

    /// Tries a failed index write again, or reads the index again when it
    /// could not be read.
    pub fn retry(&mut self, context: &mut Context) {
        if !self.loaded {
            self.start(context);
            return;
        }
        self.failed = false;
        self.flush(context);
        self.pump(context);
    }

    /// Starts the next piece of work, if one is waiting and a lane is free.
    ///
    /// Call after the application's own tasks finish: a lane the keeper could
    /// not have earlier may be free now.
    pub fn pump(&mut self, context: &mut Context) {
        if !self.loaded || self.failed {
            return;
        }
        if self.job.is_none() {
            let Some(next) = self.queue.pop_front() else {
                return;
            };
            if self.is_full() {
                // The device filled up while these waited. They are let go
                // rather than left queued, so nothing claims to be on its way
                // to a shelf with no room on it; the application asks again
                // once something has been removed.
                drop(next);
                self.queue.clear();
                return;
            }
            let mut job = Running {
                id: next.id,
                ..Running::default()
            };
            match next.source {
                Source::Link(url) => {
                    job.origin.clone_from(&url);
                    job.page = Some(url);
                }
                Source::Markup { origin, html } => {
                    job.wanted = wanted_pictures(&origin, &html);
                    job.origin = origin;
                    job.html = html;
                }
            }
            self.job = Some(job);
        }
        self.advance(context);
    }

    /// Whether the queue is stuck because the device is full.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.index.len() >= MAX_KEPT || self.index.values().sum::<u64>() >= KEPT_BYTES
    }

    fn advance(&mut self, context: &mut Context) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        if job.task.is_some() || job.upload.is_some() {
            return;
        }
        if let Some(url) = job.page.clone() {
            job.task = context.spawn(fetch(url, PAGE_BYTES));
            return;
        }
        if let Some((name, url)) = job.wanted.pop_front() {
            match context.spawn(fetch(url.clone(), MAX_PICTURE_BYTES)) {
                Some(task) => {
                    job.task = Some(task);
                    job.fetching = Some(name);
                }
                None => job.wanted.push_front((name, url)),
            }
            return;
        }
        let kept = Kept {
            origin: std::mem::take(&mut job.origin),
            html: std::mem::take(&mut job.html),
            pictures: std::mem::take(&mut job.pictures),
        };
        let bytes = kept.encode();
        job.size = bytes.len() as u64;
        let mut upload = ShelfUpload::new(shelf_name(&self.prefix, &job.id), bytes);
        upload.start(context);
        job.upload = Some(upload);
    }

    /// Takes a finished task, if it was one of the keeper's.
    pub fn task(&mut self, context: &mut Context, task: TaskId, outcome: &TaskOutcome) -> Heard {
        let Some(job) = self.job.as_mut().filter(|job| job.task == Some(task)) else {
            return Heard::Elsewhere;
        };
        job.task = None;
        if job.page.take().is_some() {
            match outcome {
                TaskOutcome::Completed(body) => {
                    job.html = kobo_doc::web::main_content(&String::from_utf8_lossy(body));
                    job.wanted = wanted_pictures(&job.origin, &job.html);
                }
                TaskOutcome::Failed(error) => {
                    let id = job.id.clone();
                    self.job = None;
                    self.pump(context);
                    return Heard::Failed(id, Failure::of(*error).advice);
                }
                TaskOutcome::Cancelled => {
                    self.job = None;
                    self.pump(context);
                    return Heard::Quiet;
                }
            }
        } else if let Some(name) = job.fetching.take() {
            match outcome {
                TaskOutcome::Completed(bytes) => {
                    if kobo_image::size(bytes).is_ok() && job.weight + bytes.len() <= ARTICLE_BYTES
                    {
                        job.weight += bytes.len();
                        job.pictures.push((name, bytes.clone()));
                    }
                }
                // The radio went, or the site stopped answering, part way
                // through. Writing the article now would keep it without the
                // pictures that were still to come and call it kept, and a
                // kept article is never fetched again. So nothing is written,
                // and the application asks again later.
                TaskOutcome::Failed(error) if error.worth_retrying() => {
                    let id = job.id.clone();
                    self.job = None;
                    if *error == kobo_sdk::TaskError::Offline {
                        // Everything still waiting would fail the same way.
                        self.queue.clear();
                    }
                    self.pump(context);
                    return Heard::Failed(id, Failure::of(*error).advice);
                }
                // A picture that is too large, or that the site will not
                // give, is a picture the page reads without, exactly as it
                // would online, so it does not fail the article.
                TaskOutcome::Failed(_) | TaskOutcome::Cancelled => {}
            }
        }
        self.advance(context);
        Heard::Quiet
    }

    /// Takes a store load, if it was the index.
    pub fn loaded(&mut self, context: &mut Context, key: &str, result: &StoreResult) -> Heard {
        if key != self.key {
            return Heard::Elsewhere;
        }
        match result {
            StoreResult::Loaded { value, .. } => {
                match decode_index(value.as_deref().unwrap_or_default()) {
                    Some(index) => {
                        self.saved.clone_from(&index);
                        self.index = index;
                        self.loaded = true;
                        self.failed = false;
                        self.pump(context);
                    }
                    None => self.failed = true,
                }
            }
            _ => self.failed = true,
        }
        Heard::Changed
    }

    /// Takes a store save, if it was the index.
    pub fn saved(&mut self, context: &mut Context, key: &str, result: &StoreResult) -> Heard {
        if key != self.key {
            return Heard::Elsewhere;
        }
        let Some(bytes) = self.writing.take() else {
            return Heard::Quiet;
        };
        if matches!(result, StoreResult::Saved { .. }) {
            // What was written is what this decodes, so the fallback is never
            // reached; it is here because a miscount of saved articles is not
            // worth ending the application over.
            self.saved = decode_index(&bytes).unwrap_or_default();
            self.flush(context);
        } else {
            self.failed = true;
        }
        Heard::Changed
    }

    /// Takes a shelf answer, if it was for one of the keeper's files.
    pub fn shelf(&mut self, context: &mut Context, name: &str, result: &StoreResult) -> Heard {
        if !name.starts_with(&self.prefix) {
            return Heard::Elsewhere;
        }
        if let Some((id, read)) = self
            .reading
            .as_mut()
            .filter(|(_, read)| read.name() == name)
        {
            let id = id.clone();
            match read.advance(context, result) {
                ShelfProgress::Moving { .. } => return Heard::Quiet,
                ShelfProgress::Done => {
                    let bytes = self.reading.take().map(|(_, read)| read.take());
                    if let Some(kept) = bytes.as_deref().and_then(Kept::decode) {
                        return Heard::Opened(id, kept);
                    }
                    self.forget_unreadable(context, &id);
                    return Heard::Unreadable(id);
                }
                ShelfProgress::Failed(_) => {
                    self.reading = None;
                    self.forget_unreadable(context, &id);
                    return Heard::Unreadable(id);
                }
                ShelfProgress::Elsewhere => {}
            }
        }
        let Some(job) = self.job.as_mut() else {
            return Heard::Quiet;
        };
        let Some(upload) = job.upload.as_mut().filter(|upload| upload.name() == name) else {
            return Heard::Quiet;
        };
        match upload.advance(context, result) {
            ShelfProgress::Moving { .. } | ShelfProgress::Elsewhere => Heard::Quiet,
            ShelfProgress::Done => {
                let (id, size) = (job.id.clone(), job.size);
                self.job = None;
                self.index.insert(id, size);
                self.flush(context);
                self.pump(context);
                Heard::Quiet
            }
            ShelfProgress::Failed(_) => {
                let id = job.id.clone();
                self.job = None;
                self.pump(context);
                Heard::Failed(id, NO_ROOM)
            }
        }
    }

    /// A file that would not read is taken off the index, so the article
    /// stops claiming to be on the device and can be kept again.
    fn forget_unreadable(&mut self, context: &mut Context, id: &str) {
        if self.index.remove(id).is_some() {
            self.flush(context);
            context.shelf().remove(self.file(id));
        }
    }

    fn flush(&mut self, context: &mut Context) {
        if !self.loaded || self.failed || self.writing.is_some() || self.index == self.saved {
            return;
        }
        let bytes = encode_index(&self.index);
        context.store().save(&self.key, bytes.clone());
        self.writing = Some(bytes);
    }

    fn file(&self, id: &str) -> String {
        shelf_name(&self.prefix, id)
    }
}

/// The shelf name an article is kept under.
///
/// A digest of the identity rather than the identity, because identities are
/// whatever the application uses -- an arXiv number with a slash in it, a
/// story number, a digest of a feed item -- and shelf names allow none of the
/// characters that would make that a problem.
fn shelf_name(prefix: &str, id: &str) -> String {
    let digest = kobo_net::sha256::hex_digest(id.as_bytes());
    format!("{prefix}-{}", &digest[..40])
}

fn fetch(url: String, max_bytes: u32) -> Task {
    Task::Fetch {
        url,
        offset: 0,
        max_bytes,
        credential: None,
        headers: Vec::new(),
    }
}

/// Every picture the markup names that may be asked for, with its address.
///
/// The same rule the reader uses when it fetches pictures for a page being
/// read: only the page's own host. A kept article must not phone anybody the
/// open one would not.
fn wanted_pictures(origin: &str, html: &str) -> VecDeque<(String, String)> {
    let mut wanted = VecDeque::new();
    for name in kobo_doc::web::picture_sources(html) {
        if wanted.len() >= MAX_PICTURES {
            break;
        }
        if wanted.iter().any(|(known, _)| *known == name) {
            continue;
        }
        if let Some(url) = picture_url(origin, &name) {
            wanted.push_back((name, url));
        }
    }
    wanted
}

fn encode_index(index: &BTreeMap<String, u64>) -> Vec<u8> {
    let mut out = String::from(INDEX_MAGIC);
    for (id, size) in index {
        out.push_str(id);
        out.push('\t');
        out.push_str(&size.to_string());
        out.push('\n');
    }
    out.into_bytes()
}

fn decode_index(bytes: &[u8]) -> Option<BTreeMap<String, u64>> {
    if bytes.is_empty() {
        return Some(BTreeMap::new());
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut index = BTreeMap::new();
    for line in text.strip_prefix(INDEX_MAGIC)?.lines() {
        let (id, size) = line.split_once('\t')?;
        if id.is_empty() || index.insert(id.to_owned(), size.parse().ok()?).is_some() {
            return None;
        }
    }
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobo_sdk::{Command, StoreError, StoreRequest};

    #[test]
    fn nothing_kept_is_said_as_a_sentence_rather_than_a_count_of_zero() {
        assert_eq!(
            ready_offline(0, 0, "story", "stories"),
            "Nothing is ready offline yet."
        );
    }

    #[test]
    fn one_thing_kept_takes_the_singular() {
        assert_eq!(
            ready_offline(1, 300 * 1024, "story", "stories"),
            "Ready offline: 1 story, 300 KB"
        );
    }

    #[test]
    fn sizes_are_rounded_the_way_a_person_would_say_them() {
        assert_eq!(size(1), "1 KB");
        assert_eq!(size(1023 * 1024), "1023 KB");
        assert_eq!(size(4 * 1024 * 1024 + 200 * 1024), "4.2 MB");
        assert_eq!(size(42 * 1024 * 1024 + 700 * 1024), "43 MB");
        assert_eq!(
            ready_offline(12, 4 * 1024 * 1024, "item", "items"),
            "Ready offline: 12 items, 4.0 MB"
        );
    }

    #[test]
    fn a_kept_article_reads_back_exactly_as_it_was_written() {
        let kept = Kept {
            origin: "https://example.com/story".into(),
            html: "<p>Words\nacross lines.</p>".into(),
            pictures: vec![
                ("a.png".into(), vec![1, 2, 3]),
                ("b.png".into(), Vec::new()),
            ],
        };
        assert_eq!(Kept::decode(&kept.encode()), Some(kept.clone()));
        let bytes = kept.encode();
        assert_eq!(Kept::decode(&bytes[..bytes.len() - 1]), None);
    }

    fn png() -> Vec<u8> {
        kobo_image::encode_png_grey(2, 2, &[0, 64, 128, 255]).expect("a picture encodes")
    }

    fn spawned_url(context: &Context) -> Option<String> {
        context
            .commands()
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Spawn {
                    work: Task::Fetch { url, .. },
                    ..
                } => Some(url.clone()),
                _ => None,
            })
    }

    fn spawned_task(context: &Context) -> TaskId {
        context
            .commands()
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Spawn { task, .. } => Some(*task),
                _ => None,
            })
            .expect("a task was spawned")
    }

    fn uploaded(context: &Context) -> Option<(String, Vec<u8>)> {
        context
            .commands()
            .iter()
            .rev()
            .find_map(|command| match command {
                Command::Store(StoreRequest::ShelfWrite { name, bytes, .. }) => {
                    Some((name.clone(), bytes.clone()))
                }
                _ => None,
            })
    }

    fn loaded_keeper(context: &mut Context) -> Keeper {
        let mut keeper = Keeper::new("kept-v1", "kept");
        keeper.start(context);
        keeper.loaded(
            context,
            "kept-v1",
            &StoreResult::Loaded {
                key: "kept-v1".into(),
                value: None,
            },
        );
        keeper
    }

    #[test]
    fn keeping_a_page_cuts_it_fetches_its_pictures_and_counts_only_once_acknowledged() {
        let mut context = Context::default();
        let mut keeper = loaded_keeper(&mut context);
        assert!(keeper.keep_page(&mut context, "42", "https://example.com/post"));
        assert!(!keeper.keep_page(&mut context, "42", "https://example.com/post"));
        assert_eq!(
            spawned_url(&context).as_deref(),
            Some("https://example.com/post")
        );
        let page = spawned_task(&context);
        let body = b"<nav>Menu</nav><article><p>Words.</p><img src=\"x.png\" alt=\"A plot\">\
            <img src=\"https://elsewhere.net/y.png\"></article>"
            .to_vec();
        keeper.task(&mut context, page, &TaskOutcome::Completed(body));
        assert_eq!(
            spawned_url(&context).as_deref(),
            Some("https://example.com/x.png")
        );
        let picture = spawned_task(&context);
        keeper.task(&mut context, picture, &TaskOutcome::Completed(png()));
        let (name, bytes) = uploaded(&context).expect("the article was written");
        let kept = Kept::decode(&bytes).expect("the file is a whole article");
        assert!(!kept.html.contains("Menu"));
        assert_eq!(kept.pictures, vec![("x.png".to_owned(), png())]);
        assert!(!keeper.is_kept("42"));
        keeper.shelf(
            &mut context,
            &name,
            &StoreResult::ShelfWritten {
                name: name.clone(),
                size: u32::try_from(bytes.len()).unwrap(),
            },
        );
        assert!(!keeper.is_kept("42"), "the index has not been acknowledged");
        keeper.saved(
            &mut context,
            "kept-v1",
            &StoreResult::Saved {
                key: "kept-v1".into(),
            },
        );
        assert!(keeper.is_kept("42"));
        assert_eq!(keeper.count(), 1);
        assert_eq!(keeper.bytes(), bytes.len() as u64);
    }

    #[test]
    fn a_page_that_will_not_come_says_why_and_keeps_nothing() {
        let mut context = Context::default();
        let mut keeper = loaded_keeper(&mut context);
        keeper.keep_page(&mut context, "7", "https://example.com/a");
        let page = spawned_task(&context);
        let heard = keeper.task(
            &mut context,
            page,
            &TaskOutcome::Failed(kobo_sdk::TaskError::Offline),
        );
        assert!(matches!(heard, Heard::Failed(id, _) if id == "7"));
        assert!(!keeper.is_keeping("7"));
        assert_eq!(keeper.count(), 0);
    }

    #[test]
    fn losing_the_radio_part_way_keeps_nothing_rather_than_an_article_without_its_pictures() {
        let mut context = Context::default();
        let mut keeper = loaded_keeper(&mut context);
        keeper.keep_markup(
            &mut context,
            "1",
            "https://example.com/a",
            "<p>Words.</p><img src=\"x.png\"><img src=\"y.png\">",
        );
        keeper.keep_markup(
            &mut context,
            "2",
            "https://example.com/b",
            "<img src=\"z.png\">",
        );
        let first = spawned_task(&context);
        let heard = keeper.task(
            &mut context,
            first,
            &TaskOutcome::Failed(kobo_sdk::TaskError::Offline),
        );
        assert!(matches!(heard, Heard::Failed(id, _) if id == "1"));
        assert!(
            uploaded(&context).is_none(),
            "an article was written without its pictures"
        );
        assert!(
            !keeper.is_busy(),
            "the rest of the queue was left to fail one by one"
        );
    }

    #[test]
    fn a_kept_article_that_will_not_read_back_stops_claiming_to_be_kept() {
        let mut context = Context::default();
        let mut keeper = Keeper::new("kept-v1", "kept");
        keeper.loaded(
            &mut context,
            "kept-v1",
            &StoreResult::Loaded {
                key: "kept-v1".into(),
                value: Some(encode_index(&BTreeMap::from([("9".to_owned(), 10)]))),
            },
        );
        assert!(keeper.is_kept("9"));
        assert!(keeper.open(&mut context, "9"));
        let name = keeper.file("9");
        let heard = keeper.shelf(
            &mut context,
            &name,
            &StoreResult::Denied(StoreError::Missing),
        );
        assert!(matches!(heard, Heard::Unreadable(id) if id == "9"));
        keeper.saved(
            &mut context,
            "kept-v1",
            &StoreResult::Saved {
                key: "kept-v1".into(),
            },
        );
        assert!(!keeper.is_kept("9"));
    }

    #[test]
    fn an_index_the_device_cannot_read_is_never_written_over() {
        let mut context = Context::default();
        let mut keeper = Keeper::new("kept-v1", "kept");
        keeper.loaded(
            &mut context,
            "kept-v1",
            &StoreResult::Loaded {
                key: "kept-v1".into(),
                value: Some(b"a future format".to_vec()),
            },
        );
        assert!(keeper.failed);
        keeper.keep_page(&mut context, "1", "https://example.com/");
        assert!(spawned_url(&context).is_none());
    }

    #[test]
    fn a_full_device_refuses_a_new_article_rather_than_leaving_it_waiting() {
        let mut context = Context::default();
        let mut keeper = Keeper::new("kept-v1", "kept");
        let full: BTreeMap<String, u64> = (0..MAX_KEPT).map(|n| (n.to_string(), 1)).collect();
        keeper.loaded(
            &mut context,
            "kept-v1",
            &StoreResult::Loaded {
                key: "kept-v1".into(),
                value: Some(encode_index(&full)),
            },
        );
        assert!(keeper.is_full());
        assert!(!keeper.keep_page(&mut context, "new", "https://example.com/"));
        assert!(!keeper.is_keeping("new"));
        assert_eq!(keeper.size("0"), Some(1));
    }

    #[test]
    fn retaining_today_takes_yesterday_off_the_device() {
        let mut context = Context::default();
        let mut keeper = Keeper::new("kept-v1", "kept");
        keeper.loaded(
            &mut context,
            "kept-v1",
            &StoreResult::Loaded {
                key: "kept-v1".into(),
                value: Some(encode_index(&BTreeMap::from([
                    ("old".to_owned(), 10),
                    ("new".to_owned(), 20),
                ]))),
            },
        );
        keeper.retain(&mut context, |id| id == "new");
        let removed: Vec<_> = context
            .commands()
            .iter()
            .filter_map(|command| match command {
                Command::Store(StoreRequest::ShelfRemove { name }) => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(removed, vec![keeper.file("old")]);
        keeper.saved(
            &mut context,
            "kept-v1",
            &StoreResult::Saved {
                key: "kept-v1".into(),
            },
        );
        assert_eq!(keeper.kept().collect::<Vec<_>>(), vec!["new"]);
    }
}
