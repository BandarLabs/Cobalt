//! `kobo clip`: a web article saved as a book, made on this computer.
//!
//! The page is read the way a browser's reader mode reads it and written as
//! an EPUB the reader opens like any other book. Nothing is sent to a reader
//! yet; the book is a file the owner can send or copy.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

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
    destination(request.out.as_deref(), "Article.epub")?;
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
    let path = destination(request.out.as_deref(), &clipping.file_name)?;
    write_atomically(&path, &clipping.epub)?;
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

/// Where the book goes. A named file is replaced; a name made from the title
/// never replaces an earlier clip, because two articles can share a title.
fn destination(out: Option<&Path>, file_name: &str) -> Result<PathBuf, String> {
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
            Ok(path.to_path_buf())
        }
        folder => {
            let folder = folder.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            Ok(unused(&folder, file_name))
        }
    }
}

fn unused(folder: &Path, file_name: &str) -> PathBuf {
    let first = folder.join(file_name);
    if !first.exists() {
        return first;
    }
    let stem = file_name.strip_suffix(".epub").unwrap_or(file_name);
    (2..10_000)
        .map(|number| folder.join(format!("{stem} ({number}).epub")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Writes beside the destination and renames, so a failed write never
/// leaves half a book where a whole one was.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let folder = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = folder.join(format!(".kobo-clip-{}.part", std::process::id()));
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

    #[test]
    fn a_book_named_after_its_title_never_replaces_an_earlier_one() {
        let folder = std::env::temp_dir().join(format!("kobo-clip-test-{}", std::process::id()));
        fs::create_dir_all(&folder).expect("a scratch folder");
        let first = destination(Some(&folder), "Tides.epub").expect("a destination");
        fs::write(&first, b"one").expect("written");
        let second = destination(Some(&folder), "Tides.epub").expect("a destination");
        assert_eq!(second, folder.join("Tides (2).epub"));
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_named_destination_must_be_an_epub() {
        assert!(destination(Some(Path::new("out.txt")), "A.epub").is_err());
        assert_eq!(
            destination(Some(Path::new("out.EPUB")), "A.epub").expect("an epub"),
            PathBuf::from("out.EPUB")
        );
    }
}
