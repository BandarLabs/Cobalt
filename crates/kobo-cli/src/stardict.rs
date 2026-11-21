//! Reading a `StarDict` dictionary as entries the reader can load.
//!
//! The reader looks words up in plain `headword<TAB>definition` files, a
//! format nobody publishes dictionaries in. The format they are published in
//! is `StarDict`: an `.ifo` that describes the dictionary, an `.idx` that
//! lists each word with where its definition sits, and a `.dict` holding the
//! definitions, usually compressed as `.dict.dz`. This module reads those
//! three on the computer and hands back headwords with plain-text
//! definitions, so the owner never has to find a converter.
//!
//! Everything read here was written by a stranger, so every size is checked
//! before it is believed: the files before they are read, the unpacked
//! definitions as they are expanded, the number of words, and each offset in
//! the index against the data it points into. A dictionary that cannot be
//! read is refused with the reason. One that can be read except for a few
//! entries loses those entries and says how many.

use std::borrow::Cow;
use std::fs;
use std::path::{Path, PathBuf};

/// The line every `.ifo` begins with.
const MAGIC: &str = "StarDict's dict ifo file";

/// The largest `.ifo` read. It is a dozen short lines.
const MAX_INFO_BYTES: u64 = 64 * 1024;

/// The largest index read, packed or not.
///
/// An entry is a word and eight bytes, so this is room for several million
/// words, which is more than any dictionary the reader could then hold.
const MAX_INDEX_BYTES: u32 = 64 * 1024 * 1024;

/// The most definitions read, unpacked.
///
/// Eight times what the reader loads in all, because markup shrinks: a
/// dictionary written in HTML can lose most of its bytes on the way to plain
/// text and still fit. Past this nothing would fit, and expanding a file that
/// claims more is how a few kilobytes of compressed zeroes fill a computer.
const MAX_DATA_BYTES: u32 = 512 * 1024 * 1024;

/// The most words read from an index.
const MAX_WORDS: usize = 2_000_000;

/// The longest word the format allows, in bytes.
const MAX_WORD_BYTES: usize = 255;

/// Markup tags that end a line without starting a paragraph.
///
/// The workspace's HTML reduction removes these and leaves nothing behind,
/// which is right for prose and wrong for a dictionary, where a `<br>` is
/// all that stands between one sense and the next.
const LINE_ENDING: [&str; 16] = [
    "br", "hr", "tr", "td", "th", "dt", "dd", "ol", "ul", "table", "h1", "h2", "h3", "h4", "h5",
    "h6",
];

/// A dictionary's three files, unpacked and still unread.
#[derive(Clone, Debug)]
pub struct Source {
    /// The file the owner named, for the sentences about it.
    shown: String,
    info: String,
    index: Vec<u8>,
    data: Vec<u8>,
    /// Whether a `.syn` file of alternative spellings sat beside the others.
    synonyms: bool,
}

/// What a conversion kept, and what it could not.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Converted {
    /// The name the dictionary gives itself.
    pub name: String,
    /// Headword and definition, in the index's own order.
    pub entries: Vec<(String, String)>,
    /// Definitions cut to the length the reader keeps.
    pub shortened: usize,
    /// Entries whose definition held nothing that reads as text.
    pub without_text: usize,
    /// Entries whose word the reader could not look up.
    pub unusable_words: usize,
    /// Entries pointing outside the definitions file.
    pub outside: usize,
    /// How many words the `.ifo` promised, when that is not how many the
    /// index held.
    pub promised: Option<usize>,
    /// Whether alternative spellings were present and left unconverted.
    pub synonyms: bool,
}

/// Whether `path` is named like something this module reads.
pub fn is_named(path: &Path) -> bool {
    let name = lowered_name(path);
    has_extension(&name, "ifo") || has_extension(&name, "zip")
}

/// Whether a name, already in lower case, ends in this extension.
fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|found| found == extension)
}

/// What to say about a file that is part of a dictionary, or packed in a
/// way this does not unpack, instead of the dictionary itself.
///
/// These are the files an owner is most likely to name by mistake: the big
/// one, or the archive the dictionary was downloaded as.
pub fn misnamed(path: &Path) -> Option<String> {
    let name = lowered_name(path);
    let shown = path.display();
    if [".idx", ".idx.gz", ".dict", ".dict.dz", ".syn"]
        .iter()
        .any(|part| name.ends_with(part))
    {
        return Some(format!(
            "{shown} is one part of a StarDict dictionary. Name the .ifo file beside it."
        ));
    }
    [
        ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tar.xz", ".7z", ".rar", ".gz", ".bz2", ".xz",
    ]
    .iter()
    .any(|packed| name.ends_with(packed))
    .then(|| {
        format!(
            "{shown} is an archive this does not unpack. Unpack it and name the .ifo \
                 file inside, or pack the dictionary's files as a .zip."
        )
    })
}

fn lowered_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Reads the dictionary `path` names: an `.ifo` with its files beside it, or
/// a `.zip` holding them.
pub fn open(path: &Path) -> Result<Source, String> {
    if has_extension(&lowered_name(path), "zip") {
        open_archive(path)
    } else {
        open_beside(path)
    }
}

/// The `.ifo` on disk, with the index and definitions that share its name.
fn open_beside(path: &Path) -> Result<Source, String> {
    let shown = path.display().to_string();
    let info = read_bounded(path, MAX_INFO_BYTES)?;
    let stem = path.with_extension("");
    let beside = |suffix: &str| -> PathBuf {
        let mut name = stem.clone().into_os_string();
        name.push(suffix);
        PathBuf::from(name)
    };
    let index = if beside(".idx").is_file() {
        read_bounded(&beside(".idx"), u64::from(MAX_INDEX_BYTES))?
    } else if beside(".idx.gz").is_file() {
        let packed = read_bounded(&beside(".idx.gz"), u64::from(MAX_INDEX_BYTES))?;
        unpack(
            &packed,
            MAX_INDEX_BYTES,
            &beside(".idx.gz").display().to_string(),
        )?
    } else {
        return Err(missing(&shown, ".idx"));
    };
    let data = if beside(".dict").is_file() {
        read_bounded(&beside(".dict"), u64::from(MAX_DATA_BYTES))?
    } else if beside(".dict.dz").is_file() {
        let packed = read_bounded(&beside(".dict.dz"), u64::from(MAX_DATA_BYTES))?;
        unpack(
            &packed,
            MAX_DATA_BYTES,
            &beside(".dict.dz").display().to_string(),
        )?
    } else {
        return Err(missing(&shown, ".dict or .dict.dz"));
    };
    Ok(Source {
        shown,
        info: text_of(info),
        index,
        data,
        synonyms: beside(".syn").is_file(),
    })
}

/// The same three files from inside a `.zip`, wherever in it they sit.
fn open_archive(zip: &Path) -> Result<Source, String> {
    let shown = zip.display().to_string();
    // Packed, a dictionary is smaller than its definitions; the unpacked
    // bounds are applied to each member as it comes out.
    let bytes = read_bounded(zip, u64::from(MAX_DATA_BYTES))?;
    let archive = kobo_doc::zip::Archive::open(&bytes)
        .map_err(|fault| format!("{shown} cannot be read as a zip: {fault}."))?;
    let infos: Vec<&str> = archive
        .names()
        .filter(|name| has_extension(name, "ifo") && !hidden_member(name))
        .collect();
    let stem = match infos.as_slice() {
        [] => {
            return Err(format!(
                "{shown} holds no .ifo file, so it is not a StarDict dictionary."
            ))
        }
        [only] => only.trim_end_matches(".ifo"),
        several => {
            return Err(format!(
                "{shown} holds {} dictionaries. Unpack it and install each .ifo by itself.",
                several.len()
            ))
        }
    };
    let has = |suffix: &str| {
        archive
            .names()
            .any(|name| name == format!("{stem}{suffix}"))
    };
    let member = |suffix: &str, limit: u32| -> Result<Vec<u8>, String> {
        let name = format!("{stem}{suffix}");
        let bytes = archive
            .read(&name)
            .map_err(|fault| format!("{name} in {shown} cannot be unpacked: {fault}."))?;
        if bytes.len() > limit as usize {
            return Err(too_large(&name, bytes.len() as u64, u64::from(limit)));
        }
        Ok(bytes)
    };
    let info = member(".ifo", u32::try_from(MAX_INFO_BYTES).unwrap_or(u32::MAX))?;
    let index = if has(".idx") {
        member(".idx", MAX_INDEX_BYTES)?
    } else if has(".idx.gz") {
        let packed = member(".idx.gz", MAX_INDEX_BYTES)?;
        unpack(&packed, MAX_INDEX_BYTES, &format!("{stem}.idx.gz"))?
    } else {
        return Err(missing(&shown, ".idx"));
    };
    let data = if has(".dict") {
        member(".dict", MAX_DATA_BYTES)?
    } else if has(".dict.dz") {
        let packed = member(".dict.dz", MAX_DATA_BYTES)?;
        unpack(&packed, MAX_DATA_BYTES, &format!("{stem}.dict.dz"))?
    } else {
        return Err(missing(&shown, ".dict or .dict.dz"));
    };
    Ok(Source {
        shown,
        info: text_of(info),
        index,
        data,
        synonyms: has(".syn"),
    })
}

/// Whether an archive member is one a desktop left behind rather than one
/// the dictionary's author packed, such as the `__MACOSX/._name.ifo` a Mac
/// adds beside every file it zips.
fn hidden_member(name: &str) -> bool {
    name.split('/')
        .any(|part| part.starts_with('.') || part == "__macosx")
}

fn missing(shown: &str, part: &str) -> String {
    format!(
        "{shown} has no {part} with it. A StarDict dictionary is an .ifo with its .idx and \
         its .dict or .dict.dz, all under the same name."
    )
}

fn too_large(shown: &str, bytes: u64, limit: u64) -> String {
    format!(
        "{shown} is {} MB, more than the {} MB this reads.",
        bytes.div_ceil(1024 * 1024),
        limit / (1024 * 1024)
    )
}

/// Reads a file whose size has been checked first, so a wrong path to a
/// film is refused by its length and not after it is in memory.
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let shown = path.display().to_string();
    let length = fs::metadata(path)
        .map_err(|error| format!("{shown} cannot be read: {error}"))?
        .len();
    if length > limit {
        return Err(too_large(&shown, length, limit));
    }
    fs::read(path).map_err(|error| format!("{shown} cannot be read: {error}"))
}

/// Expands a gzip file, which is what both `.dict.dz` and `.idx.gz` are.
///
/// A `.dict.dz` adds a table for seeking to the gzip header and is otherwise
/// one ordinary deflate stream, so the workspace's own gzip reader expands
/// it, against the limit rather than against the size the file claims.
fn unpack(packed: &[u8], limit: u32, shown: &str) -> Result<Vec<u8>, String> {
    kobo_net::gzip::expand(packed, limit).map_err(|error| match error {
        kobo_protocol::TaskError::TooLarge => format!(
            "{shown} unpacks to more than the {} MB this reads.",
            limit / (1024 * 1024)
        ),
        _ => format!("{shown} is not a gzip file this can unpack. It may be damaged."),
    })
}

/// An `.ifo` as text. A byte that is not UTF-8 costs that character, not the
/// dictionary: the one field kept from here is its name.
fn text_of(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// What an `.ifo` says that reading the other two files depends on.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Info {
    name: String,
    words: Option<usize>,
    index_bytes: Option<usize>,
    /// Whether the index gives each offset in eight bytes rather than four.
    wide: bool,
    /// The types every definition holds, in order, when the dictionary
    /// declares them once instead of marking each field.
    sequence: Option<String>,
}

fn parse_info(text: &str, shown: &str) -> Result<Info, String> {
    let mut lines = text
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    if lines.next() != Some(MAGIC) {
        return Err(format!(
            "{shown} is not a StarDict .ifo file: it does not begin with the line every \
             one begins with."
        ));
    }
    let mut info = Info {
        name: String::new(),
        words: None,
        index_bytes: None,
        wide: false,
        sequence: None,
    };
    for line in lines {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "version" if !matches!(value, "2.4.2" | "3.0.0") => {
                return Err(format!(
                    "{shown} is StarDict version {value}, and this reads 2.4.2 and 3.0.0."
                ));
            }
            "bookname" => value.clone_into(&mut info.name),
            "wordcount" => info.words = value.parse().ok(),
            "idxfilesize" => info.index_bytes = value.parse().ok(),
            "idxoffsetbits" => match value {
                "32" => info.wide = false,
                "64" => info.wide = true,
                other => {
                    return Err(format!(
                        "{shown} gives its index offsets {other} bits, and an index has 32 \
                         or 64."
                    ))
                }
            },
            "sametypesequence" => {
                if value.is_empty()
                    || value.len() > 16
                    || !value.bytes().all(|byte| byte.is_ascii_alphabetic())
                {
                    return Err(format!(
                        "{shown} declares definition types this cannot read: {value:?}."
                    ));
                }
                info.sequence = Some(value.to_owned());
            }
            _ => {}
        }
    }
    Ok(info)
}

/// One line of the index: a word and where its definition is.
struct Pointer<'a> {
    word: &'a [u8],
    offset: u64,
    size: u32,
}

/// The index's entries, in its own order.
///
/// Stops at the first entry it cannot make sense of and keeps what came
/// before, because an index cut short by a failed download is far more
/// common than one that is subtly wrong, and the count beside the `.ifo`'s
/// own says how much was lost.
fn pointers<'a>(index: &'a [u8], wide: bool, shown: &str) -> Result<Vec<Pointer<'a>>, String> {
    let numbers = if wide { 12 } else { 8 };
    let mut found = Vec::new();
    let mut rest = index;
    while !rest.is_empty() {
        let Some(end) = rest
            .iter()
            .take(MAX_WORD_BYTES + 1)
            .position(|byte| *byte == 0)
        else {
            break;
        };
        let Some(fields) = rest.get(end + 1..end + 1 + numbers) else {
            break;
        };
        let (offset, size) = fields.split_at(numbers - 4);
        let offset = offset
            .iter()
            .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte));
        let size = u32::from_be_bytes([size[0], size[1], size[2], size[3]]);
        if found.len() == MAX_WORDS {
            return Err(format!(
                "{shown} lists more than {MAX_WORDS} words, which is more than this reads."
            ));
        }
        found.push(Pointer {
            word: &rest[..end],
            offset,
            size,
        });
        rest = &rest[end + 1 + numbers..];
    }
    Ok(found)
}

/// Converts a dictionary to headwords with plain-text definitions.
///
/// `longest_word` and `longest_definition` are the reader's own limits, in
/// bytes. A definition past its limit is cut to it, because the reader drops
/// a longer line whole and a shortened definition is better than a missing
/// word; a word past its limit cannot be shortened into the same word, so it
/// is left out and counted.
pub fn convert(
    source: &Source,
    longest_word: usize,
    longest_definition: usize,
) -> Result<Converted, String> {
    let shown = &source.shown;
    let info = parse_info(&source.info, shown)?;
    if info
        .index_bytes
        .is_some_and(|bytes| bytes != source.index.len())
    {
        return Err(format!(
            "The .idx with {shown} is {} bytes and the .ifo describes one of {}. They are \
             not from the same dictionary, or one of them is incomplete.",
            source.index.len(),
            info.index_bytes.unwrap_or_default()
        ));
    }
    let pointers = pointers(&source.index, info.wide, shown)?;
    if pointers.is_empty() {
        return Err(format!("The .idx with {shown} lists no words."));
    }
    let mut converted = Converted {
        name: one_line(&info.name),
        promised: info.words.filter(|words| *words != pointers.len()),
        synonyms: source.synonyms,
        ..Converted::default()
    };
    for pointer in pointers {
        let Some(word) = headword(pointer.word, longest_word) else {
            converted.unusable_words += 1;
            continue;
        };
        let Some(data) = usize::try_from(pointer.offset)
            .ok()
            .and_then(|from| from.checked_add(pointer.size as usize).map(|to| from..to))
            .and_then(|range| source.data.get(range))
        else {
            converted.outside += 1;
            continue;
        };
        let text = one_line(&definition(
            data,
            info.sequence.as_deref(),
            longest_definition,
        ));
        if text.is_empty() {
            converted.without_text += 1;
            continue;
        }
        let (text, cut) = fit(text, longest_definition);
        converted.shortened += usize::from(cut);
        converted.entries.push((word, text));
    }
    if converted.entries.is_empty() {
        return Err(format!(
            "{shown} holds no definition this can read as text. It may hold only sounds or \
             pictures."
        ));
    }
    Ok(converted)
}

/// A word as the reader's file can carry it, or nothing if it cannot.
///
/// A tab would end the headword early and a leading `#` would make the line
/// a comment, so neither survives. A word left empty, or longer than the
/// reader keeps, is one no lookup could reach.
fn headword(word: &[u8], longest: usize) -> Option<String> {
    let word = one_line(std::str::from_utf8(word).ok()?);
    let word = word.trim_start_matches(['#', ' ']);
    (!word.is_empty() && word.len() <= longest && !kobo_dict::normalize(word).is_empty())
        .then(|| word.to_owned())
}

/// The readable text of one entry's data, every text field joined.
///
/// With a declared sequence the fields follow one another in that order and
/// the last runs to the end of the entry. Without one, each field begins
/// with a byte naming its type. A lower-case type is text ending at a zero
/// byte; an upper-case one is a length and that many bytes of something that
/// is not text, which is stepped over.
fn definition(data: &[u8], sequence: Option<&str>, ceiling: usize) -> String {
    let mut text = String::new();
    let mut rest = data;
    let mut declared = sequence.map(|sequence| sequence.bytes().peekable());
    loop {
        let (kind, last) = match &mut declared {
            Some(types) => match types.next() {
                Some(kind) => (kind, types.peek().is_none()),
                None => break,
            },
            None => match rest.split_first() {
                Some((kind, after)) => {
                    rest = after;
                    (*kind, false)
                }
                None => break,
            },
        };
        let field = if kind.is_ascii_uppercase() {
            if last {
                std::mem::take(&mut rest)
            } else {
                let Some(length) = rest
                    .get(..4)
                    .map(|size| u32::from_be_bytes([size[0], size[1], size[2], size[3]]) as usize)
                else {
                    break;
                };
                let Some(field) = rest.get(4..4 + length) else {
                    break;
                };
                rest = &rest[4 + length..];
                field
            }
        } else {
            let end = if last {
                rest.len()
            } else {
                rest.iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(rest.len())
            };
            let field = &rest[..end];
            rest = rest.get(end + 1..).unwrap_or_default();
            field
        };
        if let Some(read) = readable(kind, field, ceiling) {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&read);
        }
    }
    text
}

/// One field as text, when its type is one that reads as text.
///
/// Markup is reduced with the workspace's own HTML reduction, so a
/// definition never carries a tag to a reader that would draw it as
/// characters. A pronunciation is kept in brackets, the way a printed
/// dictionary sets it apart. Pictures, sounds and lists of attached files
/// have nothing to say on a page of text.
fn readable(kind: u8, field: &[u8], ceiling: usize) -> Option<String> {
    let text = std::str::from_utf8(field).ok()?;
    let text = text.trim_end_matches('\0');
    if text.trim().is_empty() {
        return None;
    }
    match kind {
        b'm' | b'l' | b'n' | b'w' => Some(text.to_owned()),
        b't' | b'y' => Some(format!("[{}]", text.trim())),
        b'h' | b'g' | b'k' => Some(kobo_html::to_text_within(&with_line_gaps(text), ceiling)),
        b'x' => Some(kobo_html::to_text_within(
            &with_line_gaps(&without_keys(text)),
            ceiling,
        )),
        _ => None,
    }
}

/// XDXF markup without its `<k>` elements.
///
/// XDXF repeats the headword inside every definition as `<k>word</k>`.
/// Reduced to text with the rest, every definition would begin by saying
/// the word that was just looked up.
fn without_keys(markup: &str) -> Cow<'_, str> {
    if !markup.contains("<k>") {
        return Cow::Borrowed(markup);
    }
    let mut out = String::with_capacity(markup.len());
    let mut rest = markup;
    while let Some(at) = rest.find("<k>") {
        out.push_str(&rest[..at]);
        rest = rest[at..]
            .find("</k>")
            .map_or("", |end| &rest[at + end + "</k>".len()..]);
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Markup with a space before every tag that ends a line.
fn with_line_gaps(markup: &str) -> String {
    let mut out = String::with_capacity(markup.len() + 16);
    let mut rest = markup;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let Some(end) = tail.find('>') else {
            break;
        };
        let name = kobo_html::element_name(&tail[1..end]);
        if LINE_ENDING.contains(&name.as_str()) {
            out.push(' ');
        }
        out.push_str(&tail[..=end]);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Text on one line: every run of white space a single space, and nothing
/// that a tab-separated file would read as structure.
pub fn one_line(text: &str) -> String {
    text.split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `text` within `limit` bytes, and whether it had to be cut to get there.
///
/// Cut on a character boundary and marked with an ellipsis, so a shortened
/// definition reads as shortened and not as one whose author stopped.
pub fn fit(text: String, limit: usize) -> (String, bool) {
    const MARK: char = '\u{2026}';
    if text.len() <= limit {
        return (text, false);
    }
    let mut cut = limit.saturating_sub(MARK.len_utf8());
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut kept = text[..cut].trim_end().trim_end_matches(MARK).to_owned();
    kept.push(MARK);
    (kept, true)
}

/// Synthetic dictionaries for tests, built the way the format lays them out.
#[cfg(test)]
pub(crate) mod fixtures {
    /// One entry of a dictionary under construction: a word and the bytes of
    /// its definition, already laid out as the format wants them.
    pub struct Entry<'a> {
        pub word: &'a str,
        pub data: Vec<u8>,
    }

    /// The `.ifo`, `.idx` and `.dict` of a dictionary holding `entries`.
    pub fn dictionary(
        name: &str,
        sequence: Option<&str>,
        wide: bool,
        entries: &[Entry<'_>],
    ) -> (String, Vec<u8>, Vec<u8>) {
        let mut index = Vec::new();
        let mut data = Vec::new();
        for entry in entries {
            index.extend_from_slice(entry.word.as_bytes());
            index.push(0);
            let offset = u64::try_from(data.len()).expect("a small dictionary");
            if wide {
                index.extend_from_slice(&offset.to_be_bytes());
            } else {
                index.extend_from_slice(
                    &u32::try_from(offset)
                        .expect("a small dictionary")
                        .to_be_bytes(),
                );
            }
            index.extend_from_slice(
                &u32::try_from(entry.data.len())
                    .expect("a small definition")
                    .to_be_bytes(),
            );
            data.extend_from_slice(&entry.data);
        }
        let mut info = format!(
            "StarDict's dict ifo file\nversion={}\nbookname={name}\nwordcount={}\nidxfilesize={}\n",
            if wide { "3.0.0" } else { "2.4.2" },
            entries.len(),
            index.len()
        );
        if wide {
            info.push_str("idxoffsetbits=64\n");
        }
        if let Some(sequence) = sequence {
            info.push_str(&format!("sametypesequence={sequence}\n"));
        }
        (info, index, data)
    }

    /// A gzip file holding `bytes`, with the extra header field and the
    /// file name a `.dict.dz` carries in front of its data.
    pub fn gzipped(bytes: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0b0000_1100, 0, 0, 0, 0, 0, 3];
        // An extra field shaped like the seek table: two letters, a length,
        // and that many bytes.
        let table = [b'R', b'A', 4, 0, 1, 0, 0, 0];
        out.extend_from_slice(&u16::try_from(table.len()).expect("short").to_le_bytes());
        out.extend_from_slice(&table);
        out.extend_from_slice(b"words.dict\0");
        out.extend_from_slice(&stored_deflate(bytes));
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(
            &u32::try_from(bytes.len())
                .expect("a small file")
                .to_le_bytes(),
        );
        out
    }

    /// A deflate stream that stores `bytes` without compressing them, which
    /// is all a test needs of a compressor and is written out here so the
    /// reader is tried against bytes it did not produce.
    fn stored_deflate(bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut blocks = bytes.chunks(0xffff).peekable();
        if blocks.peek().is_none() {
            out.extend_from_slice(&[1, 0, 0, 0xff, 0xff]);
        }
        while let Some(block) = blocks.next() {
            let length = u16::try_from(block.len()).expect("one block");
            out.push(u8::from(blocks.peek().is_none()));
            out.extend_from_slice(&length.to_le_bytes());
            out.extend_from_slice(&(!length).to_le_bytes());
            out.extend_from_slice(block);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{dictionary, gzipped, Entry};
    use super::*;

    const WORD: usize = 128;
    const DEFINITION: usize = 4096;

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("kobo-stardict-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("a scratch folder");
        folder
    }

    fn source(parts: (String, Vec<u8>, Vec<u8>)) -> Source {
        Source {
            shown: "words.ifo".to_owned(),
            info: parts.0,
            index: parts.1,
            data: parts.2,
            synonyms: false,
        }
    }

    fn plain(word: &str, text: &str) -> Entry<'static> {
        Entry {
            word: Box::leak(word.to_owned().into_boxed_str()),
            data: text.as_bytes().to_vec(),
        }
    }

    fn converted(parts: (String, Vec<u8>, Vec<u8>)) -> Converted {
        convert(&source(parts), WORD, DEFINITION).expect("converted")
    }

    #[test]
    fn a_dictionary_of_plain_text_comes_out_word_for_word() {
        let found = converted(dictionary(
            "Pocket English",
            Some("m"),
            false,
            &[
                plain("reader", "A person who reads."),
                plain("reading", "The act of reading.\nAlso a town."),
            ],
        ));
        assert_eq!(found.name, "Pocket English");
        assert_eq!(
            found.entries,
            [
                ("reader".to_owned(), "A person who reads.".to_owned()),
                (
                    "reading".to_owned(),
                    "The act of reading. Also a town.".to_owned()
                ),
            ]
        );
        assert_eq!(found.promised, None);
        assert_eq!(
            (found.shortened, found.without_text, found.unusable_words),
            (0, 0, 0)
        );
    }

    #[test]
    fn sixty_four_bit_offsets_point_at_the_same_definitions() {
        let entries = [
            plain("ink", "A coloured fluid."),
            plain("page", "One side of a leaf."),
        ];
        let narrow = converted(dictionary("Words", Some("m"), false, &entries));
        let wide = converted(dictionary("Words", Some("m"), true, &entries));
        assert_eq!(narrow.entries, wide.entries);
        assert_eq!(wide.entries[1].1, "One side of a leaf.");
    }

    #[test]
    fn a_declared_sequence_reads_each_field_by_its_place() {
        // A pronunciation, then a picture with its length, then the meaning
        // running to the end of the entry.
        let mut data = b"ri:d\0".to_vec();
        data.extend_from_slice(&3_u32.to_be_bytes());
        data.extend_from_slice(&[0xff, 0xd8, 0xff]);
        data.extend_from_slice(b"To look at words and understand them.");
        let found = converted(dictionary(
            "Words",
            Some("tPm"),
            false,
            &[Entry { word: "read", data }],
        ));
        assert_eq!(
            found.entries[0].1,
            "[ri:d] To look at words and understand them."
        );
    }

    #[test]
    fn fields_that_name_their_own_type_are_read_one_after_another() {
        let mut data = b"tri:d\0".to_vec();
        data.push(b'W');
        data.extend_from_slice(&2_u32.to_be_bytes());
        data.extend_from_slice(&[1, 2]);
        data.extend_from_slice(b"mTo look at words.\0");
        data.extend_from_slice(b"h<b>Also</b> a town.\0");
        let found = converted(dictionary(
            "Words",
            None,
            false,
            &[Entry { word: "read", data }],
        ));
        assert_eq!(found.entries[0].1, "[ri:d] To look at words. Also a town.");
    }

    #[test]
    fn markup_is_reduced_to_text_with_its_lines_kept_apart() {
        let found = converted(dictionary(
            "Words",
            Some("h"),
            false,
            &[
                plain(
                    "cat",
                    "<b>cat</b> <i>n.</i><br>1. A small animal.<br/>2. A jazz player &amp; fan.",
                ),
                plain("dog", "<script>alert(1)</script><p>A loyal animal.</p><p>See <a href=\"x\">hound</a>.</p>"),
            ],
        ));
        assert_eq!(
            found.entries[0].1,
            "cat n. 1. A small animal. 2. A jazz player & fan."
        );
        assert_eq!(found.entries[1].1, "A loyal animal. See hound.");
        assert!(found
            .entries
            .iter()
            .all(|(_, text)| !text.contains('<') && !text.contains('\n')));
    }

    #[test]
    fn an_xdxf_definition_does_not_begin_by_repeating_its_word() {
        let found = converted(dictionary(
            "Words",
            Some("x"),
            false,
            &[plain(
                "ink",
                "<k>ink</k>\n<tr>ɪŋk</tr> <abr>n.</abr> A coloured fluid for writing.",
            )],
        ));
        assert_eq!(found.entries[0].1, "ɪŋk n. A coloured fluid for writing.");
    }

    #[test]
    fn a_definition_longer_than_the_reader_keeps_is_shortened_and_counted() {
        let long = "word ".repeat(2_000);
        let found = converted(dictionary(
            "Words",
            Some("m"),
            false,
            &[plain("long", &long), plain("short", "Brief.")],
        ));
        assert_eq!(found.shortened, 1);
        let text = &found.entries[0].1;
        assert!(text.len() <= DEFINITION, "{}", text.len());
        assert!(text.ends_with('\u{2026}'));
        assert_eq!(found.entries[1].1, "Brief.");
        // The cut never lands inside a character.
        let (kept, cut) = fit("é".repeat(10), 9);
        assert!(
            cut && kept.len() <= 9 && kept.ends_with('\u{2026}'),
            "{kept}"
        );
    }

    #[test]
    fn a_word_the_reader_could_not_look_up_is_left_out_and_counted() {
        let endless = "a".repeat(WORD + 1);
        let found = converted(dictionary(
            "Words",
            Some("m"),
            false,
            &[
                plain(&endless, "Too long a word to keep."),
                plain("...", "Nothing to look up."),
                plain("#tag", "A label."),
                plain("tab\there", "Two words."),
                plain("fine", "Kept."),
            ],
        ));
        assert_eq!(found.unusable_words, 2);
        let words: Vec<&str> = found
            .entries
            .iter()
            .map(|(word, _)| word.as_str())
            .collect();
        assert_eq!(words, ["tag", "tab here", "fine"]);
    }

    #[test]
    fn an_entry_with_nothing_to_read_or_pointing_nowhere_costs_only_itself() {
        let (info, mut index, data) = dictionary(
            "Words",
            Some("W"),
            false,
            &[Entry {
                word: "bell",
                data: vec![1, 2, 3],
            }],
        );
        // A second word whose definition would lie past the end of the file.
        index.extend_from_slice(b"ghost\0");
        index.extend_from_slice(&900_u32.to_be_bytes());
        index.extend_from_slice(&10_u32.to_be_bytes());
        let info = info.replace("idxfilesize=13", &format!("idxfilesize={}", index.len()));
        let error = convert(
            &source((info.clone(), index.clone(), data.clone())),
            WORD,
            DEFINITION,
        )
        .expect_err("nothing readable");
        assert!(
            error.contains("no definition this can read as text"),
            "{error}"
        );

        let (text_info, text_index, mut text_data) =
            dictionary("Words", Some("m"), false, &[plain("ink", "A fluid.")]);
        let mut both = text_index;
        both.extend_from_slice(b"ghost\0");
        both.extend_from_slice(&900_u32.to_be_bytes());
        both.extend_from_slice(&10_u32.to_be_bytes());
        text_data.extend_from_slice(b"unused");
        let text_info = text_info.replace("idxfilesize=12", &format!("idxfilesize={}", both.len()));
        let found = converted((text_info, both, text_data));
        assert_eq!(found.outside, 1);
        assert_eq!(found.entries.len(), 1);
        assert_eq!(
            found.promised,
            Some(1),
            "the .ifo promised one and the index held two"
        );
    }

    #[test]
    fn an_index_cut_short_keeps_the_words_that_arrived() {
        let (info, mut index, data) = dictionary(
            "Words",
            Some("m"),
            false,
            &[plain("ink", "A fluid."), plain("page", "A leaf.")],
        );
        index.truncate(index.len() - 3);
        let info = info.replace("idxfilesize=25", &format!("idxfilesize={}", index.len()));
        let found = converted((info, index, data));
        assert_eq!(found.entries.len(), 1);
        assert_eq!(found.promised, Some(2));
    }

    #[test]
    fn files_that_do_not_belong_together_are_refused_with_the_reason() {
        let (info, mut index, data) =
            dictionary("Words", Some("m"), false, &[plain("ink", "A fluid.")]);
        index.extend_from_slice(b"extra");
        let error = convert(&source((info, index, data)), WORD, DEFINITION).expect_err("refused");
        assert!(error.contains("not from the same dictionary"), "{error}");

        for (text, said) in [
            ("bookname=Words\n", "not a StarDict .ifo file"),
            (
                "StarDict's dict ifo file\nversion=9.9\n",
                "StarDict version 9.9",
            ),
            ("StarDict's dict ifo file\nidxoffsetbits=48\n", "32 or 64"),
            (
                "StarDict's dict ifo file\nsametypesequence=m;rm\n",
                "types this cannot read",
            ),
        ] {
            let error = parse_info(text, "words.ifo").expect_err(text);
            assert!(error.contains(said), "{error}");
        }
    }

    #[test]
    fn a_dictionary_is_read_from_the_files_beside_its_ifo_packed_or_not() {
        let folder = scratch("beside");
        let (info, index, data) = dictionary(
            "Pocket",
            Some("m"),
            false,
            &[plain("ink", "A coloured fluid."), plain("page", "A leaf.")],
        );
        fs::write(folder.join("pocket.ifo"), &info).expect("ifo");
        fs::write(folder.join("pocket.idx"), &index).expect("idx");
        fs::write(folder.join("pocket.dict.dz"), gzipped(&data)).expect("dict.dz");
        fs::write(folder.join("pocket.syn"), b"inks\0\0\0\0\0").expect("syn");
        let opened = open(&folder.join("pocket.ifo")).expect("opened");
        let found = convert(&opened, WORD, DEFINITION).expect("converted");
        assert_eq!(found.entries.len(), 2);
        assert_eq!(found.entries[0].1, "A coloured fluid.");
        assert!(found.synonyms, "the unconverted spellings are owned up to");

        // The same dictionary with the index packed and the data not.
        fs::remove_file(folder.join("pocket.idx")).expect("removed");
        fs::remove_file(folder.join("pocket.dict.dz")).expect("removed");
        fs::write(folder.join("pocket.idx.gz"), gzipped(&index)).expect("idx.gz");
        fs::write(folder.join("pocket.dict"), &data).expect("dict");
        let again = convert(
            &open(&folder.join("pocket.ifo")).expect("opened"),
            WORD,
            DEFINITION,
        )
        .expect("converted");
        assert_eq!(again.entries, found.entries);

        fs::remove_file(folder.join("pocket.dict")).expect("removed");
        let error = open(&folder.join("pocket.ifo")).expect_err("incomplete");
        assert!(error.contains("no .dict or .dict.dz"), "{error}");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_dictionary_is_read_from_a_zip_wherever_in_it_the_files_sit() {
        let folder = scratch("zip");
        let (info, index, data) = dictionary(
            "Pocket",
            Some("m"),
            false,
            &[plain("ink", "A coloured fluid.")],
        );
        let members = vec![
            ("Pocket-2.4.2/README".to_owned(), b"read me".to_vec()),
            (
                "__MACOSX/Pocket-2.4.2/._Pocket.ifo".to_owned(),
                b"junk".to_vec(),
            ),
            ("Pocket-2.4.2/Pocket.ifo".to_owned(), info.into_bytes()),
            ("Pocket-2.4.2/Pocket.idx".to_owned(), index),
            ("Pocket-2.4.2/Pocket.dict.dz".to_owned(), gzipped(&data)),
        ];
        let zip = folder.join("pocket.zip");
        fs::write(&zip, kobo_doc::zip::stored(&members).expect("zipped")).expect("zip");
        let found = convert(&open(&zip).expect("opened"), WORD, DEFINITION).expect("converted");
        assert_eq!(
            found.entries,
            [("ink".to_owned(), "A coloured fluid.".to_owned())]
        );

        let empty = folder.join("empty.zip");
        fs::write(
            &empty,
            kobo_doc::zip::stored(&[("notes.txt".to_owned(), b"x".to_vec())]).expect("zipped"),
        )
        .expect("zip");
        let error = open(&empty).expect_err("no dictionary");
        assert!(error.contains("holds no .ifo file"), "{error}");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_packed_file_that_would_unpack_past_the_limit_is_refused() {
        let error = unpack(&gzipped(&[b'a'; 4_000]), 1_000, "words.dict.dz").expect_err("refused");
        assert!(error.contains("unpacks to more than"), "{error}");
        let error = unpack(b"not gzip at all", 1_000, "words.dict.dz").expect_err("refused");
        assert!(error.contains("not a gzip file"), "{error}");
    }

    #[test]
    fn a_part_or_an_archive_named_by_mistake_is_answered_with_what_to_name() {
        let said = misnamed(Path::new("oxford.dict.dz")).expect("a part");
        assert!(said.contains("Name the .ifo file beside it"), "{said}");
        let said = misnamed(Path::new("oxford.tar.bz2")).expect("an archive");
        assert!(said.contains("Unpack it"), "{said}");
        assert!(misnamed(Path::new("oxford.ifo")).is_none());
        assert!(is_named(Path::new("Oxford.IFO")) && is_named(Path::new("oxford.zip")));
        assert!(!is_named(Path::new("oxford.tsv")));
    }
}
