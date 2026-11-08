//! `kobo clip`: a web article saved as a book, made on this computer.
//!
//! The page is read the way a browser's reader mode reads it and written as
//! an EPUB the reader opens like any other book. Nothing is sent to a reader
//! yet; the book is a file the owner can send or copy.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const USAGE: &str = "\
usage: kobo clip PAGE.html --url URL [--out FILE.epub] [--colour] [--no-pictures]
       kobo clip URL [--out FILE.epub] [--colour] [--no-pictures]

Saves a web article as an EPUB, as a browser's reader mode shows it: the
text, headings, lists, tables, code, and figures with their captions, with
the menus, advertisements, share buttons and cookie notices left out. The
first page names the site, the author and the date, and links back to the
page.

Give a saved page with the address it came from, or an address to fetch.
Pictures are fetched, fitted to the reader's screen and turned grey;
--colour keeps their colour, and --no-pictures leaves them out. --out names
the file or a folder for it; by default the book is named after the article
and saved in the current folder.";

struct Request {
    source: Source,
    out: Option<PathBuf>,
    options: kobo_article::Options,
}

enum Source {
    File { path: PathBuf, url: String },
    Address(String),
}

pub fn command(arguments: &[String]) -> Result<(), String> {
    if super::wants_help(arguments) {
        return super::print_command_help(USAGE);
    }
    let request = parse(arguments)?;
    // A wrong --out is said before anything is fetched.
    let target = destination(request.out.as_deref())?;
    let fetch = kobo_article::Web;
    let (html, page) = match &request.source {
        Source::File { path, url } => {
            let bytes = read_page(path)?;
            let html = kobo_article::page_text(&bytes).map_err(|refusal| refusal.to_string())?;
            (html, url.clone())
        }
        Source::Address(address) => {
            eprintln!("Fetching {address}");
            let html =
                kobo_article::fetch_page(address, &fetch).map_err(|refusal| refusal.to_string())?;
            (html, address.clone())
        }
    };
    let clipping = kobo_article::clip(&html, &page, &fetch, &request.options)
        .map_err(|refusal| refusal.to_string())?;
    let path = publish(&target, &clipping.file_name, &clipping.epub)?;
    let pictures = match clipping.pictures {
        1 => "1 picture".to_owned(),
        count => format!("{count} pictures"),
    };
    println!(
        "Saved {} ({pictures}, {})",
        path.display(),
        size(clipping.epub.len())
    );
    for note in &clipping.notes {
        println!("{note}");
    }
    Ok(())
}

fn parse(arguments: &[String]) -> Result<Request, String> {
    let mut source = None;
    let mut url = None;
    let mut out = None;
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
