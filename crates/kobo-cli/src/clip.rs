//! `kobo clip`: a web article made into a book and sent to the reader.
//!
//! The page is read the way a browser's reader mode reads it and written as
//! an EPUB the reader opens like any other book. It goes to the reader's
//! Articles shelf over the connection `kobo send` uses, or waits on this
//! computer while the reader is asleep; `--out` saves it to a folder
//! instead. The browser extension and the Safari service reach the same code
//! through `native-host` and `install`.

use super::clip_delivery as delivery;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const USAGE: &str = "\
usage: kobo clip URL [--sim | --device IP | --reader NAME] [--colour] [--no-pictures]
       kobo clip PAGE.html --url URL [same options]
       kobo clip URL --out FILE.epub|FOLDER
       kobo clip URL --later
       kobo clip sync [--sim | --device IP | --reader NAME]
       kobo clip install --browser chrome|edge|brave|chromium|firefox|safari | --all
       kobo clip uninstall [--browser NAME | --all]

Makes a web article into an EPUB, as a browser's reader mode shows it: the
text, headings, lists, tables, code, and figures with their captions, with
the menus, advertisements, share buttons and cookie notices left out. The
first page names the site, the author and the date, and links back to the
page. Pictures are fitted to the reader's screen and turned grey; --colour
keeps their colour, and --no-pictures leaves them out.

The book goes to your Kobo's Articles shelf, the saved reader unless a
target is named. A reader that is asleep or away gets it later: it waits on
this computer until kobo clip sync, Sync on the reader's Articles shelf, or
the next clip that finds the reader awake. --later keeps it waiting without
trying now. --out saves it to a file or folder on this computer instead and
sends nothing.

install adds Send to Kobo to a browser's right-click menu, and uninstall
takes it away again.";

struct Request {
    source: Source,
    out: Option<PathBuf>,
    /// Keep it for the reader rather than saving it to a folder.
    later: bool,
    options: kobo_article::Options,
}

/// The largest book that goes to the reader: the most Books can open, so
/// pictures past it are left out with a note rather than the article
/// arriving as a book that will not open there.
const READER_BOOK_BYTES: usize = kobo_protocol::MAX_LIBRARY_DOCUMENT_BYTES;

pub(crate) enum Source {
    File {
        path: PathBuf,
        url: String,
    },
    /// A page a browser already rendered, as the extension sends it.
    Html {
        html: String,
        url: String,
    },
    Address(String),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    match arguments
        .split_first()
        .map(|(verb, rest)| (verb.as_str(), rest))
    {
        Some(("sync", rest)) => return sync(rest),
        Some(("native-host", _)) => return super::clip_host::run(),
        Some(("install", rest)) => return super::clip_install::install(rest),
        Some(("uninstall", rest)) => return super::clip_install::uninstall(rest),
        _ => {}
    }
    let notify = arguments.iter().any(|word| word == "--notify");
    let arguments: Vec<String> = arguments
        .iter()
        .filter(|word| *word != "--notify")
        .cloned()
        .collect();
    let result = clip(&arguments);
    if notify {
        // The Safari service has no terminal to print to, so the outcome is
        // a notification instead.
        let said = match &result {
            Ok(said) => said.clone(),
            Err(error) => format!(
                "Could not send to your Kobo. {}",
                super::console::display(error)
            ),
        };
        notify_on_this_computer(&said);
    }
    result.map(|_| ())
}

/// Clips, then saves, keeps or sends, and returns the line that says which.
fn clip(arguments: &[String]) -> Result<String, String> {
    let (target, rest) = super::targets::TargetArgs::parse(arguments)?;
    let mut request = parse(&rest)?;
    if request.out.is_some() && !target.is_empty() {
        return Err(
            "--out saves the book in a folder on this computer; leave out --sim, --device and --reader"
                .to_owned(),
        );
    }
    if !target.is_empty() {
        // A wrong target is said before anything is fetched.
        target.resolve()?;
    }
    if request.out.is_none() {
        request.options.max_book_bytes = READER_BOOK_BYTES;
    }
    // A wrong --out is said before anything is fetched too.
    let folder = request
        .out
        .as_deref()
        .map(|out| destination(Some(out)))
        .transpose()?;
    let clipping = make(&request.source, &request.options)?;
    let pictures = match clipping.pictures {
        1 => "1 picture".to_owned(),
        count => format!("{count} pictures"),
    };
    let said = if let Some(folder) = folder {
        let path = publish(&folder, &clipping.file_name, &clipping.epub)?;
        format!(
            "Saved {} ({pictures}, {})",
            path.display(),
            size(clipping.epub.len())
        )
    } else {
        let article = article_of(&clipping);
        let queue = delivery::queue_folder();
        let landing = if request.later {
            delivery::enqueue(&queue, &article)?;
            delivery::Landing::Waiting { why: String::new() }
        } else {
            delivery::send_now(&queue, &article, reader_for(&target))?
        };
        match landing {
            delivery::Landing::Landed { also, .. } => {
                let mut said = format!(
                    "Landed on your Kobo: {} ({pictures}, {}).",
                    clipping.title,
                    size(clipping.epub.len())
                );
                if also > 0 {
                    said.push_str(&format!(
                        " {} that had been waiting went with it.",
                        delivery::articles_word(also)
                    ));
                }
                said
            }
            delivery::Landing::Waiting { why } => format!(
                "Waiting for your Kobo: {}. {why}{}It goes over with kobo clip sync, with Sync on the reader's Articles shelf, or with the next clip that finds the reader awake.",
                clipping.title,
                if why.is_empty() { "" } else { " " }
            ),
        }
    };
    println!("{said}");
    for note in &clipping.notes {
        println!("{note}");
    }
    Ok(said)
}

/// The clipped book as the queue and the reader take it.
pub(crate) fn article_of(clipping: &kobo_article::Clipping) -> delivery::Article {
    delivery::Article {
        title: clipping.title.clone(),
        site: clipping.site.clone(),
        file_name: clipping.file_name.clone(),
        epub: clipping.epub.clone(),
    }
}

/// Reads the page and makes the book.
pub(crate) fn make(
    source: &Source,
    options: &kobo_article::Options,
) -> Result<kobo_article::Clipping, String> {
    let fetch = kobo_article::Web;
    match source {
        Source::File { path, url } => {
            let bytes = read_page(path)?;
            let html = kobo_article::page_text(&bytes).map_err(|refusal| refusal.to_string())?;
            kobo_article::clip(&html, url, &fetch, options)
        }
        Source::Html { html, url } => kobo_article::clip(html, url, &fetch, options),
        Source::Address(address) => {
            eprintln!("Fetching {address}");
            // Read against wherever the address redirected to, so the page's
            // pictures and links resolve as they did in the browser.
            kobo_article::clip_address(address, &fetch, options)
        }
    }
    .map_err(|refusal| refusal.to_string())
}

/// Options for a book that goes to the reader.
pub(crate) fn reader_options() -> kobo_article::Options {
    kobo_article::Options {
        max_book_bytes: READER_BOOK_BYTES,
        ..kobo_article::Options::default()
    }
}

/// Said when there is no reader to send to at all.
const NO_READER: &str =
    "No reader is saved on this computer yet. Run kobo setup with the reader plugged in.";

/// The reader to send to, found without waiting for it to be woken: a
/// reader that is asleep is the article waiting for later, not a minute of
/// someone looking at a browser notification.
pub(crate) fn reader_for(target: &super::targets::TargetArgs) -> Result<delivery::Reader, String> {
    use super::targets::{DefaultReader, Target};
    let saved = |name: &str| {
        super::targets::resolve_nickname_now(name)
            .map(delivery::Reader::Device)
            .map_err(|error| {
                if error.contains("No reader is saved") {
                    super::console::display(&error).to_string()
                } else {
                    delivery::AWAY.to_owned()
                }
            })
    };
    if !target.is_empty() {
        return match target.resolve()? {
            Target::Simulator => Ok(delivery::Reader::Simulator),
            Target::Address(host) if super::valid_device_host(&host) => {
                Ok(delivery::Reader::Device(host))
            }
            Target::Address(host) => Err(format!("{host:?} is not an address this can reach")),
            Target::Nickname(name) => saved(&name),
        };
    }
    match super::targets::configured() {
        Ok(DefaultReader::Address(address)) => Ok(delivery::Reader::Device(address)),
        Ok(DefaultReader::Nickname(name)) => saved(&name),
        Err(_) => Err(NO_READER.to_owned()),
    }
}

/// A notification on this computer, for the Safari service. macOS only, and
/// quiet if it cannot be shown: the article itself is already decided.
fn notify_on_this_computer(said: &str) {
    if cfg!(target_os = "macos") {
        // Passed as arguments, so no character of a title is read as script.
        let _ = std::process::Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                "Send to Kobo",
                said,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// How long one sync keeps sending. Sync on the reader waits for the answer,
/// and the runtime gives a request three minutes.
const SYNC_PATIENCE: std::time::Duration = std::time::Duration::from_secs(100);

/// `kobo clip sync`: sends what is waiting to a reader.
///
/// `--json` prints one line instead of sentences. It is how the Sidekick
/// helper runs this for Sync on the reader, and hands the line to Books.
fn sync(arguments: &[String]) -> Result<(), String> {
    let (target, rest) = super::targets::TargetArgs::parse(arguments)?;
    let json = match rest.as_slice() {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => return Err(USAGE.to_owned()),
    };
    let reader = match target.resolve()? {
        super::targets::Target::Simulator => delivery::Reader::Simulator,
        super::targets::Target::Address(host) => {
            if !super::valid_device_host(&host) {
                return Err(format!("{host:?} is not an address this can reach"));
            }
            delivery::Reader::Device(host)
        }
        super::targets::Target::Nickname(name) => {
            delivery::Reader::Device(super::targets::resolve_nickname(&name)?)
        }
    };
    let report = delivery::sync(&delivery::queue_folder(), &reader, SYNC_PATIENCE);
    if json {
        println!("{}", report.to_json());
        return Ok(());
    }
    if let Some(detail) = &report.detail {
        eprintln!("{detail}");
    }
    if report.delivered.is_empty() && report.waiting == 0 {
        println!("Nothing is waiting to go to your Kobo.");
        return Ok(());
    }
    if !report.delivered.is_empty() {
        println!(
            "Landed on your Kobo: {}.",
            delivery::articles_word(report.delivered.len())
        );
        for title in &report.delivered {
            println!("  {title}");
        }
    }
    if report.waiting > 0 {
        let reason = report.problem.as_deref().unwrap_or("");
        return Err(format!(
            "{} still waiting for {}. {reason}",
            delivery::articles_word(report.waiting),
            reader.label()
        )
        .trim_end()
        .to_owned());
    }
    Ok(())
}

fn parse(arguments: &[String]) -> Result<Request, String> {
    let mut source = None;
    let mut url = None;
    let mut out = None;
    let mut later = false;
    let mut options = kobo_article::Options::default();
    let mut rest = arguments.iter();
    while let Some(word) = rest.next() {
        match word.as_str() {
            "--url" => url = Some(rest.next().ok_or("--url takes the page's address")?.clone()),
            "--out" => {
                out = Some(PathBuf::from(
                    rest.next().ok_or("--out takes a file or folder")?,
                ));
            }
            "--later" => later = true,
            "--colour" | "--color" => options.colour = true,
            "--no-pictures" => options.pictures = false,
            flag if flag.starts_with("--") => {
                return Err(format!("kobo clip does not know {flag}\n\n{USAGE}"));
            }
            _ if source.is_none() => source = Some(word.clone()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let source = source.ok_or_else(|| USAGE.to_owned())?;
    if later && out.is_some() {
        return Err(
            "--later keeps the book for the reader and --out saves it to a folder; choose one"
                .to_owned(),
        );
    }
    let source = if kobo_article::address::is_web_address(&source) {
        if url.is_some() {
            return Err(
                "--url is for a saved page; an address to fetch is already the page's address"
                    .to_owned(),
            );
        }
        Source::Address(source)
    } else {
        let url = url.ok_or_else(|| {
            format!(
                "{source} is a saved page, so kobo clip needs the address it came from: \
                 add --url https://..."
            )
        })?;
        if !kobo_article::address::is_web_address(&url) {
            return Err(format!(
                "{url} is not a web address. Give one starting with https://."
            ));
        }
        Source::File {
            path: PathBuf::from(source),
            url,
        }
    };
    Ok(Request {
        source,
        out,
        later,
        options,
    })
}

fn read_page(path: &Path) -> Result<Vec<u8>, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if metadata.is_dir() {
        return Err(format!("{} is a folder, not a saved page", path.display()));
    }
    let limit = kobo_article::MAX_PAGE_BYTES as u64;
    if metadata.len() > limit {
        return Err(kobo_article::Refusal::PageTooLarge {
            bytes: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
            limit: kobo_article::MAX_PAGE_BYTES,
        }
        .to_string());
    }
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))
}

/// Where the book goes.
#[derive(Debug, PartialEq, Eq)]
enum Destination {
    /// A file `--out` named, which the owner asked to have replaced.
    Named(PathBuf),
    /// A name made from the title in this folder, which never replaces
    /// anything, because two articles can share a title.
    Fresh(PathBuf),
}

fn destination(out: Option<&Path>) -> Result<Destination, String> {
    match out {
        Some(path) if !path.is_dir() => {
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("epub"))
            {
                return Err(format!(
                    "{} should end in .epub, or name a folder",
                    path.display()
                ));
            }
            Ok(Destination::Named(path.to_path_buf()))
        }
        folder => Ok(Destination::Fresh(
            folder.map_or_else(|| PathBuf::from("."), Path::to_path_buf),
        )),
    }
}

/// How many numbered names are tried before saying the folder is full of
/// books with this title.
const MAX_NUMBERED: u32 = 9_999;

fn publish(destination: &Destination, file_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    match destination {
        Destination::Named(path) => {
            write_atomically(path, bytes)?;
            Ok(path.clone())
        }
        Destination::Fresh(folder) => publish_fresh(folder, file_name, bytes, MAX_NUMBERED),
    }
}

/// A temporary name no other clip, in this process or another, is using.
fn temporary_in(folder: &Path) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    folder.join(format!(".kobo-clip-{}-{number}.part", std::process::id()))
}

/// Saves under the title's name, or the first numbered name after it that
/// is free, without ever replacing a file.
///
/// The book is written in full under a temporary name first and then linked
/// to its name, which fails rather than replaces when a file is already
/// there: so a second clip with the same title, even one running at the same
/// moment, takes the next number instead of overwriting the first. A folder
/// that cannot hold links, such as a FAT card, gets the file created only if
/// the name is free.
fn publish_fresh(
    folder: &Path,
    file_name: &str,
    bytes: &[u8],
    numbered: u32,
) -> Result<PathBuf, String> {
    let stem = file_name.strip_suffix(".epub").unwrap_or(file_name);
    let names = std::iter::once(file_name.to_owned())
        .chain((2..=numbered.saturating_add(1)).map(|number| format!("{stem} ({number}).epub")));
    let temporary = temporary_in(folder);
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot write in {}: {error}", folder.display()));
    }
    let mut result = None;
    for name in names {
        let path = folder.join(&name);
        match link_or_create(&temporary, &path, bytes) {
            Ok(()) => {
                result = Some(Ok(path));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                result = Some(Err(format!("cannot write {}: {error}", path.display())));
                break;
            }
        }
    }
    let _ = fs::remove_file(&temporary);
    result.unwrap_or_else(|| {
        Err(format!(
            "{} already holds {file_name} and {numbered} numbered copies of it; \
             name the book with --out",
            folder.display()
        ))
    })
}

/// Gives the finished book its name, failing with `AlreadyExists` if the
/// name is taken.
fn link_or_create(temporary: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    match fs::hard_link(temporary, path) {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => {
            // No links here. Creating the file only when it is new keeps the
            // promise not to replace anything; a failed write is taken back.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            let written = file.write_all(bytes).and_then(|()| file.sync_all());
            if written.is_err() {
                let _ = fs::remove_file(path);
            }
            written
        }
        other => other,
    }
}

/// Writes beside the destination and renames, so a failed write never
/// leaves half a book where a whole one was.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let folder = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = temporary_in(folder);
    let written = fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(format!("cannot write {}: {error}", path.display()));
    }
    Ok(())
}

fn size(bytes: usize) -> String {
    if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        let tenths = bytes * 10 / (1024 * 1024);
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn a_saved_page_needs_the_address_it_came_from() {
        let Err(said) = parse(&words("page.html")) else {
            panic!("a saved page without an address was accepted");
        };
        assert!(said.contains("--url"), "{said}");
        let request = parse(&words("page.html --url https://example.org/a --colour"))
            .expect("a saved page with its address");
        assert!(request.options.colour);
        assert!(matches!(request.source, Source::File { .. }));
    }

    #[test]
    fn an_address_is_fetched_and_takes_no_second_address() {
        let request = parse(&words("https://example.org/a --no-pictures")).expect("an address");
        assert!(!request.options.pictures);
        assert!(matches!(request.source, Source::Address(_)));
        assert!(parse(&words("https://example.org/a --url https://example.org/b")).is_err());
    }

    #[test]
    fn later_keeps_the_book_for_the_reader_and_cannot_also_save_it() {
        let request = parse(&words("https://example.org/a --later")).expect("later");
        assert!(request.later);
        assert!(parse(&words("https://example.org/a --later --out a.epub")).is_err());
    }

    #[test]
    fn an_unknown_flag_is_named_back() {
        let Err(said) = parse(&words("https://example.org/a --kindle")) else {
            panic!("an unknown flag was accepted");
        };
        assert!(
            said.starts_with("kobo clip does not know --kindle"),
            "{said}"
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("kobo-clip-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).expect("a scratch folder");
        folder
    }

    #[test]
    fn a_book_named_after_its_title_never_replaces_an_earlier_one() {
        let folder = scratch("again");
        let fresh = destination(Some(&folder)).expect("a destination");
        let first = publish(&fresh, "Tides.epub", b"one").expect("saved");
        let second = publish(&fresh, "Tides.epub", b"two").expect("saved");
        assert_eq!(first, folder.join("Tides.epub"));
        assert_eq!(second, folder.join("Tides (2).epub"));
        assert_eq!(fs::read(&first).expect("kept"), b"one");
        assert_eq!(fs::read(&second).expect("kept"), b"two");
        let left: Vec<_> = fs::read_dir(&folder).expect("listed").collect();
        assert_eq!(left.len(), 2, "no temporary file is left behind");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn clips_of_the_same_title_at_the_same_moment_each_keep_their_own_book() {
        let folder = scratch("together");
        let saved: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8u8)
                .map(|index| {
                    let folder = folder.clone();
                    scope.spawn(move || {
                        publish(&Destination::Fresh(folder), "Tides.epub", &[index; 64])
                            .expect("saved")
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("joined"))
                .collect()
        });
        let distinct: std::collections::BTreeSet<_> = saved.iter().collect();
        assert_eq!(distinct.len(), 8, "{saved:?}");
        let mut contents: Vec<u8> = saved
            .iter()
            .map(|path| fs::read(path).expect("kept")[0])
            .collect();
        contents.sort_unstable();
        assert_eq!(contents, (0..8).collect::<Vec<u8>>());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_folder_full_of_the_title_says_so_instead_of_replacing_a_book() {
        let folder = scratch("full");
        for name in ["Tides.epub", "Tides (2).epub", "Tides (3).epub"] {
            fs::write(folder.join(name), b"kept").expect("written");
        }
        let said = publish_fresh(&folder, "Tides.epub", b"new", 2).expect_err("full");
        assert!(said.contains("name the book with --out"), "{said}");
        for name in ["Tides.epub", "Tides (2).epub", "Tides (3).epub"] {
            assert_eq!(fs::read(folder.join(name)).expect("kept"), b"kept");
        }
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_named_book_is_replaced_as_asked() {
        let folder = scratch("named");
        let path = folder.join("Mine.epub");
        fs::write(&path, b"old").expect("written");
        let named = destination(Some(&path)).expect("a destination");
        assert_eq!(publish(&named, "Tides.epub", b"new").expect("saved"), path);
        assert_eq!(fs::read(&path).expect("read"), b"new");
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_named_destination_must_be_an_epub() {
        assert!(destination(Some(Path::new("out.txt"))).is_err());
        assert_eq!(
            destination(Some(Path::new("out.EPUB"))).expect("an epub"),
            Destination::Named(PathBuf::from("out.EPUB"))
        );
    }
}
