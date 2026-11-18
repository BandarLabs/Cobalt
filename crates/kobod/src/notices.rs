//! Notices: short news the runtime shows over whatever is on the panel, and
//! the list it keeps of them.
//!
//! Anything trusted on the reader raises one by dropping a small JSON file
//! into the inbox folder under Cobalt's state, which is the one thing a
//! companion on the computer can already do over SSH. Writing there takes
//! root on the reader, so the folder is its own authentication. The session
//! empties the inbox into a list it keeps beside it, and the launcher shows
//! that list.
//!
//! Shared by the device runtime and the simulator, so what a route sees in
//! the simulator is what the panel does.

use kobo_protocol::{
    Notice, NoticeList, NoticeTarget, MAX_NOTICES, MAX_NOTICE_DETAIL_LEN, MAX_NOTICE_HEADING_LEN,
    MAX_NOTICE_TITLE_LEN,
};
use kobo_ui::{Glyph, NoticeCard};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

/// The folder producers write into, under the notices folder.
pub const INBOX: &str = "inbox";
/// The list the runtime keeps, under the notices folder.
const LIST: &str = "list";
/// The largest record read from the inbox. A record is a title and a few
/// short fields; anything bigger is not one, and is not read into memory.
const MAX_RECORD_BYTES: u64 = 4 * 1024;
/// How many inbox files one pass reads. A producer gone wrong cannot hold
/// the panel's loop on a folder of thousands; the rest wait for the next pass.
const MAX_PER_PASS: usize = 32;
/// How long a card stays on the panel, in milliseconds. Long enough to read
/// a heading and a title at a glance, short enough that nobody waits for it.
pub const SHOWN_FOR_MS: u64 = 4_000;

/// What kind of news a notice is. The runtime words the kinds it knows, so
/// every article reads the same whoever announced it.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Kind {
    Article,
    /// Anything else, worded by its producer.
    Other(String),
}

impl Kind {
    fn heading(&self) -> &str {
        match self {
            Self::Article => "Article arrived",
            Self::Other(heading) => heading,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Article => "article",
            Self::Other(_) => "other",
        }
    }
}

/// One notice as the runtime keeps it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Kept {
    id: u64,
    kind: Kind,
    title: String,
    source: String,
    app: String,
    /// What a tap opens, relative to the card. Checked again when shown.
    path: Option<String>,
    at: u64,
    read: bool,
}

/// What a tap on the card that is showing opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Opens {
    /// The one notice it announces.
    One(u64),
    /// The list, because it stood for several.
    List,
}

/// The card to draw, and what tapping it opens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Shown {
    pub card: NoticeCard,
    pub opens: Opens,
}

/// The notices kept on this reader, and the inbox they arrive through.
#[derive(Debug)]
pub struct Notices {
    folder: PathBuf,
    kept: Vec<Kept>,
    /// Arrived and not yet on a card, oldest first.
    waiting: Vec<u64>,
    /// On the card that is up now. Something arriving while it is up joins
    /// it, so the owner sees one card that grows rather than a queue of them.
    showing: Vec<u64>,
    next_id: u64,
}

impl Notices {
    /// Opens the notices kept under `folder`, an empty list if there are none
    /// or they cannot be read. A damaged list costs the owner old news, not
    /// the session.
    #[must_use]
    pub fn open(folder: &Path) -> Self {
        let kept = fs::read_to_string(folder.join(LIST))
            .ok()
            .and_then(|text| kobo_json::parse(&text).ok())
            .map(|value| read_list(&value))
            .unwrap_or_default();
        let next_id = kept.iter().map(|kept| kept.id).max().unwrap_or(0) + 1;
        Self {
            folder: folder.to_path_buf(),
            kept,
            waiting: Vec::new(),
            showing: Vec::new(),
            next_id,
        }
    }

    /// The folder producers write into.
    #[must_use]
    pub fn inbox(&self) -> PathBuf {
        self.folder.join(INBOX)
    }

    /// Takes everything waiting in the inbox into the list, at `now`
    /// milliseconds since 1970. Returns how many arrived and how many files
    /// were not notices and were thrown away, which is all the trace says:
    /// a title is the owner's business, not the log's.
    pub fn collect(&mut self, now: u64) -> (usize, usize) {
        let Ok(entries) = fs::read_dir(self.inbox()) else {
            return (0, 0);
        };
        let mut names: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                // A writer renames a finished file into place, so a dotted or
                // `.part` name is one still being written.
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        !name.starts_with('.')
                            && Path::new(name)
                                .extension()
                                .is_some_and(|extension| extension == "json")
                    })
            })
            .collect();
        // Oldest first by name, which is how a producer that names its files
        // by time orders them; the list is newest first either way.
        names.sort();
        names.truncate(MAX_PER_PASS);
        let (mut arrived, mut discarded) = (0, 0);
        for path in names {
            let record = read_record(&path).and_then(|value| record(&value, now));
            // Removed whether it was a notice or not, so a bad file is said
            // once rather than on every pass.
            let _ = fs::remove_file(&path);
            match record {
                Some(mut kept) => {
                    kept.id = self.next_id;
                    self.next_id += 1;
                    self.waiting.push(kept.id);
                    self.kept.insert(0, kept);
                    arrived += 1;
                }
                None => discarded += 1,
            }
        }
        if arrived > 0 {
            self.kept
                .sort_by(|left, right| right.at.cmp(&left.at).then(right.id.cmp(&left.id)));
            self.kept.truncate(MAX_NOTICES);
            let kept: Vec<u64> = self.kept.iter().map(|kept| kept.id).collect();
            self.waiting.retain(|id| kept.contains(id));
            self.save();
        }
        (arrived, discarded)
    }

    /// Whether anything has arrived that no card has announced yet.
    #[must_use]
    pub fn waiting(&self) -> bool {
        !self.waiting.is_empty()
    }

    /// The card announcing everything waiting, and forgets that it waited.
    ///
    /// One notice is announced by its title. Several become one card, so a
    /// companion that sends five articles at once costs one interruption,
    /// not five. `asleep` says they came while the reader slept, which is
    /// the more useful thing to say than one title out of several.
    pub fn announce(&mut self, asleep: bool, glyph_of: impl Fn(&str) -> Glyph) -> Option<Shown> {
        if self.waiting.is_empty() {
            return None;
        }
        let mut ids = std::mem::take(&mut self.showing);
        ids.append(&mut self.waiting);
        let waiting: Vec<&Kept> = ids
            .iter()
            .filter_map(|id| self.kept.iter().find(|kept| kept.id == *id))
            .collect();
        let newest = waiting.iter().max_by_key(|kept| (kept.at, kept.id))?;
        let shown = if waiting.len() == 1 {
            Shown {
                card: NoticeCard {
                    glyph: glyph_of(&newest.app),
                    heading: newest.kind.heading().to_owned(),
                    line: newest.title.clone(),
                },
                opens: Opens::One(newest.id),
            }
        } else {
            let count = waiting.len();
            let articles = waiting.iter().all(|kept| kept.kind == Kind::Article);
            let same_app = waiting.iter().all(|kept| kept.app == newest.app);
            Shown {
                card: NoticeCard {
                    glyph: if same_app {
                        glyph_of(&newest.app)
                    } else {
                        Glyph::Bell
                    },
                    heading: if articles {
                        format!("{count} articles arrived")
                    } else {
                        format!("{count} new notifications")
                    },
                    line: if asleep {
                        "While the reader was asleep".to_owned()
                    } else {
                        format!("{} and {} more", newest.title, count - 1)
                    },
                },
                opens: Opens::List,
            }
        };
        self.showing = ids;
        Some(shown)
    }

    /// The card has gone, so whatever arrives next starts a card of its own.
    pub fn card_gone(&mut self) {
        self.showing.clear();
    }

    /// The list as the launcher shows it. `library` is the folder a notice's
    /// path is relative to, which is how a deleted article is told apart.
    #[must_use]
    pub fn list(&self, library: &Path, open: bool, glyph_of: impl Fn(&str) -> Glyph) -> NoticeList {
        NoticeList {
            entries: self
                .kept
                .iter()
                .map(|kept| Notice {
                    id: kept.id,
                    heading: clip(kept.kind.heading(), MAX_NOTICE_HEADING_LEN),
                    title: clip(&kept.title, MAX_NOTICE_TITLE_LEN),
                    detail: clip(&kept.source, MAX_NOTICE_DETAIL_LEN),
                    glyph: glyph_of(&kept.app),
                    at: kept.at,
                    unread: !kept.read,
                    target: match &kept.path {
                        None => NoticeTarget::Nothing,
                        Some(path) if library.join(path).is_file() => NoticeTarget::Ready,
                        Some(_) => NoticeTarget::Gone,
                    },
                })
                .collect(),
            open,
        }
    }

    /// What notice `id` opens: the application and the library identifier
    /// of the document, when it names one that is still on the reader.
    #[must_use]
    pub fn target(&self, id: u64, library: &Path) -> Option<(String, String)> {
        let kept = self.kept.iter().find(|kept| kept.id == id)?;
        let path = kept.path.as_ref()?;
        library
            .join(path)
            .is_file()
            .then(|| (kept.app.clone(), format!("0/{path}")))
    }

    /// Keeps what is waiting in the list without ever putting it on a card.
    /// For news that arrived while the owner was reading: by the time they
    /// look up it is not news, and the launcher's bell already says so.
    pub fn keep_quiet(&mut self) {
        self.waiting.clear();
        self.showing.clear();
    }

    /// Marks every notice read. Returns whether anything changed.
    pub fn mark_read(&mut self) -> bool {
        let changed = self.kept.iter().any(|kept| !kept.read);
        for kept in &mut self.kept {
            kept.read = true;
        }
        if changed {
            self.save();
        }
        changed
    }

    /// Forgets every notice, and any card still waiting to say one.
    pub fn clear(&mut self) {
        self.kept.clear();
        self.waiting.clear();
        self.showing.clear();
        self.save();
    }

    /// How many notices the owner has not seen in the list.
    #[must_use]
    pub fn unread(&self) -> usize {
        self.kept.iter().filter(|kept| !kept.read).count()
    }

    /// Written whole and renamed into place, so a reader that loses power
    /// mid-write keeps the list it had rather than half of the new one.
    fn save(&self) {
        let entries: Vec<kobo_json::Value> = self.kept.iter().map(write_kept).collect();
        let text = kobo_json::Value::Array(entries).to_json();
        let path = self.folder.join(LIST);
        let next = self.folder.join("list.next");
        let written = fs::create_dir_all(&self.folder).and_then(|()| {
            let mut file = fs::File::create(&next)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&next, &path)
        });
        if written.is_err() {
            eprintln!("notices: the list could not be saved");
        }
    }
}

fn read_record(path: &Path) -> Option<kobo_json::Value> {
    let mut text = String::new();
    fs::File::open(path)
        .ok()?
        .take(MAX_RECORD_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    kobo_json::parse(&text).ok()
}

/// Cut to `limit` bytes on a character boundary, controls removed: the wire
/// refuses anything longer or with a control in it, and one bad title must
/// not cost the owner the whole list.
fn clip(text: &str, limit: usize) -> String {
    let mut clipped = String::new();
    for character in text.trim().chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if clipped.len() + character.len_utf8() > limit {
            break;
        }
        clipped.push(character);
    }
    clipped
}

fn text(value: &kobo_json::Value, key: &str, limit: usize) -> String {
    value
        .get(key)
        .and_then(kobo_json::Value::as_str)
        .map(|text| clip(text, limit))
        .unwrap_or_default()
}

/// A path inside the card, or nothing: never absolute and never climbing out.
fn relative_path(value: &kobo_json::Value) -> Option<String> {
    let path = value.get("path")?.as_str()?;
    let parsed = Path::new(path);
    let plain = !path.is_empty()
        && path.len() <= 512
        && parsed
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    plain.then(|| path.to_owned())
}

/// Milliseconds in a record's `time`, which may be seconds or milliseconds.
fn moment(value: &kobo_json::Value, now: u64) -> u64 {
    let Some(time) = value.get("time").and_then(kobo_json::Value::as_f64) else {
        return now;
    };
    if !(time.is_finite() && time > 0.0) {
        return now;
    }
    // Anything before 2001 in milliseconds is a count of seconds.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let time = time as u64;
    let millis = if time < 1_000_000_000_000 {
        time.saturating_mul(1000)
    } else {
        time
    };
    // A producer's clock ahead of the reader's would put news in the future,
    // where "today" reads strangely; it arrived now.
    millis.min(now)
}

fn record(value: &kobo_json::Value, now: u64) -> Option<Kept> {
    let title = text(value, "title", MAX_NOTICE_TITLE_LEN);
    if title.is_empty() {
        return None;
    }
    let kind = match value.get("kind").and_then(kobo_json::Value::as_str) {
        Some("article") | None => Kind::Article,
        Some(_) => {
            let heading = text(value, "heading", MAX_NOTICE_HEADING_LEN);
            if heading.is_empty() {
                return None;
            }
            Kind::Other(heading)
        }
    };
    let app = text(value, "app", 64);
    Some(Kept {
        id: 0,
        kind,
        title,
        source: text(value, "source", MAX_NOTICE_DETAIL_LEN),
        app: if app.is_empty() {
            "books".to_owned()
        } else {
            app
        },
        path: relative_path(value),
        at: moment(value, now),
        read: false,
    })
}

#[allow(clippy::cast_precision_loss)]
fn write_kept(kept: &Kept) -> kobo_json::Value {
    let mut object = kobo_json::ObjectBuilder::new()
        .set("id", kept.id as f64)
        .set("kind", kept.kind.name())
        .set("heading", kept.kind.heading())
        .set("title", kept.title.as_str())
        .set("source", kept.source.as_str())
        .set("app", kept.app.as_str())
        .set("time", kept.at as f64)
        .set("read", kept.read);
    if let Some(path) = &kept.path {
        object = object.set("path", path.as_str());
    }
    object.build()
}

fn read_list(value: &kobo_json::Value) -> Vec<Kept> {
    let Some(entries) = value.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let mut kept = record(entry, u64::MAX)?;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                kept.id = entry.get("id")?.as_f64()? as u64;
            }
            kept.read = entry
                .get("read")
                .and_then(kobo_json::Value::as_bool)
                .unwrap_or(false);
            Some(kept)
        })
        .take(MAX_NOTICES)
        .collect()
}

/// Whether a card may go over `screen`, the one on the panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Room {
    /// Nothing on the panel minds a card.
    Show,
    /// A keyboard or a dialogue is up. The card waits until it is answered,
    /// because a card over a question competes with it for the same glance.
    Hold,
    /// The owner is reading. No card at all: a reading page is the one
    /// screen nothing should interrupt.
    Quiet,
}

/// Where a card would stand on `screen`, with a runtime keyboard open or not.
///
/// A reading page is a screen marked as one, or one that hides its bar to
/// give the whole panel to a page, a comic or a photograph.
#[must_use]
pub fn room_on(screen: Option<&kobo_ui::Screen>, typing: bool) -> Room {
    match screen {
        Some(screen) if screen.reading || screen.auto_hide_top_bar => Room::Quiet,
        Some(screen) if !typing && screen.overlay.is_none() => Room::Show,
        _ => Room::Hold,
    }
}

/// Milliseconds since 1970 by the reader's clock, which is when a notice
/// with no time of its own arrived.
#[must_use]
pub fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The mark of a built-in application, by the name the launcher starts it
/// by. A Store application's own mark is looked up by the caller first.
#[must_use]
pub fn builtin_mark(app: &str) -> Glyph {
    match app {
        "books" => Glyph::Book,
        "store" => Glyph::Download,
        "settings" => Glyph::Settings,
        "terminal" => Glyph::Terminal,
        _ => Glyph::Bell,
    }
}

#[cfg(test)]
mod tests;
