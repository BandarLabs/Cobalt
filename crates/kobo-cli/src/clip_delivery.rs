//! Getting a clipped article onto the reader, now or when it is next awake.
//!
//! An article lands in the card's `Articles` folder, where Books and the
//! stock reader both find books, and a notice record follows it into the
//! runtime's inbox so the reader says it arrived. Both travel over the same
//! SSH session `kobo send` uses; the simulator gets the same two files in its
//! own folders.
//!
//! A reader that is asleep or away is the usual case, not an error, so an
//! article that cannot be delivered waits in a queue on this computer. The
//! queue is a folder of EPUBs, each with a small record beside it, and is
//! emptied by `kobo clip sync`, by the next clip, and by Sync on the reader,
//! which asks the Sidekick helper to run the same sync back to it.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The folder on the card articles are delivered to. Books recognises an
/// article by it, and the runtime deletes only inside it.
pub const ARTICLES: &str = "Articles";

/// Where the runtime reads notice records from, on the reader.
const DEVICE_INBOX: &str = "/mnt/onboard/.adds/cobalt/state/notices/inbox";
const DEVICE_ARTICLES: &str = "/mnt/onboard/Articles";

/// How long one article's copy may take. A book is at most a few megabytes
/// over the reader's Wi-Fi.
const COPY_PATIENCE: Duration = Duration::from_secs(120);

/// A delivery that started this long ago and never finished was a process
/// that died, and its claim on the queue is let go.
const STALE_CLAIM: Duration = Duration::from_secs(15 * 60);

/// An article ready to go to a reader.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Article {
    pub title: String,
    /// The site it came from, which the notice names under the title.
    pub site: Option<String>,
    /// The file name it is saved under, before any number a clash adds.
    pub file_name: String,
    pub epub: Vec<u8>,
}

/// Where an article goes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reader {
    Simulator,
    Device(String),
}

impl Reader {
    /// The reader a target names, in words for the owner.
    pub fn label(&self) -> String {
        match self {
            Self::Simulator => "the simulator".to_owned(),
            Self::Device(host) => format!("the reader at {host}"),
        }
    }
}

/// Why an article did not land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Missed {
    /// Nothing answered: the reader is asleep, away or off Wi-Fi. The
    /// article waits and goes next time.
    Away(String),
    /// The reader answered and would not take it, such as a full card.
    Refused(String),
}

/// Copies one article to `reader` and announces it there.
///
/// Returns the name it was saved under on the reader, which is the title's
/// file name or the first numbered one free, so an article never replaces
/// another with the same title.
pub fn deliver(reader: &Reader, article: &Article) -> Result<String, Missed> {
    match reader {
        Reader::Simulator => deliver_to_folders(
            &kobo_sim::simulated_library_root(),
            &kobo_sim::simulated_notices_folder().join("inbox"),
            article,
        )
        .map_err(Missed::Refused),
        Reader::Device(host) => deliver_to_device(host, article),
    }
}

/// The simulator's half: the card and the inbox are folders here.
pub(crate) fn deliver_to_folders(
    card: &Path,
    inbox: &Path,
    article: &Article,
) -> Result<String, String> {
    let folder = card.join(ARTICLES);
    fs::create_dir_all(&folder)
        .and_then(|()| fs::create_dir_all(inbox))
        .map_err(|error| format!("could not make the articles folder: {error}"))?;
    let stem = stem_of(&article.file_name);
    let partial = folder.join(format!(".{}.writing", unique()));
    fs::write(&partial, &article.epub)
        .map_err(|error| format!("could not write the article: {error}"))?;
    let mut number = 1;
    let name = loop {
        let name = numbered(stem, number);
        let path = folder.join(&name);
        // Linked rather than renamed, because a link fails on a name that is
        // taken where a rename would replace it.
        match fs::hard_link(&partial, &path) {
            Ok(()) => break name,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => number += 1,
            Err(error) => {
                let _ = fs::remove_file(&partial);
                return Err(format!("could not save the article: {error}"));
            }
        }
    };
    let _ = fs::remove_file(&partial);
    let record = notice(article, &name);
    let waiting = inbox.join(format!(".{}.writing", unique()));
    fs::write(&waiting, record)
        .and_then(|()| fs::rename(&waiting, inbox.join(format!("clip-{}.json", unique()))))
        .map_err(|error| format!("saved the article but could not announce it: {error}"))?;
    Ok(name)
}

fn deliver_to_device(host: &str, article: &Article) -> Result<String, Missed> {
    let output = super::run_remote_shell(
        &format!("root@{host}"),
        &device_script(article),
        COPY_PATIENCE,
    )
    .map_err(Missed::Away)?;
    let said = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        if let Some(name) = said.lines().find_map(|line| line.strip_prefix("saved=")) {
            return Ok(name.to_owned());
        }
    }
    let error = super::remote_shell_error(
        format!("the reader at {host} did not take the article"),
        &output.stdout,
        &output.stderr,
    );
    // ssh keeps 255 for its own failures: nothing on the reader ran.
    if output.status.code() == Some(255) {
        Err(Missed::Away(error))
    } else {
        Err(Missed::Refused(error))
    }
}

/// The script that saves an article on the reader and announces it.
///
/// Every value travels base64 encoded in a quoted here-document or argument,
/// so no byte of a title is ever read by the reader's shell. The book is
/// written under a hidden name and moved into place, so Books never lists
/// half of one, and the notice follows only once the book is there.
pub(crate) fn device_script(article: &Article) -> String {
    let stem = super::base64_encode(stem_of(&article.file_name).as_bytes());
    let (before, after) = notice_halves(article);
    format!(
        "set -eu\n\
         umask 022\n\
         dir='{DEVICE_ARTICLES}'\n\
         inbox='{DEVICE_INBOX}'\n\
         mkdir -p \"$dir\" \"$inbox\"\n\
         stem=$(printf '%s' '{stem}' | base64 -d)\n\
         partial=\"$dir/.clip-$$.writing\"\n\
         base64 -d > \"$partial\" <<'KOBO_ARTICLE'\n\
         {epub}\n\
         KOBO_ARTICLE\n\
         name=\"$stem.epub\"\n\
         n=2\n\
         while [ -e \"$dir/$name\" ]; do name=\"$stem ($n).epub\"; n=$((n + 1)); done\n\
         mv \"$partial\" \"$dir/$name\"\n\
         sync\n\
         record=\"$inbox/.clip-$$.writing\"\n\
         {{ printf '%s' '{before}' | base64 -d; printf '%s' \"$name\"; printf '%s' '{after}' | base64 -d; }} > \"$record\"\n\
         mv \"$record\" \"$inbox/clip-$(date +%s)-$$.json\"\n\
         sync\n\
         printf 'saved=%s\\n' \"$name\"\n",
        epub = super::base64_encode(&article.epub),
        before = super::base64_encode(before.as_bytes()),
        after = super::base64_encode(after.as_bytes()),
    )
}

/// The notice record for an article saved as `name`.
fn notice(article: &Article, name: &str) -> String {
    let (before, after) = notice_halves(article);
    format!("{before}{name}{after}")
}

/// The notice record either side of the file name, which only the reader
/// knows once it has found a free one.
///
/// The name is put in raw: a clipped file name never holds a quote, a
/// backslash or a control character, the only things JSON would escape.
fn notice_halves(article: &Article) -> (String, String) {
    let mut record = kobo_json::ObjectBuilder::new()
        .set("kind", "article")
        .set("title", article.title.as_str())
        .set("app", "books");
    if let Some(site) = article.site.as_deref().filter(|site| !site.is_empty()) {
        record = record.set("source", site);
    }
    let mut before = record.build().to_json();
    before.pop();
    before.push_str(&format!(",\"path\":\"{ARTICLES}/"));
    (before, "\"}".to_owned())
}

fn stem_of(file_name: &str) -> &str {
    file_name.strip_suffix(".epub").unwrap_or(file_name)
}

fn numbered(stem: &str, number: u32) -> String {
    if number <= 1 {
        format!("{stem}.epub")
    } else {
        format!("{stem} ({number}).epub")
    }
}

/// A name no other write from this computer is using at the same moment.
fn unique() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!(
        "{millis}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// `~/.config/kobo`, or `KOBO_CONFIG_DIR`, where the companion keeps its
/// state.
pub fn config_root() -> PathBuf {
    if let Some(folder) = std::env::var_os("KOBO_CONFIG_DIR") {
        return PathBuf::from(folder);
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".config").join("kobo")
}

/// Where articles wait for their reader.
pub fn queue_folder() -> PathBuf {
    config_root().join("clip").join("queue")
}

/// One article in the queue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Waiting {
    /// The name both its files share in the queue.
    pub id: String,
    pub title: String,
    pub site: Option<String>,
    pub file_name: String,
}

/// Puts an article in the queue.
///
/// The book is written first and its record second, and the queue reads
/// only records, so an article half written when the computer went to sleep
/// is never sent.
pub fn enqueue(queue: &Path, article: &Article) -> Result<Waiting, String> {
    fs::create_dir_all(queue)
        .map_err(|error| format!("could not make the queue at {}: {error}", queue.display()))?;
    let id = unique();
    let waiting = Waiting {
        id: id.clone(),
        title: article.title.clone(),
        site: article.site.clone(),
        file_name: article.file_name.clone(),
    };
    write_whole(&queue.join(format!("{id}.epub")), &article.epub)?;
    let mut record = kobo_json::ObjectBuilder::new()
        .set("title", article.title.as_str())
        .set("file_name", article.file_name.as_str());
    if let Some(site) = &article.site {
        record = record.set("site", site.as_str());
    }
    write_whole(
        &queue.join(format!("{id}.json")),
        record.build().to_json().as_bytes(),
    )?;
    Ok(waiting)
}

fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let partial = path.with_extension("writing");
    let written = fs::File::create(&partial)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&partial, path));
    written.map_err(|error| {
        let _ = fs::remove_file(&partial);
        format!("could not write {}: {error}", path.display())
    })
}

/// What is waiting, oldest first.
pub fn waiting(queue: &Path) -> Vec<Waiting> {
    let Ok(entries) = fs::read_dir(queue) else {
        return Vec::new();
    };
    let mut found: Vec<Waiting> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?.to_owned();
            if !queue.join(format!("{id}.epub")).is_file() {
                return None;
            }
            let record = kobo_json::parse(&fs::read_to_string(entry.path()).ok()?).ok()?;
            let text = |key: &str| {
                record
                    .get(key)
                    .and_then(kobo_json::Value::as_str)
                    .map(str::to_owned)
            };
            Some(Waiting {
                id,
                title: text("title")?,
                site: text("site"),
                file_name: text("file_name")?,
            })
        })
        .collect();
    // Ids begin with the time they were queued.
    found.sort_by_key(|waiting| {
        let millis: u128 = waiting
            .id
            .split('-')
            .next()
            .and_then(|stamp| stamp.parse().ok())
            .unwrap_or(0);
        (millis, waiting.id.clone())
    });
    found
}

fn load(queue: &Path, waiting: &Waiting) -> Option<Article> {
    Some(Article {
        title: waiting.title.clone(),
        site: waiting.site.clone(),
        file_name: waiting.file_name.clone(),
        epub: fs::read(queue.join(format!("{}.epub", waiting.id))).ok()?,
    })
}

fn forget(queue: &Path, waiting: &Waiting) {
    // The record goes first: without it the book is never sent again.
    let _ = fs::remove_file(queue.join(format!("{}.json", waiting.id)));
    let _ = fs::remove_file(queue.join(format!("{}.epub", waiting.id)));
}

/// What a sync did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Report {
    /// The titles that landed, oldest first.
    pub delivered: Vec<String>,
    /// How many are still waiting.
    pub waiting: usize,
    /// Why some were not delivered, in words for the owner.
    pub problem: Option<String>,
    /// What the connection itself said, for `kobo clip sync` to print under
    /// the sentence when someone needs to see why.
    pub detail: Option<String>,
}

impl Report {
    /// The one line `kobo clip sync --json` prints, which the Sidekick helper
    /// hands to Books as it is.
    pub fn to_json(&self) -> String {
        let mut reply = kobo_json::ObjectBuilder::new()
            .set(
                "delivered",
                u32::try_from(self.delivered.len()).unwrap_or(u32::MAX),
            )
            .set(
                "titles",
                kobo_json::Value::Array(
                    self.delivered
                        .iter()
                        .map(|title| kobo_json::Value::String(title.clone()))
                        .collect(),
                ),
            )
            .set("waiting", u32::try_from(self.waiting).unwrap_or(u32::MAX));
        if let Some(problem) = &self.problem {
            reply = reply.set("problem", problem.as_str());
        }
        reply.build().to_json()
    }
}

/// Sends everything waiting to `reader`, oldest first, until `patience`
/// runs out or the reader stops answering.
///
/// Only one sync works the queue at a time, so a clip arriving while Sync on
/// the reader is under way cannot send the same article twice. A second one
/// reports what is waiting and leaves it to the first.
pub fn sync(queue: &Path, reader: &Reader, patience: Duration) -> Report {
    sync_with(queue, patience, |article| deliver(reader, article))
}

pub(crate) fn sync_with(
    queue: &Path,
    patience: Duration,
    mut send: impl FnMut(&Article) -> Result<String, Missed>,
) -> Report {
    let Some(_claim) = Claim::take(queue) else {
        return Report {
            delivered: Vec::new(),
            waiting: waiting(queue).len(),
            problem: Some("Another delivery from this computer is under way.".to_owned()),
            detail: None,
        };
    };
    let started = Instant::now();
    let mut report = Report::default();
    let queued = waiting(queue);
    let mut left = queued.len();
    for entry in &queued {
        if started.elapsed() >= patience {
            break;
        }
        let Some(article) = load(queue, entry) else {
            // A record without its book cannot ever be sent.
            forget(queue, entry);
            left -= 1;
            continue;
        };
        match send(&article) {
            Ok(_) => {
                forget(queue, entry);
                left -= 1;
                report.delivered.push(entry.title.clone());
            }
            Err(Missed::Away(reason)) => {
                report.problem = Some(AWAY.to_owned());
                report.detail = Some(reason);
                break;
            }
            // A refusal is about this article, such as one too large for the
            // card's space; the rest may still fit.
            Err(Missed::Refused(reason)) => {
                report.problem = Some("Your Kobo would not take an article.".to_owned());
                report.detail = Some(reason);
            }
        }
    }
    report.waiting = left;
    report
}

/// What a reader that did not answer is, to its owner.
pub const AWAY: &str = "Your Kobo did not answer. It may be asleep or away from Wi-Fi.";

/// How long a clip keeps sending before it leaves the rest for later. The
/// owner, or a browser notification, is waiting on it.
const SEND_PATIENCE: Duration = Duration::from_secs(60);

/// Where one clip ended up.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Landing {
    /// On the reader. `also` counts articles that had been waiting and went
    /// with it.
    Landed { reader: String, also: usize },
    /// Kept on this computer for the next sync, and why.
    Waiting { why: String },
}

/// Sends an article now if the reader can be reached, and keeps it for
/// later if not.
///
/// The article joins the queue first and the queue is then sent, oldest
/// first, so anything clipped while the reader was asleep goes with it.
/// `reader` is the reader to send to, or why there is none to try.
pub fn send_now(
    queue: &Path,
    article: &Article,
    reader: Result<Reader, String>,
) -> Result<Landing, String> {
    send_with(queue, article, reader, |reader, article| {
        deliver(reader, article)
    })
}

pub(crate) fn send_with(
    queue: &Path,
    article: &Article,
    reader: Result<Reader, String>,
    mut send: impl FnMut(&Reader, &Article) -> Result<String, Missed>,
) -> Result<Landing, String> {
    let kept = enqueue(queue, article)?;
    let reader = match reader {
        Ok(reader) => reader,
        Err(why) => return Ok(Landing::Waiting { why }),
    };
    let report = sync_with(queue, SEND_PATIENCE, |article| send(&reader, article));
    if waiting(queue).iter().any(|entry| entry.id == kept.id) {
        return Ok(Landing::Waiting {
            why: report.problem.unwrap_or_else(|| AWAY.to_owned()),
        });
    }
    Ok(Landing::Landed {
        reader: reader.label(),
        also: report.delivered.len().saturating_sub(1),
    })
}

/// The queue's claim, held by one sync at a time and let go when dropped.
struct Claim(PathBuf);

impl Claim {
    fn take(queue: &Path) -> Option<Self> {
        let _ = fs::create_dir_all(queue);
        let marker = queue.join(".delivering");
        if fs::create_dir(&marker).is_ok() {
            return Some(Self(marker));
        }
        let stale = fs::metadata(&marker)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|made| made.elapsed().ok())
            .is_some_and(|age| age > STALE_CLAIM);
        if stale && fs::remove_dir(&marker).is_ok() && fs::create_dir(&marker).is_ok() {
            return Some(Self(marker));
        }
        None
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

/// "1 article", "2 articles".
pub fn articles_word(count: usize) -> String {
    if count == 1 {
        "1 article".to_owned()
    } else {
        format!("{count} articles")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("kobo-clip-delivery-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("a scratch folder");
        folder
    }

    fn article(title: &str) -> Article {
        Article {
            title: title.to_owned(),
            site: Some("example.org".to_owned()),
            file_name: kobo_article::book::file_name(title),
            epub: format!("book of {title}").into_bytes(),
        }
    }

    #[test]
    fn an_article_lands_in_the_articles_folder_and_is_announced_after() {
        let card = scratch("land-card");
        let inbox = card.join("inbox");
        let name = deliver_to_folders(&card, &inbox, &article("How the river \"ran\""))
            .expect("delivered");
        assert_eq!(name, "How the river ran.epub");
        assert_eq!(
            fs::read(card.join("Articles").join(&name)).expect("saved"),
            b"book of How the river \"ran\""
        );
        let records: Vec<_> = fs::read_dir(&inbox)
            .expect("the inbox")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(records.len(), 1, "one record and nothing half written");
        assert_eq!(
            records[0].extension().and_then(|e| e.to_str()),
            Some("json")
        );
        let record =
            kobo_json::parse(&fs::read_to_string(&records[0]).expect("read")).expect("valid");
        assert_eq!(
            record.get("title").and_then(kobo_json::Value::as_str),
            Some("How the river \"ran\"")
        );
        assert_eq!(
            record.get("path").and_then(kobo_json::Value::as_str),
            Some("Articles/How the river ran.epub")
        );
        assert_eq!(
            record.get("source").and_then(kobo_json::Value::as_str),
            Some("example.org")
        );
        assert_eq!(
            record.get("app").and_then(kobo_json::Value::as_str),
            Some("books")
        );
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn a_second_article_with_the_same_title_takes_the_next_number() {
        let card = scratch("again-card");
        let inbox = card.join("inbox");
        let first = deliver_to_folders(&card, &inbox, &article("Tides")).expect("first");
        let second = deliver_to_folders(&card, &inbox, &article("Tides")).expect("second");
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("Tides.epub", "Tides (2).epub")
        );
        let hidden = fs::read_dir(card.join("Articles"))
            .expect("listed")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with('.'));
        assert!(!hidden, "no partial file is left behind");
        let _ = fs::remove_dir_all(&card);
    }

    #[test]
    fn the_reader_script_carries_every_value_encoded_and_announces_last() {
        let script = device_script(&article("Robert'); rm -rf /"));
        assert!(!script.contains("rm -rf"), "a title reached the shell");
        assert!(script.contains("/mnt/onboard/Articles"));
        let saved = script
            .find("mv \"$partial\"")
            .expect("the book is moved into place");
        let announced = script.find("mv \"$record\"").expect("the notice is moved");
        assert!(saved < announced, "the notice must follow the book");
        assert!(
            script.contains("while [ -e \"$dir/$name\" ]"),
            "a clash is numbered"
        );
    }

    #[test]
    fn the_notice_halves_make_a_valid_record_around_any_saved_name() {
        let (before, after) = notice_halves(&article("A \\ title \u{1}"));
        let record = format!("{before}A title (2).epub{after}");
        let parsed = kobo_json::parse(&record).expect("valid json");
        assert_eq!(
            parsed.get("path").and_then(kobo_json::Value::as_str),
            Some("Articles/A title (2).epub")
        );
    }

    #[test]
    fn the_queue_keeps_articles_in_the_order_they_were_clipped() {
        let queue = scratch("order");
        for title in ["First", "Second", "Third"] {
            enqueue(&queue, &article(title)).expect("queued");
        }
        let titles: Vec<_> = waiting(&queue).into_iter().map(|w| w.title).collect();
        assert_eq!(titles, ["First", "Second", "Third"]);
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn a_book_whose_record_was_never_written_is_not_sent() {
        let queue = scratch("half");
        fs::write(queue.join("1-1-1.epub"), b"half").expect("a book alone");
        assert!(waiting(&queue).is_empty());
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn a_sync_sends_what_waits_and_forgets_only_what_landed() {
        let queue = scratch("sync");
        for title in ["One", "Two", "Three"] {
            enqueue(&queue, &article(title)).expect("queued");
        }
        let mut tried = Vec::new();
        let report = sync_with(&queue, Duration::from_secs(60), |article| {
            tried.push(article.title.clone());
            if article.title == "Two" {
                Err(Missed::Away("The reader is asleep.".to_owned()))
            } else {
                Ok(article.file_name.clone())
            }
        });
        assert_eq!(
            tried,
            ["One", "Two"],
            "a reader that went away is not tried again"
        );
        assert_eq!(report.delivered, ["One"]);
        assert_eq!(report.waiting, 2);
        assert_eq!(report.problem.as_deref(), Some(AWAY), "said plainly");
        assert_eq!(report.detail.as_deref(), Some("The reader is asleep."));
        let left: Vec<_> = waiting(&queue).into_iter().map(|w| w.title).collect();
        assert_eq!(left, ["Two", "Three"]);
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn a_refused_article_waits_while_the_rest_go_on() {
        let queue = scratch("refused");
        for title in ["Huge", "Small"] {
            enqueue(&queue, &article(title)).expect("queued");
        }
        let report = sync_with(&queue, Duration::from_secs(60), |article| {
            if article.title == "Huge" {
                Err(Missed::Refused("The card is full.".to_owned()))
            } else {
                Ok(article.file_name.clone())
            }
        });
        assert_eq!(report.delivered, ["Small"]);
        assert_eq!(report.waiting, 1);
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn two_syncs_at_once_never_send_the_same_article_twice() {
        let queue = scratch("claim");
        enqueue(&queue, &article("Only")).expect("queued");
        let held = Claim::take(&queue).expect("the first claim");
        let report = sync_with(&queue, Duration::from_secs(60), |_| {
            panic!("a second sync sent an article while the first held the queue")
        });
        assert_eq!(report.waiting, 1);
        assert!(report.problem.is_some());
        drop(held);
        let report = sync_with(&queue, Duration::from_secs(60), |article| {
            Ok(article.file_name.clone())
        });
        assert_eq!(report.delivered, ["Only"]);
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn the_report_reads_back_as_the_json_books_expects() {
        let report = Report {
            delivered: vec!["One".to_owned(), "Two".to_owned()],
            waiting: 1,
            problem: Some("The reader is asleep.".to_owned()),
            detail: Some("ssh: connect to host 192.168.1.9 port 22: timed out".to_owned()),
        };
        let parsed = kobo_json::parse(&report.to_json()).expect("valid");
        assert_eq!(
            parsed.get("delivered").and_then(kobo_json::Value::as_i64),
            Some(2)
        );
        assert_eq!(
            parsed.get("waiting").and_then(kobo_json::Value::as_i64),
            Some(1)
        );
        assert_eq!(
            parsed.get("problem").and_then(kobo_json::Value::as_str),
            Some("The reader is asleep.")
        );
        assert!(
            parsed.get("detail").is_none(),
            "the reader is told the sentence"
        );
    }

    #[test]
    fn a_clip_lands_with_what_was_waiting_and_the_queue_is_left_empty() {
        let queue = scratch("send-landed");
        enqueue(&queue, &article("Earlier")).expect("queued");
        let mut sent = Vec::new();
        let landing = send_with(
            &queue,
            &article("Now"),
            Ok(Reader::Device("192.168.1.9".to_owned())),
            |_, article| {
                sent.push(article.title.clone());
                Ok(article.file_name.clone())
            },
        )
        .expect("sent");
        assert_eq!(sent, ["Earlier", "Now"], "oldest first");
        assert_eq!(
            landing,
            Landing::Landed {
                reader: "the reader at 192.168.1.9".to_owned(),
                also: 1
            }
        );
        assert!(waiting(&queue).is_empty());
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn a_clip_for_a_reader_that_is_asleep_waits_and_says_so_plainly() {
        let queue = scratch("send-asleep");
        let landing = send_with(
            &queue,
            &article("Now"),
            Ok(Reader::Device("192.168.1.9".to_owned())),
            |_, _| Err(Missed::Away("ssh: connect to host: timed out".to_owned())),
        )
        .expect("kept");
        assert_eq!(
            landing,
            Landing::Waiting {
                why: AWAY.to_owned()
            }
        );
        assert_eq!(waiting(&queue).len(), 1);
        let _ = fs::remove_dir_all(&queue);
    }

    #[test]
    fn a_clip_with_no_reader_to_try_is_kept_with_the_reason() {
        let queue = scratch("send-none");
        let landing = send_with(
            &queue,
            &article("Now"),
            Err("No reader is saved.".to_owned()),
            |_, _| panic!("nothing should be sent without a reader"),
        )
        .expect("kept");
        assert_eq!(
            landing,
            Landing::Waiting {
                why: "No reader is saved.".to_owned()
            }
        );
        assert_eq!(waiting(&queue).len(), 1);
        let _ = fs::remove_dir_all(&queue);
    }
}
