//! `kobo dict`: getting an offline dictionary onto the reader.
//!
//! The reader looks words up in tab-separated files in one folder, and until
//! now the only way to put one there was to make it by hand and copy it over
//! USB. This command takes a dictionary as it is published, converts it on
//! the computer, and installs it over the connection every other command
//! uses, or onto a reader mounted over USB, or into the simulator's folder.
//!
//! The reader loads a bounded number of files of a bounded size, and it
//! passes over anything past those bounds without a word. So everything is
//! settled here, where there is somebody to tell: a large dictionary is
//! split into files the reader will load, the result is read back with the
//! reader's own parser before it travels, and one that would not fit beside
//! what is already installed is refused with the numbers.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read as _, Write};
use std::path::{Path, PathBuf};

use super::books::size_words;
use super::{console, stardict};
use kobo_dict::{
    Dictionary, MAX_DEFINITION_BYTES, MAX_DICTIONARIES, MAX_DICTIONARY_BYTES, MAX_HEADWORD_BYTES,
    MAX_TOTAL_BYTES,
};

/// Where the runtime reads dictionaries from, on the reader.
const DEVICE_DICTIONARIES: &str = "/mnt/onboard/.adds/cobalt/dictionaries";

/// How much of a file is read to find the name and language it declares.
const METADATA_BYTES: usize = 4096;

/// How many headwords `check` shows.
const SAMPLES: usize = 5;

/// What `kobo dict --help` prints.
pub const USAGE: &str = concat!(
    "usage: kobo dict install FILE [--name NAME] [--language CODE] ",
    "[--device IP | --reader NAME | --volume PATH | --sim]\n",
    "       kobo dict list [--device IP | --reader NAME | --volume PATH | --sim]\n",
    "       kobo dict remove NAME [--device IP | --reader NAME | --volume PATH | --sim]\n",
    "       kobo dict check FILE [--name NAME] [--language CODE]\n",
    "\n",
    "Installs an offline dictionary, so a word can be looked up while reading\n",
    "without Wi-Fi. FILE is one of:\n",
    "  a Cobalt dictionary   a .tsv of headword, tab, definition\n",
    "  a StarDict dictionary the .ifo, with its .idx and .dict or .dict.dz\n",
    "                        beside it, or a .zip holding them\n",
    "\n",
    "A StarDict dictionary is converted on this computer. Its markup becomes\n",
    "plain text, and a large one is split into the files the reader loads.\n",
    "check converts and reads the result back without sending anything.\n",
    "--name NAME sets the name the dictionary is listed and removed by.\n",
    "--language CODE says which books it answers for, such as en or pt-BR.\n",
    "--volume PATH installs onto a reader mounted over USB.\n",
    "The reader loads at most 32 dictionary files of 8 MB each, 64 MB in all.\n",
    super::targets::saved_reader_note!()
);

/// Where dictionaries are installed.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Place {
    /// A reader at this address, over SSH.
    Device(String),
    /// A folder on this computer: the simulator's, or a mounted reader's.
    Folder { folder: PathBuf, label: String },
}

impl Place {
    fn label(&self) -> String {
        match self {
            Self::Device(host) => format!("the reader at {host}"),
            Self::Folder { label, .. } => label.clone(),
        }
    }
}

/// One command line, read.
#[derive(Debug, Default)]
struct Request {
    /// The file to install or check, or the name to remove.
    subject: Option<String>,
    name: Option<String>,
    language: Option<String>,
    place: Option<Place>,
}

/// Runs `kobo dict`.
pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let (verb, rest) = arguments
        .split_first()
        .ok_or_else(|| console::usage(USAGE))?;
    let request = parse(rest)?;
    let subject = |what: &str| {
        request
            .subject
            .clone()
            .ok_or_else(|| console::usage(format!("kobo dict {verb} {what}")))
    };
    let place = || {
        request
            .place
            .clone()
            .ok_or_else(|| console::usage(format!("kobo dict {verb}: {NO_PLACE}")))
    };
    match verb.as_str() {
        "check" => {
            if request.place.is_some() {
                return Err(console::usage(
                    "kobo dict check sends nothing, so it takes no reader",
                ));
            }
            check(&subject("FILE")?, &request)
        }
        "install" => install(&subject("FILE")?, &request, &place()?),
        "list" => list(&place()?),
        "remove" => remove(&subject("NAME")?, &place()?),
        _ => Err(console::usage(USAGE)),
    }
}

const NO_PLACE: &str = "name a reader with --reader NAME, --device IP, --volume PATH or --sim";

fn parse(arguments: &[String]) -> Result<Request, String> {
    let mut request = Request::default();
    let mut arguments = arguments.iter();
    while let Some(argument) = arguments.next() {
        let mut value = |what: &str| {
            arguments
                .next()
                .cloned()
                .ok_or_else(|| console::usage(format!("{argument} takes {what}")))
        };
        let place = match argument.as_str() {
            flag if super::is_device_flag(flag) => {
                let host = value("an address")?;
                if !super::valid_device_host(&host) {
                    return Err(console::usage(format!(
                        "{host:?} is not an address this can reach"
                    )));
                }
                Place::Device(host)
            }
            "--volume" => mounted(Path::new(&value("the folder a reader is mounted at")?))?,
            "--sim" => Place::Folder {
                folder: simulator_folder(),
                label: "the simulator".to_owned(),
            },
            "--name" if request.name.is_none() => {
                request.name = Some(value("a name")?);
                continue;
            }
            "--language" if request.language.is_none() => {
                let code = value("a language code")?;
                if !valid_language(&code) {
                    return Err(console::usage(
                        "--language takes a short code such as en, fr or pt-BR",
                    ));
                }
                request.language = Some(code);
                continue;
            }
            word if request.subject.is_none() && !word.starts_with('-') => {
                request.subject = Some(word.to_owned());
                continue;
            }
            other => {
                return Err(console::usage(format!(
                    "kobo dict does not know {other}. See kobo dict --help."
                )))
            }
        };
        if request.place.replace(place).is_some() {
            return Err(console::usage(format!(
                "choose one place to install to, not several: {NO_PLACE}"
            )));
        }
    }
    Ok(request)
}

/// The dictionaries folder of a reader mounted at `volume`.
///
/// Every Kobo's card has a `.kobo` folder at its root, and a path without
/// one is a mistyped mount point far more often than a reader. Writing a
/// `.adds` tree into somebody's Documents folder and calling it installed is
/// the failure this refuses.
fn mounted(volume: &Path) -> Result<Place, String> {
    if !volume.join(".kobo").is_dir() {
        return Err(console::target(format!(
            "{} is not a Kobo's card: it has no .kobo folder. Name the folder the reader \
             is mounted at.",
            volume.display()
        )));
    }
    Ok(Place::Folder {
        folder: volume.join(".adds").join("cobalt").join("dictionaries"),
        label: format!("the reader mounted at {}", volume.display()),
    })
}

/// Where the runtime on this computer reads dictionaries, which is the one
/// place `kobod` looks when it is not on a reader.
fn simulator_folder() -> PathBuf {
    kobo_abi::home_directory().map_or_else(
        || PathBuf::from(".kobo-dictionaries"),
        |home| home.join(".config").join("kobo").join("dictionaries"),
    )
}

/// The rule the reader applies to a language tag, so one it would ignore is
/// refused where it is typed.
fn valid_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 16
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

/// A dictionary read from the owner's file and ready to be split.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Prepared {
    name: String,
    language: String,
    priority: i16,
    /// Headword and definition, in the source's own order.
    entries: Vec<(String, String)>,
    /// What the owner should know about what was left out or changed.
    notes: Vec<String>,
}

/// Reads `file` as whichever kind of dictionary it is.
fn prepare(file: &str, request: &Request) -> Result<Prepared, String> {
    let path = Path::new(file);
    if !path.is_file() {
        return Err(format!("{file} is not a file on this computer."));
    }
    if let Some(said) = stardict::misnamed(path) {
        return Err(console::unsupported(said));
    }
    let mut prepared = if stardict::is_named(path) {
        from_stardict(path)?
    } else {
        from_tsv(path)?
    };
    if let Some(name) = &request.name {
        prepared.name = label(name, &prepared.name);
    }
    if let Some(language) = &request.language {
        prepared.language.clone_from(language);
    }
    Ok(prepared)
}

fn from_stardict(path: &Path) -> Result<Prepared, String> {
    let converted = stardict::convert(
        &stardict::open(path)?,
        MAX_HEADWORD_BYTES,
        MAX_DEFINITION_BYTES,
    )?;
    let mut notes = Vec::new();
    let mut note = |count: usize, one: &str, many: &str| match count {
        0 => {}
        1 => notes.push(format!("1 {one}")),
        _ => notes.push(format!("{count} {many}")),
    };
    note(
        converted.shortened,
        "definition was longer than the 4 KB the reader keeps and was shortened.",
        "definitions were longer than the 4 KB the reader keeps and were shortened.",
    );
    note(
        converted.unusable_words,
        "word could not be looked up as written, or was longer than the reader keeps, and \
         was left out.",
        "words could not be looked up as written, or were longer than the reader keeps, and \
         were left out.",
    );
    note(
        converted.without_text,
        "entry held a sound or a picture and no text, and was left out.",
        "entries held a sound or a picture and no text, and were left out.",
    );
    note(
        converted.outside,
        "entry pointed outside the definitions file and was left out.",
        "entries pointed outside the definitions file and were left out.",
    );
    if let Some(promised) = converted.promised {
        notes.push(format!(
            "Its .ifo promises {promised} words and its index held {}.",
            converted.entries.len()
                + converted.unusable_words
                + converted.without_text
                + converted.outside
        ));
    }
    if converted.synonyms {
        notes.push(
            "Its alternative spellings (the .syn file) are not converted, so a word is \
             found under its main spelling."
                .to_owned(),
        );
    }
    let fallback = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Prepared {
        name: label(&converted.name, &label(&fallback, "Dictionary")),
        language: "und".to_owned(),
        priority: 0,
        entries: converted.entries,
        notes,
    })
}

/// A Cobalt dictionary as its owner wrote it.
///
/// The reader's own parser says what the file calls itself, and each line is
/// held to the rule that parser applies, so a line the reader would drop is
/// dropped here and counted, where somebody can be told.
fn from_tsv(path: &Path) -> Result<Prepared, String> {
    let shown = path.display();
    let length = fs::metadata(path)
        .map_err(|error| format!("{shown} cannot be read: {error}"))?
        .len();
    if length > MAX_TOTAL_BYTES as u64 {
        return Err(too_large_for_the_reader(
            &shown.to_string(),
            length,
            usize::MAX,
        ));
    }
    let text = fs::read(path)
        .map_err(|error| format!("{shown} cannot be read: {error}"))
        .and_then(|bytes| {
            String::from_utf8(bytes).map_err(|_| {
                console::unsupported(format!(
                    "{shown} is not a dictionary this reads. A Cobalt dictionary is UTF-8 \
                     text, one headword, a tab and a definition to the line; a StarDict \
                     dictionary is named by its .ifo file."
                ))
            })
        })?;
    let fallback = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let declared = Dictionary::from_tsv(&fallback, &text);
    let mut entries = Vec::new();
    let mut dropped = 0_usize;
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        match line.split_once('\t') {
            Some((headword, definition)) if keeps(headword.trim(), definition.trim()) => {
                entries.push((headword.trim().to_owned(), definition.trim().to_owned()));
            }
            _ => dropped += 1,
        }
    }
    if entries.is_empty() {
        return Err(console::unsupported(format!(
            "{shown} holds no entry the reader would keep. A Cobalt dictionary is one \
             headword, a tab and a definition to the line."
        )));
    }
    let mut notes = Vec::new();
    if dropped > 0 {
        notes.push(format!(
            "{dropped} {} not a headword, a tab and a definition within the reader's limits, \
             and {} left out.",
            if dropped == 1 { "line is" } else { "lines are" },
            if dropped == 1 { "was" } else { "were" }
        ));
    }
    Ok(Prepared {
        name: declared.name.clone(),
        language: declared.language.clone(),
        priority: declared.priority,
        entries,
        notes,
    })
}

/// Whether the reader keeps a line holding this headword and definition.
fn keeps(headword: &str, definition: &str) -> bool {
    !headword.is_empty()
        && !definition.is_empty()
        && headword.len() <= MAX_HEADWORD_BYTES
        && definition.len() <= MAX_DEFINITION_BYTES
        && !kobo_dict::normalize(headword).is_empty()
}

/// A name the reader accepts: one line, and no longer than it keeps.
fn label(name: &str, fallback: &str) -> String {
    let (name, _) = stardict::fit(stardict::one_line(name), 96);
    let name = name.trim_start_matches(['#', ' ']).trim();
    if name.is_empty() {
        fallback.to_owned()
    } else {
        name.to_owned()
    }
}

/// One file of an installed dictionary.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Part {
    file: String,
    text: String,
}

/// The file name stem a dictionary's files share, made from its name.
///
/// Letters and digits of any script are kept, so a dictionary named in
/// Cyrillic keeps a name its owner can read over USB, and everything else
/// becomes a hyphen, so no name needs quoting anywhere it is used.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "dictionary".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Whether `file` is one of the files an install under `slug` writes.
fn is_part_of(file: &str, slug: &str) -> bool {
    let Some(middle) = file
        .strip_prefix(slug)
        .and_then(|rest| rest.strip_suffix(".tsv"))
    else {
        return false;
    };
    middle.is_empty()
        || middle
            .strip_prefix('.')
            .is_some_and(|number| number.len() == 2 && number.bytes().all(|b| b.is_ascii_digit()))
}

/// Splits a dictionary into files the reader will load.
///
/// Each file carries the dictionary's name, so the reader shows every part
/// under the one name. A file ends between two headwords, never between two
/// senses of one: entries the reader files under the same key stay together,
/// so a lookup finds them in the order the dictionary gave them.
fn split(prepared: &Prepared) -> Result<Vec<Part>, String> {
    let mut header = format!(
        "# name={}\n# language={}\n",
        prepared.name, prepared.language
    );
    if prepared.priority != 0 {
        header.push_str(&format!("# priority={}\n", prepared.priority));
    }
    let mut texts = vec![header.clone()];
    let mut entries = prepared.entries.iter().peekable();
    while let Some((headword, definition)) = entries.next() {
        let key = kobo_dict::normalize(headword);
        let mut group = format!("{headword}\t{definition}\n");
        while let Some((next, definition)) =
            entries.next_if(|(next, _)| kobo_dict::normalize(next) == key)
        {
            group.push_str(&format!("{next}\t{definition}\n"));
        }
        let current = texts.last_mut().expect("there is always a file");
        if current.len() + group.len() > MAX_DICTIONARY_BYTES && current.len() > header.len() {
            texts.push(format!("{header}{group}"));
        } else {
            current.push_str(&group);
        }
    }
    let total: usize = texts.iter().map(String::len).sum();
    let oversized = texts.iter().any(|text| text.len() > MAX_DICTIONARY_BYTES);
    if texts.len() > MAX_DICTIONARIES || total > MAX_TOTAL_BYTES || oversized {
        return Err(too_large_for_the_reader(
            &prepared.name,
            total as u64,
            texts.len(),
        ));
    }
    let slug = slug(&prepared.name);
    let several = texts.len() > 1;
    Ok(texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| Part {
            file: if several {
                format!("{slug}.{:02}.tsv", index + 1)
            } else {
                format!("{slug}.tsv")
            },
            text,
        })
        .collect())
}

/// The refusal for a dictionary no reader could load, with the numbers.
fn too_large_for_the_reader(name: &str, bytes: u64, files: usize) -> String {
    let needs = if files == usize::MAX {
        size_words(bytes)
    } else {
        format!("{} in {}", size_words(bytes), files_word(files))
    };
    console::unsupported(format!(
        "{name} is {needs}. The reader loads at most {MAX_DICTIONARIES} dictionary files of \
         {} each and {} in all, so it cannot take this one.",
        size_words(MAX_DICTIONARY_BYTES as u64),
        size_words(MAX_TOTAL_BYTES as u64)
    ))
}

fn files_word(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

/// What the reader's own parser made of the files about to be installed.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Checked {
    entries: usize,
    samples: Vec<String>,
}

/// Reads every part back with the reader's parser.
///
/// The parts were written by this command, so the reader keeping fewer
/// entries than went in is a fault here and not in the owner's file. It is
/// still checked, because the alternative is finding out on the reader, one
/// missing word at a time.
fn read_back(prepared: &Prepared, parts: &[Part]) -> Result<Checked, String> {
    let mut entries = 0;
    let mut rejected = 0;
    let mut headwords = Vec::new();
    for part in parts {
        let dictionary = Dictionary::from_tsv("unnamed", &part.text);
        if dictionary.name != prepared.name || dictionary.language != prepared.language {
            return Err(format!(
                "{} did not read back under its own name and language, so nothing was \
                 installed. Try another --name.",
                prepared.name
            ));
        }
        entries += dictionary.len();
        rejected += dictionary.rejected_lines;
        headwords.extend(dictionary.headwords().map(str::to_owned));
    }
    if rejected != 0 || entries != prepared.entries.len() {
        return Err(format!(
            "{} did not read back whole: the reader's parser kept {entries} of {} entries. \
             Nothing was installed.",
            prepared.name,
            prepared.entries.len()
        ));
    }
    let step = (headwords.len() / SAMPLES).max(1);
    let samples = headwords.into_iter().step_by(step).take(SAMPLES).collect();
    Ok(Checked { entries, samples })
}

fn check(file: &str, request: &Request) -> Result<(), String> {
    let prepared = prepare(file, request)?;
    let parts = split(&prepared)?;
    let checked = read_back(&prepared, &parts)?;
    println!("{}", summary(&prepared, &parts, &checked));
    println!("Sample headwords: {}", checked.samples.join(", "));
    for note in &prepared.notes {
        println!("{note}");
    }
    println!("Nothing was sent. To install it: kobo dict install {file}");
    Ok(())
}

/// One line saying what a dictionary is: its name, size and language.
fn summary(prepared: &Prepared, parts: &[Part], checked: &Checked) -> String {
    let bytes: usize = parts.iter().map(|part| part.text.len()).sum();
    format!(
        "{}: {} {}, {} in {} for the reader. {}",
        prepared.name,
        checked.entries,
        if checked.entries == 1 {
            "entry"
        } else {
            "entries"
        },
        size_words(bytes as u64),
        files_word(parts.len()),
        if prepared.language == "und" {
            "No language is set, so it answers for books in any language; --language CODE \
             sets one."
                .to_owned()
        } else {
            format!("Language: {}.", prepared.language)
        }
    )
}

/// One dictionary file already on a reader.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Held {
    file: String,
    bytes: u64,
    name: String,
    language: String,
}

impl Held {
    /// A file as the reader would describe it, from its name and the
    /// comment lines at its head.
    fn described(file: &str, bytes: u64, head: &str) -> Self {
        let stem = file.strip_suffix(".tsv").unwrap_or(file);
        let declared = Dictionary::from_tsv(stem, &comments_of(head));
        Self {
            file: file.to_owned(),
            bytes,
            name: declared.name,
            language: declared.language,
        }
    }
}

/// The comment lines of a file's head, which is where a dictionary says
/// what it is called. The last line may be cut mid-way and is left out.
fn comments_of(head: &str) -> String {
    let whole = head.rfind('\n').map_or("", |end| &head[..=end]);
    whole
        .lines()
        .filter(|line| line.starts_with('#'))
        .fold(String::new(), |mut comments, line| {
            comments.push_str(line);
            comments.push('\n');
            comments
        })
}

/// The files of `held` the reader loads, by the reader's own rule: the
/// first thirty-two by name, skipping any that is too large or would take
/// the total past what it holds.
fn loaded(files: &[(String, u64)]) -> BTreeSet<String> {
    let mut sorted: Vec<&(String, u64)> = files.iter().collect();
    sorted.sort();
    let mut total = 0_u64;
    let mut kept = BTreeSet::new();
    for (file, bytes) in sorted.into_iter().take(MAX_DICTIONARIES) {
        if *bytes > MAX_DICTIONARY_BYTES as u64 || total + bytes > MAX_TOTAL_BYTES as u64 {
            continue;
        }
        total += bytes;
        kept.insert(file.clone());
    }
    kept
}

/// The dictionary files on `place`.
fn held(place: &Place) -> Result<Vec<Held>, String> {
    match place {
        Place::Folder { folder, .. } => Ok(held_in(folder)),
        Place::Device(host) => {
            let output =
                super::run_remote_shell(&format!("root@{host}"), &listing_script(), PATIENCE)
                    .map_err(|detail| away(place, &detail))?;
            let said = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || !said.lines().any(|line| line == "listed") {
                return Err(not_done(place, "list its dictionaries", &output));
            }
            Ok(held_from(&said))
        }
    }
}

/// How long a short exchange with the reader may take.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

fn held_in(folder: &Path) -> Vec<Held> {
    let mut found: Vec<Held> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file = entry.file_name().into_string().ok()?;
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() || !plain_name(&file) {
                return None;
            }
            let mut head = Vec::new();
            fs::File::open(entry.path())
                .ok()?
                .take(METADATA_BYTES as u64)
                .read_to_end(&mut head)
                .ok()?;
            Some(Held::described(
                &file,
                metadata.len(),
                &String::from_utf8_lossy(&head),
            ))
        })
        .collect();
    found.sort_by(|left, right| left.file.cmp(&right.file));
    found
}

/// The script that lists the reader's dictionary files: each one's name,
/// its size from the directory, and the comment lines at its head.
///
/// The size is asked of the listing and only the head of each file is read,
/// so listing sixty megabytes of dictionaries does not read them.
fn listing_script() -> String {
    format!(
        "dir='{DEVICE_DICTIONARIES}'\n\
         for file in \"$dir\"/*.tsv; do\n\
         [ -f \"$file\" ] || continue\n\
         set -- $(ls -l \"$file\")\n\
         printf 'file=%s\\n' \"${{file##*/}}\"\n\
         printf 'bytes=%s\\n' \"${{5:-0}}\"\n\
         head -c {METADATA_BYTES} \"$file\" | grep '^#' | sed 's/^/meta=/'\n\
         printf 'end\\n'\n\
         done\n\
         printf 'listed\\n'\n"
    )
}

/// The files a listing script reported.
fn held_from(said: &str) -> Vec<Held> {
    let mut found = Vec::new();
    let (mut file, mut bytes, mut head) = (None, 0, String::new());
    for line in said.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(name) = line.strip_prefix("file=") {
            file = Some(name.to_owned());
            bytes = 0;
            head.clear();
        } else if let Some(size) = line.strip_prefix("bytes=") {
            bytes = size.trim().parse().unwrap_or(0);
        } else if let Some(comment) = line.strip_prefix("meta=") {
            head.push_str(comment);
            head.push('\n');
        } else if line == "end" {
            if let Some(file) = file.take().filter(|file| plain_name(file)) {
                found.push(Held::described(&file, bytes, &head));
            }
        }
    }
    found.sort_by(|left: &Held, right| left.file.cmp(&right.file));
    found
}

/// Whether a file name from a reader is safe to act on: one dictionary file
/// in the dictionaries folder and nothing that could name another place.
fn plain_name(file: &str) -> bool {
    // The suffix exactly as the reader asks for it: a `.TSV` is not loaded,
    // so it is not listed as though it were.
    Path::new(file)
        .extension()
        .is_some_and(|extension| extension == "tsv")
        && !file.starts_with('.')
        && !file.contains(['/', '\\'])
        && !file.chars().any(char::is_control)
}

fn away(place: &Place, detail: &str) -> String {
    console::Console::with_details(
        console::target(format!(
            "{} did not answer. It may be asleep or away from Wi-Fi.",
            capitalised(&place.label())
        )),
        detail,
    )
}

fn not_done(place: &Place, what: &str, output: &super::RemoteShellOutput) -> String {
    // ssh keeps 255 for its own failures: nothing on the reader ran.
    if output.status.code() == Some(255) {
        return away(place, &String::from_utf8_lossy(&output.stderr));
    }
    super::remote_shell_error(
        format!("{} could not {what}", capitalised(&place.label())),
        &output.stdout,
        &output.stderr,
    )
}

fn capitalised(label: &str) -> String {
    let mut letters = label.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(letters).collect()
    })
}

fn install(file: &str, request: &Request, place: &Place) -> Result<(), String> {
    let prepared = prepare(file, request)?;
    let parts = split(&prepared)?;
    let checked = read_back(&prepared, &parts)?;
    let there = held(place)?;
    let slug = slug(&prepared.name);
    // A reinstall replaces the dictionary of that name, however its files
    // were named when they were put there.
    let (replaced, kept): (Vec<&Held>, Vec<&Held>) = there
        .iter()
        .partition(|held| held.name == prepared.name || is_part_of(&held.file, &slug));
    let before: Vec<(String, u64)> = there
        .iter()
        .map(|held| (held.file.clone(), held.bytes))
        .collect();
    let mut after: Vec<(String, u64)> = kept
        .iter()
        .map(|held| (held.file.clone(), held.bytes))
        .collect();
    after.extend(
        parts
            .iter()
            .map(|part| (part.file.clone(), part.text.len() as u64)),
    );
    let (loads_now, would_load) = (loaded(&before), loaded(&after));
    let fits = parts.iter().all(|part| would_load.contains(&part.file))
        && kept
            .iter()
            .all(|held| !loads_now.contains(&held.file) || would_load.contains(&held.file));
    if !fits {
        return Err(no_slot(&prepared, &parts, &kept, place));
    }
    let stale: Vec<String> = replaced
        .iter()
        .filter(|held| parts.iter().all(|part| part.file != held.file))
        .map(|held| held.file.clone())
        .collect();
    match place {
        Place::Folder { folder, label } => write_to_folder(folder, label, &parts, &stale)?,
        Place::Device(host) => write_to_device(host, place, &parts, &stale)?,
    }
    println!(
        "Installed {} on {}: {} {} in {}, {}.",
        prepared.name,
        place.label(),
        checked.entries,
        if checked.entries == 1 {
            "entry"
        } else {
            "entries"
        },
        files_word(parts.len()),
        size_words(parts.iter().map(|part| part.text.len() as u64).sum())
    );
    if !replaced.is_empty() {
        println!("It replaces the {} that was there.", prepared.name);
    }
    for note in &prepared.notes {
        println!("{note}");
    }
    println!(
        "Cobalt loads dictionaries at the first word looked up after it starts. If it is \
         running, leave Cobalt and start it again."
    );
    Ok(())
}

/// The refusal for a dictionary that does not fit beside what is installed.
fn no_slot(prepared: &Prepared, parts: &[Part], kept: &[&Held], place: &Place) -> String {
    let bytes: u64 = parts.iter().map(|part| part.text.len() as u64).sum();
    let held: u64 = kept.iter().map(|held| held.bytes).sum();
    console::unsupported(format!(
        "{} is {} in {}, and {} already holds {} in {}. The reader loads at most \
         {MAX_DICTIONARIES} dictionary files and {} in all, so it would not load all of \
         both. Remove one first: kobo dict list, then kobo dict remove NAME.",
        prepared.name,
        size_words(bytes),
        files_word(parts.len()),
        place.label(),
        size_words(held),
        files_word(kept.len()),
        size_words(MAX_TOTAL_BYTES as u64)
    ))
}

/// Installs into a folder on this computer: the simulator's, or a mounted
/// reader's.
///
/// Every part is written under a hidden name before any is given its own,
/// so a card pulled out half way leaves the dictionary that was there.
fn write_to_folder(
    folder: &Path,
    label: &str,
    parts: &[Part],
    stale: &[String],
) -> Result<(), String> {
    fs::create_dir_all(folder)
        .map_err(|error| format!("{} cannot be made: {error}", folder.display()))?;
    let bytes: u64 = parts.iter().map(|part| part.text.len() as u64).sum();
    if let Some(free) = kobo_abi::free_space(folder) {
        if free < super::books::needed(bytes) {
            return Err(no_room(bytes, free, label));
        }
    }
    let hidden = |part: &Part| folder.join(format!(".{}.writing", part.file));
    let written = parts.iter().try_for_each(|part| {
        fs::File::create(hidden(part)).and_then(|mut file| {
            file.write_all(part.text.as_bytes())?;
            file.sync_all()
        })
    });
    let placed = written.and_then(|()| {
        parts
            .iter()
            .try_for_each(|part| fs::rename(hidden(part), folder.join(&part.file)))
    });
    if let Err(error) = placed {
        for part in parts {
            let _ = fs::remove_file(hidden(part));
        }
        return Err(format!(
            "The dictionary could not be written to {}: {error}",
            folder.display()
        ));
    }
    for file in stale {
        let _ = fs::remove_file(folder.join(file));
    }
    Ok(())
}

fn no_room(bytes: u64, free: u64, label: &str) -> String {
    format!(
        "The card on {label} has {} free. The dictionary is {}, and an install leaves {} \
         for the Kobo's own library, so it was not installed.",
        size_words(free),
        size_words(bytes),
        size_words(kobo_protocol::SHELF_RESERVE)
    )
}

fn write_to_device(
    host: &str,
    place: &Place,
    parts: &[Part],
    stale: &[String],
) -> Result<(), String> {
    let bytes: u64 = parts.iter().map(|part| part.text.len() as u64).sum();
    let head = install_script(parts, stale, super::books::needed(bytes).div_ceil(1024));
    let texts: Vec<String> = parts.iter().map(|part| part.text.clone()).collect();
    let output = super::run_remote_shell_fed(
        &format!("root@{host}"),
        move |shell| write_install(shell, &head, &texts),
        super::books::patience_for(bytes),
    )
    .map_err(|detail| away(place, &detail))?;
    let said = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
        if let Some(free) = said.lines().find_map(|line| line.strip_prefix("full=")) {
            let free = free.trim().parse::<u64>().unwrap_or(0).saturating_mul(1024);
            return Err(no_room(bytes, free, &place.label()));
        }
        if said.lines().any(|line| line == "damaged") {
            return Err(format!(
                "The copy that reached {} was not what was sent, so it was removed and the \
                 dictionaries there are as they were. Install it again.",
                place.label()
            ));
        }
        if said.lines().any(|line| line.starts_with("installed=")) {
            return Ok(());
        }
    }
    Err(not_done(place, "install the dictionary", &output))
}

/// The script that installs a dictionary's parts on the reader, up to the
/// first part's bytes.
///
/// Every part is uploaded under a hidden name and hashed before any is
/// moved into place, and the move is a function defined before the upload
/// and called by the script's last word. A connection that drops then leaves
/// the dictionary that was there, never half of the new one beside half of
/// the old.
fn install_script(parts: &[Part], stale: &[String], needed_kilobytes: u64) -> String {
    let named = |file: &str| {
        format!(
            "\"$dir/$(printf '%s' '{}' | base64 -d)\"",
            super::base64_encode(file.as_bytes())
        )
    };
    let mut finish = String::new();
    for (index, part) in parts.iter().enumerate() {
        finish.push_str(&format!(
            "set -- $(sha256sum \"$dir/.dict-$$-{index}.writing\")\n\
             if [ \"$1\" != '{}' ]; then printf 'damaged\\n'; exit 0; fi\n",
            super::sha256::hex_digest(part.text.as_bytes())
        ));
    }
    for file in stale {
        finish.push_str(&format!("rm -f {}\n", named(file)));
    }
    for (index, part) in parts.iter().enumerate() {
        finish.push_str(&format!(
            "mv -f \"$dir/.dict-$$-{index}.writing\" {}\n",
            named(&part.file)
        ));
    }
    format!(
        "set -eu\n\
         umask 022\n\
         dir='{DEVICE_DICTIONARIES}'\n\
         mkdir -p \"$dir\"\n\
         {room}\
         trap 'rm -f \"$dir\"/.dict-$$-*.writing' EXIT\n\
         trap 'exit 1' HUP INT TERM\n\
         finish() {{\n\
         {finish}\
         sync\n\
         printf 'installed={count}\\n'\n\
         }}\n",
        room = super::room_script(needed_kilobytes),
        count = parts.len(),
    )
}

/// Writes an install to the reader's shell: the script, each part in
/// slices under its hidden name, and the word that finishes it.
fn write_install(shell: &mut dyn Write, head: &str, texts: &[String]) -> std::io::Result<()> {
    shell.write_all(head.as_bytes())?;
    for (index, text) in texts.iter().enumerate() {
        shell.write_all(
            format!("partial=\"$dir/.dict-$$-{index}.writing\"\n: > \"$partial\"\n").as_bytes(),
        )?;
        super::append_in_slices(shell, text.as_bytes())?;
    }
    shell.write_all(b"finish\n")
}

/// The installed files gathered into the dictionaries they belong to, in
/// name order.
fn shelves(held: &[Held]) -> Vec<(String, Vec<&Held>)> {
    let mut shelves: Vec<(String, Vec<&Held>)> = Vec::new();
    for file in held {
        match shelves.iter_mut().find(|(name, _)| *name == file.name) {
            Some((_, files)) => files.push(file),
            None => shelves.push((file.name.clone(), vec![file])),
        }
    }
    shelves.sort_by(|left, right| left.0.to_lowercase().cmp(&right.0.to_lowercase()));
    shelves
}

fn list(place: &Place) -> Result<(), String> {
    let held = held(place)?;
    if held.is_empty() {
        println!(
            "No dictionaries are installed on {}. Install one with kobo dict install FILE.",
            place.label()
        );
        return Ok(());
    }
    print!("{}", listing(&held, &place.label()));
    Ok(())
}

/// What `kobo dict list` prints for these files.
fn listing(held: &[Held], label: &str) -> String {
    let loads = loaded(
        &held
            .iter()
            .map(|held| (held.file.clone(), held.bytes))
            .collect::<Vec<_>>(),
    );
    let shelves = shelves(held);
    let width = shelves
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let language_width = shelves
        .iter()
        .map(|(_, files)| files[0].language.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("Dictionaries on {label}:\n");
    for (name, files) in &shelves {
        let bytes: u64 = files.iter().map(|file| file.bytes).sum();
        let unloaded = files
            .iter()
            .filter(|file| !loads.contains(&file.file))
            .count();
        let padding = " ".repeat(width.saturating_sub(name.chars().count()));
        out.push_str(&format!(
            "  {name}{padding}  {:language_width$}  {}, {}{}\n",
            files[0].language,
            files_word(files.len()),
            size_words(bytes),
            match unloaded {
                0 => String::new(),
                _ if unloaded == files.len() => {
                    "  not loaded: past the reader's limits".to_owned()
                }
                _ => format!("  {unloaded} not loaded: past the reader's limits"),
            }
        ));
    }
    out.push_str(&format!(
        "{} and {} of the {MAX_DICTIONARIES} files and {} the reader loads.\n",
        files_word(held.len()),
        size_words(held.iter().map(|held| held.bytes).sum()),
        size_words(MAX_TOTAL_BYTES as u64)
    ));
    out
}

fn remove(name: &str, place: &Place) -> Result<(), String> {
    let held = held(place)?;
    let wanted = slug(name);
    let shelves = shelves(&held);
    let Some((found, files)) = shelves.iter().find(|(shelf, files)| {
        shelf.eq_ignore_ascii_case(name)
            || slug(shelf) == wanted
            || files
                .iter()
                .any(|file| file.file.strip_suffix(".tsv") == Some(name))
    }) else {
        println!(
            "No dictionary called {name} is on {}. kobo dict list shows what is.",
            place.label()
        );
        return Ok(());
    };
    match place {
        Place::Folder { folder, .. } => {
            for file in files {
                fs::remove_file(folder.join(&file.file))
                    .map_err(|error| format!("{} could not be removed: {error}", file.file))?;
            }
        }
        Place::Device(host) => {
            let mut script = format!("set -eu\ndir='{DEVICE_DICTIONARIES}'\n");
            for file in files {
                script.push_str(&format!(
                    "rm -f \"$dir/$(printf '%s' '{}' | base64 -d)\"\n",
                    super::base64_encode(file.file.as_bytes())
                ));
            }
            script.push_str("sync\nprintf 'removed\\n'\n");
            let output = super::run_remote_shell(&format!("root@{host}"), &script, PATIENCE)
                .map_err(|detail| away(place, &detail))?;
            let said = String::from_utf8_lossy(&output.stdout);
            if !output.status.success() || !said.lines().any(|line| line == "removed") {
                return Err(not_done(place, "remove the dictionary", &output));
            }
        }
    }
    println!(
        "Removed {found} from {} ({}).",
        place.label(),
        files_word(files.len())
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stardict::fixtures::{dictionary, gzipped, Entry};

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("kobo-dict-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("a scratch folder");
        folder
    }

    fn words(line: &[&str]) -> Vec<String> {
        line.iter().map(|word| (*word).to_owned()).collect()
    }

    fn prepared(name: &str, entries: &[(&str, &str)]) -> Prepared {
        Prepared {
            name: name.to_owned(),
            language: "en".to_owned(),
            priority: 0,
            entries: entries
                .iter()
                .map(|(word, text)| ((*word).to_owned(), (*text).to_owned()))
                .collect(),
            notes: Vec::new(),
        }
    }

    /// A `StarDict` dictionary of plain text on disk, returning its `.ifo`.
    fn stardict_on_disk(folder: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
        let entries: Vec<Entry<'_>> = entries
            .iter()
            .map(|(word, text)| Entry {
                word,
                data: text.as_bytes().to_vec(),
            })
            .collect();
        let (info, index, data) = dictionary(name, Some("m"), false, &entries);
        fs::write(folder.join("words.ifo"), info).expect("ifo");
        fs::write(folder.join("words.idx"), index).expect("idx");
        fs::write(folder.join("words.dict.dz"), gzipped(&data)).expect("dict.dz");
        folder.join("words.ifo")
    }

    fn folder_place(folder: &Path) -> Place {
        Place::Folder {
            folder: folder.to_path_buf(),
            label: "the simulator".to_owned(),
        }
    }

    #[test]
    fn a_stardict_dictionary_installs_as_a_file_the_reader_loads_and_looks_up() {
        let root = scratch("install");
        let ifo = stardict_on_disk(
            &root,
            "Pocket English",
            &[
                ("reader", "A person who reads."),
                ("story", "An account of events."),
            ],
        );
        let folder = root.join("dictionaries");
        let request = Request {
            language: Some("en".to_owned()),
            ..Request::default()
        };
        install(&ifo.display().to_string(), &request, &folder_place(&folder)).expect("installed");
        assert_eq!(
            fs::read_to_string(folder.join("pocket-english.tsv")).expect("written"),
            "# name=Pocket English\n# language=en\nreader\tA person who reads.\n\
             story\tAn account of events.\n"
        );
        // The reader's own loader is the judge of whether it is installed.
        let index = kobo_dict::Index::load_directory(&folder);
        assert_eq!(index.len(), 1);
        let found = index.lookup("stories", Some("en"));
        assert_eq!(found[0].dictionary, "Pocket English");
        assert_eq!(found[0].definition, "An account of events.");
        let hidden = fs::read_dir(&folder)
            .expect("listed")
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with('.'));
        assert!(!hidden, "no partial file is left behind");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_dictionary_is_listed_and_removed_by_the_name_it_was_installed_under() {
        let root = scratch("list");
        let ifo = stardict_on_disk(&root, "Pocket English", &[("ink", "A coloured fluid.")]);
        let folder = root.join("dictionaries");
        let place = folder_place(&folder);
        install(&ifo.display().to_string(), &Request::default(), &place).expect("installed");
        fs::write(
            folder.join("Mine.tsv"),
            "# language=fr\nencre\tUn liquide.\n",
        )
        .expect("own");
        let held = held(&place).expect("listed");
        let said = listing(&held, "the simulator");
        assert!(said.contains("Mine            fr   1 file, 1 KB"), "{said}");
        assert!(said.contains("Pocket English  und  1 file, 1 KB"), "{said}");
        assert!(
            said.contains("2 files and 1 KB of the 32 files and 64 MB"),
            "{said}"
        );

        remove("pocket english", &place).expect("removed");
        assert!(!folder.join("pocket-english.tsv").exists());
        assert!(
            folder.join("Mine.tsv").exists(),
            "another dictionary is left alone"
        );
        remove("Mine", &place).expect("removed by its file's name");
        assert!(held_in(&folder).is_empty());
        remove("Mine", &place).expect("removing what is not there is not a failure");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installing_a_name_again_replaces_it_and_clears_parts_it_no_longer_needs() {
        let folder = scratch("replace");
        let place = folder_place(&folder);
        // What an earlier, larger install of the same name left.
        for part in ["pocket.01.tsv", "pocket.02.tsv", "pocket.03.tsv"] {
            fs::write(folder.join(part), "# name=Pocket\nold\tAn old entry.\n").expect("old");
        }
        fs::write(
            folder.join("other.tsv"),
            "# name=Other\nkept\tStill here.\n",
        )
        .expect("other");
        let source = folder.join("pocket.tsv.source");
        fs::write(&source, "# name=Pocket\nnew\tA new entry.\n").expect("source");
        install(&source.display().to_string(), &Request::default(), &place).expect("installed");
        let files: Vec<String> = held_in(&folder).into_iter().map(|held| held.file).collect();
        assert_eq!(files, ["other.tsv", "pocket.tsv"]);
        assert!(fs::read_to_string(folder.join("pocket.tsv"))
            .expect("read")
            .contains("new\tA new entry."));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_large_dictionary_is_split_between_headwords_into_files_the_reader_loads() {
        let definition = "d".repeat(4_000);
        let mut entries = Vec::new();
        for number in 0..5_000 {
            entries.push((format!("word{number:05}"), definition.clone()));
            // A second sense of the same headword, which must stay beside it.
            entries.push((format!("Word{number:05}"), definition.clone()));
        }
        let large = Prepared {
            name: "Large".to_owned(),
            language: "en".to_owned(),
            priority: 3,
            entries,
            notes: Vec::new(),
        };
        let parts = split(&large).expect("split");
        assert!(parts.len() > 1, "forty megabytes do not fit one file");
        assert_eq!(parts[0].file, "large.01.tsv");
        for part in &parts {
            assert!(part.text.len() <= MAX_DICTIONARY_BYTES, "{}", part.file);
            assert!(part
                .text
                .starts_with("# name=Large\n# language=en\n# priority=3\n"));
            let lines: Vec<&str> = part.text.lines().filter(|l| !l.starts_with('#')).collect();
            assert_eq!(lines.len() % 2, 0, "{} ends between two senses", part.file);
            assert!(
                lines[lines.len() - 1].starts_with("Word"),
                "{} ends between two senses",
                part.file
            );
        }
        let checked = read_back(&large, &parts).expect("read back");
        assert_eq!(checked.entries, 10_000);
        assert_eq!(checked.samples.len(), SAMPLES);
        assert!(parts.iter().all(|part| is_part_of(&part.file, "large")));
    }

    #[test]
    fn a_dictionary_too_large_for_any_reader_is_refused_with_the_numbers() {
        let definition = "d".repeat(4_000);
        let entries: Vec<(String, String)> = (0..17_000)
            .map(|number| (format!("word{number:05}"), definition.clone()))
            .collect();
        let huge = Prepared {
            name: "Huge".to_owned(),
            language: "und".to_owned(),
            priority: 0,
            entries,
            notes: Vec::new(),
        };
        let error = split(&huge).expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_UNSUPPORTED);
        assert!(
            error.contains("32 dictionary files of 8 MB each and 64 MB in all"),
            "{error}"
        );
        assert!(error.contains("Huge is 6"), "{error}");
    }

    #[test]
    fn a_dictionary_that_would_not_load_beside_what_is_installed_is_refused_first() {
        let folder = scratch("crowded");
        let place = folder_place(&folder);
        for number in 0..MAX_DICTIONARIES {
            fs::write(
                folder.join(format!("held{number:02}.tsv")),
                format!("# name=Held {number}\nword\tDefinition {number}.\n"),
            )
            .expect("held");
        }
        let source = folder.join("new.source");
        fs::write(&source, "# name=Newcomer\nword\tOne more.\n").expect("source");
        let error = install(&source.display().to_string(), &Request::default(), &place)
            .expect_err("refused");
        assert!(error.contains("already holds"), "{error}");
        assert!(error.contains("kobo dict remove NAME"), "{error}");
        assert!(!folder.join("newcomer.tsv").exists(), "nothing was written");
        // The reader's loader agrees about how many it takes.
        assert_eq!(
            kobo_dict::Index::load_directory(&folder).len(),
            MAX_DICTIONARIES
        );
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_files_counted_as_loaded_are_the_ones_the_reader_loads() {
        let folder = scratch("loaded");
        let line = format!("word\t{}\n", "d".repeat(4_000));
        let big = line.repeat(MAX_DICTIONARY_BYTES / line.len() + 1);
        fs::write(folder.join("a-too-big.tsv"), &big).expect("too big");
        fs::write(folder.join("b-fits.tsv"), "word\tA definition.\n").expect("fits");
        let files: Vec<(String, u64)> = held_in(&folder)
            .into_iter()
            .map(|held| (held.file, held.bytes))
            .collect();
        let loads = loaded(&files);
        assert_eq!(loads.into_iter().collect::<Vec<_>>(), ["b-fits.tsv"]);
        assert_eq!(kobo_dict::Index::load_directory(&folder).len(), 1);
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_cobalt_dictionary_keeps_its_own_name_and_loses_only_lines_the_reader_would_drop() {
        let folder = scratch("tsv");
        let source = folder.join("glossary.tsv");
        fs::write(
            &source,
            format!(
                "# name=Ship's glossary\n# language=en\n# priority=7\n\
                 abaft\tToward the stern.\r\nbroken line\n\n\
                 long\t{}\nbeam\tThe width of a ship.\n",
                "d".repeat(MAX_DEFINITION_BYTES + 1)
            ),
        )
        .expect("source");
        let read = prepare(&source.display().to_string(), &Request::default()).expect("read");
        assert_eq!(
            (read.name.as_str(), read.language.as_str(), read.priority),
            ("Ship's glossary", "en", 7)
        );
        assert_eq!(read.entries.len(), 2);
        assert_eq!(read.notes.len(), 1);
        assert!(
            read.notes[0].starts_with("2 lines are not"),
            "{:?}",
            read.notes
        );
        let renamed = prepare(
            &source.display().to_string(),
            &Request {
                name: Some("  Sea words\n".to_owned()),
                language: Some("en-GB".to_owned()),
                ..Request::default()
            },
        )
        .expect("read");
        assert_eq!(
            (renamed.name.as_str(), renamed.language.as_str()),
            ("Sea words", "en-GB")
        );
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_file_that_is_not_a_dictionary_is_refused_with_what_one_is() {
        let folder = scratch("refused");
        let photo = folder.join("photo.jpg");
        fs::write(&photo, [0xff, 0xd8, 0xff, 0xe0]).expect("a photo");
        let error = prepare(&photo.display().to_string(), &Request::default()).expect_err("no");
        assert!(error.contains("is not a dictionary this reads"), "{error}");
        let prose = folder.join("notes.txt");
        fs::write(&prose, "no tabs anywhere\n").expect("prose");
        let error = prepare(&prose.display().to_string(), &Request::default()).expect_err("no");
        assert!(
            error.contains("holds no entry the reader would keep"),
            "{error}"
        );
        let part = folder.join("oxford.dict.dz");
        fs::write(&part, b"x").expect("a part");
        let error = prepare(&part.display().to_string(), &Request::default()).expect_err("no");
        assert!(error.contains("Name the .ifo file beside it"), "{error}");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn the_command_line_names_one_place_and_refuses_what_it_does_not_know() {
        let request = parse(&words(&["oxford.ifo", "--name", "Oxford", "--sim"])).expect("read");
        assert_eq!(request.subject.as_deref(), Some("oxford.ifo"));
        assert_eq!(request.name.as_deref(), Some("Oxford"));
        assert!(matches!(request.place, Some(Place::Folder { .. })));
        let request = parse(&words(&["-s", "192.168.1.9"])).expect("read");
        assert_eq!(request.place, Some(Place::Device("192.168.1.9".to_owned())));
        for (line, said) in [
            (&["--sim", "--device", "192.168.1.9"][..], "one place"),
            (&["--device", "-oProxyCommand=x"][..], "not an address"),
            (&["--language", "english language"][..], "short code"),
            (&["a.tsv", "b.tsv"][..], "does not know b.tsv"),
            (
                &["--volume", "/nonexistent-kobo-volume"][..],
                "not a Kobo's card",
            ),
            (&["--name"][..], "takes a name"),
        ] {
            let error = parse(&words(line)).expect_err("refused");
            assert!(error.contains(said), "{line:?}: {error}");
        }
        let error = command(&words(&["check", "a.tsv", "--sim"])).expect_err("refused");
        assert_eq!(console::category_of(&error), console::EXIT_USAGE);
        let error = command(&words(&["install", "a.tsv"])).expect_err("no place");
        assert!(error.contains("--volume PATH"), "{error}");
    }

    #[test]
    fn a_reader_mounted_over_usb_takes_a_dictionary_in_the_folder_the_runtime_reads() {
        let volume = scratch("volume");
        fs::create_dir_all(volume.join(".kobo")).expect("a card");
        let Place::Folder { folder, label } = mounted(&volume).expect("a reader") else {
            panic!("a mounted reader is a folder");
        };
        assert!(
            folder.ends_with(".adds/cobalt/dictionaries"),
            "{}",
            folder.display()
        );
        assert!(label.starts_with("the reader mounted at"));
        let _ = fs::remove_dir_all(&volume);
    }

    #[test]
    fn file_names_are_made_from_the_name_and_recognised_again() {
        assert_eq!(slug("Oxford English (2nd ed.)"), "oxford-english-2nd-ed");
        assert_eq!(slug("Русский словарь"), "русский-словарь");
        assert_eq!(slug("!!!"), "dictionary");
        assert!(is_part_of("oxford.tsv", "oxford"));
        assert!(is_part_of("oxford.07.tsv", "oxford"));
        assert!(!is_part_of("oxford-english.tsv", "oxford"));
        assert!(!is_part_of("oxford.notes.tsv", "oxford"));
        assert!(plain_name("oxford.tsv"));
        assert!(!plain_name("../oxford.tsv") && !plain_name(".hidden.tsv"));
    }

    #[test]
    fn a_reader_listing_is_read_into_files_with_their_names_and_sizes() {
        let said = "file=oxford.01.tsv\nbytes=8388000\nmeta=# name=Oxford\nmeta=# language=en\n\
                    end\nfile=oxford.02.tsv\nbytes=120\nmeta=# name=Oxford\r\nend\n\
                    file=Mine.tsv\nbytes=40\nend\nfile=../escape.tsv\nbytes=1\nend\nlisted\n";
        let held = held_from(said);
        assert_eq!(
            held.len(),
            3,
            "a name that points elsewhere is not acted on"
        );
        assert_eq!(held[0].file, "Mine.tsv");
        assert_eq!(
            (held[0].name.as_str(), held[0].language.as_str()),
            ("Mine", "und")
        );
        assert_eq!(
            (held[1].name.as_str(), held[1].bytes),
            ("Oxford", 8_388_000)
        );
        assert_eq!(held[2].name, "Oxford");
        let shelves = shelves(&held);
        assert_eq!(shelves.len(), 2);
        assert_eq!(shelves[1].1.len(), 2, "both parts are one dictionary");
    }

    /// Runs a reader script under this computer's shell with the
    /// dictionaries folder pointed at `folder`.
    #[cfg(unix)]
    fn run(folder: &Path, script: Vec<u8>) -> String {
        let script = String::from_utf8(script)
            .expect("a script is text")
            .replace(DEVICE_DICTIONARIES, &folder.to_string_lossy());
        let mut shell = std::process::Command::new("sh")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("sh runs");
        let mut input = shell.stdin.take().expect("a pipe");
        let writer = std::thread::spawn(move || {
            let _ = input.write_all(script.as_bytes());
        });
        let output = shell.wait_with_output().expect("sh ends");
        writer.join().expect("the script was written");
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[cfg(unix)]
    #[test]
    fn the_reader_scripts_install_list_and_replace_a_dictionary_whole() {
        let folder = scratch("script");
        let pocket = prepared(
            "Pocket",
            &[("ink", "A coloured fluid."), ("page", "A leaf.")],
        );
        let parts = split(&pocket).expect("split");
        let mut script = Vec::new();
        let texts: Vec<String> = parts.iter().map(|part| part.text.clone()).collect();
        write_install(&mut script, &install_script(&parts, &[], 1), &texts).expect("written");
        assert!(run(&folder, script).contains("installed=1"));
        fs::write(folder.join("stale.tsv"), "# name=Pocket\nold\tOld.\n").expect("stale");

        let listed = held_from(&run(&folder, listing_script().into_bytes()));
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].file, "pocket.tsv");
        assert_eq!(listed[0].name, "Pocket");
        assert_eq!(listed[0].bytes, parts[0].text.len() as u64);

        // A reinstall whose copy arrives wrong leaves what was there.
        let mut wrong = parts.clone();
        wrong[0].text.push_str("extra\tNot what was hashed.\n");
        let mut script = Vec::new();
        write_install(
            &mut script,
            &install_script(&parts, &["stale.tsv".to_owned()], 1),
            &[wrong[0].text.clone()],
        )
        .expect("written");
        assert!(run(&folder, script).contains("damaged"));
        assert!(folder.join("stale.tsv").exists(), "nothing was removed");
        assert_eq!(
            fs::read_to_string(folder.join("pocket.tsv")).expect("kept"),
            parts[0].text
        );

        let mut script = Vec::new();
        write_install(
            &mut script,
            &install_script(&parts, &["stale.tsv".to_owned()], 1),
            &texts,
        )
        .expect("written");
        assert!(run(&folder, script).contains("installed=1"));
        assert!(!folder.join("stale.tsv").exists(), "the stale part went");
        let left: Vec<String> = fs::read_dir(&folder)
            .expect("listed")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["pocket.tsv"], "no hidden upload is left behind");

        let mut script = Vec::new();
        write_install(
            &mut script,
            &install_script(&parts, &[], 4_000_000_000_000_000),
            &texts,
        )
        .expect("written");
        assert!(run(&folder, script).contains("full="));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_dictionary_name_never_reaches_the_readers_shell_as_words() {
        let parts = vec![Part {
            file: "x'; rm -rf $HOME #.tsv".to_owned(),
            text: "word\tDefinition.\n".to_owned(),
        }];
        let script = install_script(&parts, &["old'; reboot #.tsv".to_owned()], 1);
        assert!(!script.contains("rm -rf $HOME") && !script.contains("reboot"));
        assert!(script.contains("/mnt/onboard/.adds/cobalt/dictionaries"));
        assert!(script.ends_with("}\n"), "the upload follows the function");
    }
}
