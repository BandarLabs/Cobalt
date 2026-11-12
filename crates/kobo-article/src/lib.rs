//! Turning a web page into a book for the reader.
//!
//! The owner sees an article in a browser and wants it on the Kobo, read the
//! way a browser's reader mode shows it: the story, its headings, figures and
//! captions, and nothing of the menus, share bars, cookie notices,
//! advertisements and lists of other stories around it. This crate does that
//! on the owner's computer and hands back an EPUB.
//!
//! The work is four steps, each in a module of its own:
//!
//! 1. [`extract`] finds the article with a reader-mode extractor and reads
//!    the page's own description of it: title, byline, site, date, language
//!    and lead picture.
//! 2. [`body`] rebuilds the article from a short list of elements a book
//!    needs, leaves out the furniture a page set inside the story itself,
//!    and works out which copy of each picture to fetch.
//! 3. [`picture`] converts the fetched pictures for the panel.
//! 4. The pages are written with `kobo_doc::epub::write_publication`, with a
//!    title page in front that says where the article came from.
//!
//! Everything is bounded: the page, the number of pictures, the bytes and
//! time spent on each, and the book. A page that cannot be made into a book
//! is refused with a sentence that says why.
//!
//! This is a host crate. Nothing that runs on the reader depends on it.

pub mod address;
pub mod body;
pub mod book;
pub mod extract;
pub mod picture;
mod web;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use kobo_doc::epub::{Page, Publication, Resource};

pub use web::Web;

/// The reference panel, 1072 by 1448. Pictures are fitted to it and never
/// enlarged.
pub const PANEL: (u32, u32) = (1072, 1448);

/// The largest page accepted. A long article with all its markup is a few
/// hundred kilobytes; the largest real pages measured were under 3 MB.
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// What a fetch is for, which decides what the request says it accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Page,
    Picture,
}

/// One bounded request.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub address: &'a str,
    /// The page the picture appears on.
    pub referer: Option<&'a str>,
    pub kind: Kind,
    pub max_bytes: usize,
    pub timeout: Duration,
}

/// What a fetch answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Answer {
    pub body: Vec<u8>,
    /// Where the body was served from once redirects were followed, which
    /// is what a page's own relative addresses are resolved against.
    pub address: String,
}

/// Fetches what a page refers to. [`Web`] is the real one; tests answer from
/// a table.
pub trait Fetch {
    /// The answer, with the address it finally came from.
    ///
    /// # Errors
    ///
    /// A plain sentence saying what went wrong.
    fn fetch(&self, request: &Request<'_>) -> Result<Answer, String>;
}

/// A fetched page: its text, and the address it was served from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebPage {
    pub html: String,
    pub address: String,
}

/// How a clip is made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    /// Keep pictures in colour, for a colour panel.
    pub colour: bool,
    /// Fetch pictures at all.
    pub pictures: bool,
    pub panel: (u32, u32),
    /// The most pictures fetched from one page.
    pub max_pictures: usize,
    /// The most bytes downloaded for one picture.
    pub max_picture_bytes: usize,
    /// How long one picture may take.
    pub picture_timeout: Duration,
    /// How long all the pictures together may take. Pictures not reached by
    /// then are left out, so a slow image host cannot hold a clip forever.
    pub pictures_deadline: Duration,
    /// The largest book written.
    pub max_book_bytes: usize,
    /// The modified time written into the book. Left empty, the current
    /// time; tests set it so the same page gives the same bytes.
    pub modified: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            colour: false,
            pictures: true,
            panel: PANEL,
            max_pictures: 40,
            max_picture_bytes: 10 * 1024 * 1024,
            picture_timeout: Duration::from_secs(20),
            pictures_deadline: Duration::from_secs(90),
            max_book_bytes: 20 * 1024 * 1024,
            modified: None,
        }
    }
}

/// Why a page was not made into a book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The address is not an `http` or `https` address.
    BadAddress(String),
    /// The page could not be fetched.
    Unreachable(String),
    /// The address answered with something that is not a web page.
    NotAWebPage(&'static str),
    /// No block of readable text was found.
    NotAnArticle,
    /// The page has more elements than extraction will walk.
    TooComplex { elements: usize, limit: usize },
    /// The page itself is too large.
    PageTooLarge { bytes: usize, limit: usize },
    /// The text alone, without any pictures, is larger than a book may be.
    BookTooLarge { bytes: usize, limit: usize },
}

fn megabytes(bytes: usize) -> String {
    if bytes < 1024 * 1024 {
        return format!("{} KB", bytes.div_ceil(1024));
    }
    let tenths = bytes.saturating_mul(10) / (1024 * 1024);
    if tenths % 10 == 0 {
        format!("{} MB", tenths / 10)
    } else {
        format!("{}.{} MB", tenths / 10, tenths % 10)
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadAddress(address) if address.is_empty() => {
                out.write_str("The page needs a full web address starting with https://.")
            }
            Self::BadAddress(address) => write!(
                out,
                "{address} is not a web address. Give one starting with https://."
            ),
            Self::Unreachable(why) => write!(out, "The page could not be fetched: {why}."),
            Self::NotAWebPage(what) => {
                write!(out, "The address points to {what}, not a web page.")
            }
            Self::NotAnArticle => out.write_str(
                "No article was found on this page. It may be a home page, a list of links, \
                 or a page that only fills itself in with scripts.",
            ),
            Self::TooComplex { elements, limit } => write!(
                out,
                "The page has {elements} elements, more than the {limit} an article is read from."
            ),
            Self::PageTooLarge { bytes, limit } => write!(
                out,
                "The page is {}, and pages over {} are not clipped.",
                megabytes(*bytes),
                megabytes(*limit)
            ),
            Self::BookTooLarge { bytes, limit } => write!(
                out,
                "The article's text alone is {}, over the {} limit for a book.",
                megabytes(*bytes),
                megabytes(*limit)
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// A finished clip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Clipping {
    pub title: String,
    /// The site it came from, as the book's title page names it.
    pub site: Option<String>,
    /// A file name for the book, from its title.
    pub file_name: String,
    pub epub: Vec<u8>,
    /// Pictures in the book.
    pub pictures: usize,
    /// Plain sentences about anything left out, for the owner.
    pub notes: Vec<String>,
}

/// Turns a page's bytes into text, honouring the two encodings real pages
/// still use: UTF-8, and Windows-1252 written by older sites that call it
/// ISO-8859-1.
///
/// # Errors
///
/// [`Refusal::NotAWebPage`] for a PDF, a picture or other binary data, and
/// [`Refusal::PageTooLarge`] past [`MAX_PAGE_BYTES`].
pub fn page_text(bytes: &[u8]) -> Result<String, Refusal> {
    if bytes.len() > MAX_PAGE_BYTES {
        return Err(Refusal::PageTooLarge {
            bytes: bytes.len(),
            limit: MAX_PAGE_BYTES,
        });
    }
    let head = &bytes[..bytes.len().min(1024)];
    if head.starts_with(b"%PDF") {
        return Err(Refusal::NotAWebPage("a PDF"));
    }
    if head.starts_with(b"\x89PNG")
        || head.starts_with(b"\xff\xd8\xff")
        || head.starts_with(b"GIF8")
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP"))
    {
        return Err(Refusal::NotAWebPage("a picture"));
    }
    if head.starts_with(b"PK\x03\x04") {
        return Err(Refusal::NotAWebPage("an archive"));
    }
    if head.contains(&0) {
        return Err(Refusal::NotAWebPage("binary data"));
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_owned());
    }
    let declared = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    let single_byte = declared_charset(&declared).is_some_and(|charset| {
        matches!(
            charset.as_str(),
            "windows-1252"
                | "iso-8859-1"
                | "iso8859-1"
                | "latin1"
                | "latin-1"
                | "us-ascii"
                | "ascii"
                | "cp1252"
        )
    });
    if single_byte {
        return Ok(bytes.iter().map(|byte| windows_1252(*byte)).collect());
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// The first character set a page declares, lowercased, however the page
/// quoted it: `charset=x`, `charset="x"`, `charset='x'`, or inside a
/// `content` attribute.
fn declared_charset(head: &str) -> Option<String> {
    let lower = head.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("charset") {
        rest = &rest[at + "charset".len()..];
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start().trim_start_matches(['"', '\'']);
        let end = value
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '>' | '/'))
            .unwrap_or(value.len());
        if end > 0 {
            return Some(value[..end].to_owned());
        }
    }
    None
}

/// One Windows-1252 byte as a character.
fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}',
        '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}',
        '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}', '\u{17e}',
        '\u{178}',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        other => char::from(other),
    }
}

/// Fetches a page, answering where it was finally served from: a link that
/// redirects to the article names the article's pictures relative to the
/// article, not to the link.
///
/// # Errors
///
/// [`Refusal::BadAddress`] for an address that is not a web address,
/// [`Refusal::Unreachable`] when it cannot be fetched, and the refusals of
/// [`page_text`].
pub fn fetch_page(address: &str, fetch: &dyn Fetch) -> Result<WebPage, Refusal> {
    if !address::is_web_address(address) {
        return Err(Refusal::BadAddress(address.to_owned()));
    }
    let answer = fetch
        .fetch(&Request {
            address,
            referer: None,
            kind: Kind::Page,
            max_bytes: MAX_PAGE_BYTES + 1,
            timeout: Duration::from_secs(60),
        })
        .map_err(Refusal::Unreachable)?;
    Ok(WebPage {
        html: page_text(&answer.body)?,
        address: answer.address,
    })
}

/// Fetches the page at `address` and makes a book of its article, resolving
/// the page's addresses against where it was finally served from.
///
/// # Errors
///
/// The refusals of [`fetch_page`] and [`clip`].
pub fn clip_address(
    address: &str,
    fetch: &dyn Fetch,
    options: &Options,
) -> Result<Clipping, Refusal> {
    let page = fetch_page(address, fetch)?;
    clip(&page.html, &page.address, fetch, options)
}

/// Words in a picture address that mark a site's logo or a stand-in image,
/// which some pages offer as their preview picture.
const NOT_A_LEAD: [&str; 6] = [
    "logo",
    "favicon",
    "placeholder",
    "default-",
    "fallback",
    "icon",
];

/// "1 picture" or "3 pictures".
fn pictures_word(count: usize) -> String {
    if count == 1 {
        "1 picture".to_owned()
    } else {
        format!("{count} pictures")
    }
}

/// Each picture costs a manifest line and two archive headers as well as
/// its bytes.
const PER_PICTURE: usize = 512;

/// The article rebuilt for a book, with its lead figure put back.
fn rebuilt_body(
    article: &extract::Article,
    page: &str,
    title: &str,
) -> Result<body::Body, Refusal> {
    let content = dom_query::Document::from(article.content.as_str());
    let marked = content.select("#readability-page-1");
    let root = match marked.nodes().first() {
        Some(root) => *root,
        None => content
            .select("body")
            .nodes()
            .first()
            .copied()
            .ok_or(Refusal::NotAnArticle)?,
    };
    if let Some(figure) = &article.lead_figure {
        root.prepend_html(figure.as_str());
    }
    let body = body::rebuild(&root, page, title, article.byline.as_deref());
    if has_words(&body.nodes) {
        Ok(body)
    } else {
        Err(Refusal::NotAnArticle)
    }
}

/// The page's preview picture, for the title page of an article that kept
/// no picture of its own. Often it is the story's main photograph; a site's
/// logo offered in its place is recognised by name and passed over.
fn lead_picture(
    article: &extract::Article,
    page: &str,
    fetch: &dyn Fetch,
    options: &Options,
    allowance: &mut Allowance,
) -> Option<picture::Prepared> {
    let lead = article.lead_image.as_deref()?;
    let lower = lead.to_ascii_lowercase();
    if NOT_A_LEAD.iter().any(|word| lower.contains(word)) {
        return None;
    }
    let wanted = body::Picture {
        address: lead.to_owned(),
        alt: String::new(),
    };
    let mut quiet = Vec::new();
    fetch_pictures(
        std::slice::from_ref(&wanted),
        page,
        fetch,
        options,
        allowance,
        &mut quiet,
    )
    .pop()
    .flatten()
}

/// Lets pictures into `room` bytes in page order, the lead first, and
/// answers how many did not fit.
fn fit_pictures(
    lead: &mut Option<picture::Prepared>,
    prepared: &mut [Option<picture::Prepared>],
    mut room: usize,
) -> usize {
    let mut over = 0;
    for picture in std::iter::once(lead).chain(prepared.iter_mut()) {
        if let Some(ready) = picture {
            let cost = ready.bytes.len() + PER_PICTURE;
            if cost <= room {
                room -= cost;
            } else {
                *picture = None;
                over += 1;
            }
        }
    }
    over
}

/// The book's picture files: the lead, then the body's in page order, each
/// carried only while a page still shows it.
fn picture_files(
    lead: Option<(picture::Prepared, String)>,
    prepared: Vec<Option<picture::Prepared>>,
    names: &[Option<String>],
    article_xhtml: &str,
) -> Vec<Resource> {
    let mut resources = Vec::new();
    if let Some((lead, name)) = lead {
        resources.push(Resource {
            name,
            media_type: lead.media_type.to_owned(),
            bytes: lead.bytes,
            cover: true,
        });
    }
    for (picture, name) in prepared.into_iter().zip(names) {
        if let (Some(picture), Some(name)) = (picture, name) {
            if !article_xhtml.contains(&format!("src=\"{name}\"")) {
                continue;
            }
            let cover = resources.is_empty();
            resources.push(Resource {
                name: name.clone(),
                media_type: picture.media_type.to_owned(),
                bytes: picture.bytes,
                cover,
            });
        }
    }
    resources
}

/// The file name each prepared picture will have.
fn picture_names(prepared: &[Option<picture::Prepared>]) -> Vec<Option<String>> {
    prepared
        .iter()
        .enumerate()
        .map(|(index, picture)| {
            picture
                .as_ref()
                .map(|picture| format!("images/{}.{}", index + 1, picture.extension))
        })
        .collect()
}

/// Writes the book, refusing one that came out over the limit.
fn write_book(publication: &Publication, limit: usize) -> Result<Vec<u8>, Refusal> {
    let text: usize = publication.pages.iter().map(|page| page.body.len()).sum();
    let epub = kobo_doc::epub::write_publication(publication)
        .map_err(|_| Refusal::BookTooLarge { bytes: text, limit })?;
    if epub.len() > limit {
        return Err(Refusal::BookTooLarge {
            bytes: epub.len(),
            limit,
        });
    }
    Ok(epub)
}

fn modified_time(options: &Options) -> String {
    options.modified.clone().unwrap_or_else(|| {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        book::timestamp(now)
    })
}

fn check_page(html: &str, page: &str) -> Result<(), Refusal> {
    if !address::is_web_address(page) {
        return Err(Refusal::BadAddress(page.to_owned()));
    }
    if html.len() > MAX_PAGE_BYTES {
        return Err(Refusal::PageTooLarge {
            bytes: html.len(),
            limit: MAX_PAGE_BYTES,
        });
    }
    Ok(())
}

/// The body's pictures, then the lead picture when the body kept none, all
/// within one allowance.
fn all_pictures(
    article: &extract::Article,
    body: &body::Body,
    page: &str,
    fetch: &dyn Fetch,
    options: &Options,
    notes: &mut Vec<String>,
) -> (Vec<Option<picture::Prepared>>, Option<picture::Prepared>) {
    let mut allowance = Allowance::new(options);
    let prepared = fetch_pictures(&body.pictures, page, fetch, options, &mut allowance, notes);
    let lead = if prepared.iter().any(Option::is_some) {
        None
    } else {
        lead_picture(article, page, fetch, options, &mut allowance)
    };
    (prepared, lead)
}

/// Makes a book of the article in `html`, which came from `page`.
///
/// # Errors
///
/// A [`Refusal`] saying why the page could not be made into a book.
pub fn clip(
    html: &str,
    page: &str,
    fetch: &dyn Fetch,
    options: &Options,
) -> Result<Clipping, Refusal> {
    check_page(html, page)?;
    let article = extract::extract(html, page)?;
    let title = if article.title.is_empty() {
        address::host_of(page).unwrap_or_else(|| "Article".to_owned())
    } else {
        article.title.clone()
    };
    let mut body = rebuilt_body(&article, page, &title)?;

    let mut notes = Vec::new();
    let (mut prepared, mut lead) = all_pictures(&article, &body, page, fetch, options, &mut notes);

    let language = article.language.clone().unwrap_or_else(|| "en".to_owned());
    let site = article.site.clone().or_else(|| address::host_of(page));
    // What the book says it was saved from, and what it is known by: the
    // same article reached under another name is the same book.
    let source = article.canonical.as_deref().unwrap_or(page);
    let date = article
        .published
        .as_deref()
        .map(|date| book::spoken_date(date, &language));
    let title_page = |lead: Option<&str>| {
        book::title_page(&book::TitlePage {
            title: &title,
            site: site.as_deref(),
            byline: article.byline.as_deref(),
            date: date.as_deref(),
            source,
            lead,
        })
    };

    // Text first: it is the book. Pictures then join in page order for as
    // long as they fit.
    let lead_name = lead
        .as_ref()
        .map(|lead| format!("images/lead.{}", lead.extension));
    let all_names = picture_names(&prepared);
    let mut text = title_page(lead_name.as_deref());
    body::render_body(&body, &all_names, &mut text);
    // The package, navigation, style sheet and archive headers.
    let text_bytes = text.len() + book::STYLESHEET.len() + 16 * 1024;
    if text_bytes > options.max_book_bytes {
        return Err(Refusal::BookTooLarge {
            bytes: text_bytes,
            limit: options.max_book_bytes,
        });
    }
    let over_budget = fit_pictures(
        &mut lead,
        &mut prepared,
        options.max_book_bytes - text_bytes,
    );
    if over_budget > 0 {
        notes.push(format!(
            "Left out {} to keep the book under {}.",
            pictures_word(over_budget),
            megabytes(options.max_book_bytes)
        ));
    }

    let names: Vec<Option<String>> = prepared
        .iter()
        .zip(all_names)
        .map(|(picture, name)| picture.as_ref().and(name))
        .collect();
    body::settle(&mut body.nodes, &names);
    let mut article_xhtml = String::new();
    body::render_body(&body, &names, &mut article_xhtml);
    let lead = lead.zip(lead_name);
    let title_xhtml = title_page(lead.as_ref().map(|(_, name)| name.as_str()));
    let resources = picture_files(lead, prepared, &names, &article_xhtml);
    let pictures = resources.len();

    let epub = write_book(
        &Publication {
            title: title.clone(),
            author: article.byline.clone(),
            language,
            publisher: site.clone(),
            date: article.published.clone(),
            source: Some(source.to_owned()),
            modified: modified_time(options),
            stylesheet: Some(book::STYLESHEET.to_owned()),
            pages: vec![
                Page {
                    title: "Title page".to_owned(),
                    body: title_xhtml,
                },
                Page {
                    title: title.clone(),
                    body: article_xhtml,
                },
            ],
            resources,
        },
        options.max_book_bytes,
    )?;
    Ok(Clipping {
        file_name: book::file_name(&title),
        site,
        title,
        epub,
        pictures,
        notes,
    })
}

fn has_words(nodes: &[body::Node]) -> bool {
    nodes.iter().any(|node| match node {
        body::Node::Text(text) => !text.trim().is_empty(),
        body::Node::Element(element) => has_words(&element.children),
        body::Node::Picture(_) => false,
    })
}

/// Fetches and prepares pictures in page order, within the count, size and
/// time allowed. Each answer is the prepared picture, or nothing when it was
/// not used.
/// What is left of one clip's picture budget: how many more may be fetched
/// and when fetching must stop. Shared by every pass, so the lead picture
/// tried after the body's cannot be a forty-first, or start after the time
/// has run out.
struct Allowance {
    pictures: usize,
    until: Instant,
}

impl Allowance {
    fn new(options: &Options) -> Self {
        Self {
            pictures: options.max_pictures,
            until: Instant::now() + options.pictures_deadline,
        }
    }
}

fn fetch_pictures(
    pictures: &[body::Picture],
    page: &str,
    fetch: &dyn Fetch,
    options: &Options,
    allowance: &mut Allowance,
    notes: &mut Vec<String>,
) -> Vec<Option<picture::Prepared>> {
    if !options.pictures {
        return vec![None; pictures.len()];
    }
    let mut failed = 0;
    let mut beyond_count = 0;
    let mut beyond_time = 0;
    let mut out = Vec::with_capacity(pictures.len());
    for wanted in pictures {
        if allowance.pictures == 0 {
            beyond_count += 1;
            out.push(None);
            continue;
        }
        let left = allowance.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            beyond_time += 1;
            out.push(None);
            continue;
        }
        allowance.pictures -= 1;
        let answer = fetch.fetch(&Request {
            address: &wanted.address,
            referer: Some(page),
            kind: Kind::Picture,
            max_bytes: options.max_picture_bytes,
            timeout: options.picture_timeout.min(left),
        });
        let ready = match answer
            .map(|answer| picture::prepare(&answer.body, options.panel, options.colour))
        {
            Ok(Ok(ready)) => Some(ready),
            // An icon or a tracking pixel is meant to be left out, and is not
            // worth a note.
            Ok(Err(picture::Unusable::TooSmall)) => None,
            Ok(Err(_)) | Err(_) => {
                failed += 1;
                None
            }
        };
        out.push(ready);
    }
    if failed > 0 {
        notes.push(format!(
            "Left out {} that could not be fetched or used.",
            pictures_word(failed)
        ));
    }
    if beyond_count > 0 {
        notes.push(format!(
            "Left out {} past the first {}.",
            pictures_word(beyond_count),
            options.max_pictures
        ));
    }
    if beyond_time > 0 {
        notes.push(format!(
            "Left out {} because fetching pictures took too long.",
            pictures_word(beyond_time)
        ));
    }
    out
}
