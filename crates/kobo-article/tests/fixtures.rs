//! Real and imitation web pages made into books, and checked for what a
//! reader should and should not find in them.
//!
//! The real pages say whether extraction works on pages nobody here wrote.
//! The imitation ones carry every kind of furniture at once, so that each
//! kind has a test that fails when it comes back. Furniture is checked to be
//! in the page before it is checked to be gone from the book, so a test can
//! never pass because the words were not there to begin with.
//!
//! Pictures are answered here rather than fetched, so the tests run offline.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use kobo_article::{clip, Answer, Clipping, Fetch, Kind, Options, Request};
use kobo_doc::zip::Archive;

/// Answers every picture request with a picture, except addresses saying
/// `missing`, and records what was asked for. A page, when one is set, is
/// answered as served from its own address, as after a redirect.
#[derive(Default)]
struct Pictures {
    asked: RefCell<Vec<String>>,
    page: Option<(String, String)>,
}

fn encoded(image: &image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format).expect("encodes");
    out.into_inner()
}

fn photograph() -> Vec<u8> {
    let image = image::RgbImage::from_fn(320, 240, |x, y| {
        let noise = (x.wrapping_mul(7919) ^ y.wrapping_mul(104_729)).wrapping_mul(2_654_435_761);
        image::Rgb([
            u8::try_from(noise & 0xff).unwrap_or(0),
            u8::try_from((noise >> 8) & 0xff).unwrap_or(0),
            u8::try_from((noise >> 16) & 0xff).unwrap_or(0),
        ])
    });
    encoded(&image.into(), image::ImageFormat::Jpeg)
}

fn diagram() -> Vec<u8> {
    let image = image::GrayImage::from_fn(300, 200, |x, y| {
        image::Luma([if (x / 20 + y / 20) % 2 == 0 { 0 } else { 255 }])
    });
    encoded(&image.into(), image::ImageFormat::Png)
}

impl Fetch for Pictures {
    fn fetch(&self, request: &Request<'_>) -> Result<Answer, String> {
        if let (Kind::Page, Some((html, served_from))) = (request.kind, &self.page) {
            return Ok(Answer {
                body: html.clone().into_bytes(),
                address: served_from.clone(),
            });
        }
        self.picture(request).map(|body| Answer {
            body,
            address: request.address.to_owned(),
        })
    }
}

impl Pictures {
    fn picture(&self, request: &Request<'_>) -> Result<Vec<u8>, String> {
        assert_eq!(request.kind, Kind::Picture, "only pictures are fetched");
        self.asked.borrow_mut().push(request.address.to_owned());
        if request.address.contains("missing") {
            return Err("the site answered with an error".to_owned());
        }
        if request.address.contains("pixel") || request.address.contains("beacon") {
            return Ok(encoded(
                &image::RgbImage::new(1, 1).into(),
                image::ImageFormat::Gif,
            ));
        }
        if request.address.contains("diagram") {
            return Ok(diagram());
        }
        Ok(photograph())
    }
}

/// A book, opened up for checking.
struct Book {
    clipping: Clipping,
    title_page: String,
    body: String,
    package: String,
    members: BTreeMap<String, Vec<u8>>,
    asked: Vec<String>,
}

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn options() -> Options {
    Options {
        modified: Some("2026-11-08T12:00:00Z".to_owned()),
        ..Options::default()
    }
}

fn book_of(name: &str, page: &str) -> Book {
    book_with(name, page, &options())
}

fn book_with(name: &str, page: &str, options: &Options) -> Book {
    let pictures = Pictures::default();
    let clipping = clip(&fixture(name), page, &pictures, options)
        .unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
    // Member names come back lowercased, as the reader looks them up.
    let archive = Archive::open(&clipping.epub).expect("the book is a zip");
    let members: BTreeMap<String, Vec<u8>> = archive
        .names()
        .map(|member| {
            (
                member.to_owned(),
                archive.read(member).expect("every member reads"),
            )
        })
        .collect();
    let text = |member: &str| {
        String::from_utf8(members.get(member).cloned().unwrap_or_default()).expect("UTF-8")
    };
    let book = Book {
        title_page: text("oebps/p1.xhtml"),
        body: text("oebps/p2.xhtml"),
        package: text("oebps/content.opf"),
        members,
        asked: pictures.asked.take(),
        clipping,
    };
    check_book(&book);
    book
}

/// What every book must be, whatever page it came from.
fn check_book(book: &Book) {
    for (member, bytes) in &book.members {
        let is_xml = Path::new(member).extension().is_some_and(|extension| {
            matches!(extension.to_str(), Some("xhtml" | "opf" | "ncx" | "xml"))
        });
        if is_xml {
            let text = std::str::from_utf8(bytes).expect("XML is UTF-8");
            assert_well_formed(member, text);
        }
    }
    // Every picture a page shows is in the book, and the book carries no
    // picture that nothing shows.
    let pages = format!("{}{}", book.title_page, book.body);
    let shown: Vec<&str> = pages
        .split("<img src=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    for name in &shown {
        assert!(
            book.members.contains_key(&format!("oebps/{name}")),
            "{name} is shown but not in the book"
        );
    }
    let carried = book
        .members
        .keys()
        .filter(|member| member.starts_with("oebps/images/"))
        .count();
    assert_eq!(carried, shown.len(), "pictures carried and shown differ");
    assert_eq!(book.clipping.pictures, shown.len());
    for (member, bytes) in &book.members {
        if member.starts_with("oebps/images/") {
            let picture = image::load_from_memory(bytes).expect("every picture decodes");
            assert!(
                picture.width() <= 1072 && picture.height() <= 1448,
                "{member} fits the panel"
            );
            assert!(
                matches!(
                    image::guess_format(bytes),
                    Ok(image::ImageFormat::Jpeg | image::ImageFormat::Png)
                ),
                "{member} is a format the reader decodes"
            );
        }
    }
    // The book reads back in the reader's own parser.
    let document = kobo_doc::epub::parse(&book.clipping.epub).expect("the reader opens it");
    assert_eq!(
        document.title.as_deref(),
        Some(book.clipping.title.as_str())
    );
    assert!(book.package.contains("version=\"3.0\""));
    assert!(book.package.contains("dcterms:modified"));
}

/// A strict enough check that a document is well-formed XML: every element
/// closed in order, and no entity XML does not define.
fn assert_well_formed(member: &str, text: &str) {
    let mut open: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(['<', '&']) {
        let (before, from) = rest.split_at(start);
        assert!(!before.contains('>'), "{member}: a stray > in text");
        if from.starts_with('&') {
            let end = from
                .find(';')
                .unwrap_or_else(|| panic!("{member}: an unended entity"));
            let entity = &from[1..end];
            assert!(
                matches!(entity, "amp" | "lt" | "gt" | "quot" | "apos") || entity.starts_with('#'),
                "{member}: &{entity}; is not an XML entity"
            );
            rest = &from[end + 1..];
            continue;
        }
        let end = from
            .find('>')
            .unwrap_or_else(|| panic!("{member}: an unended tag"));
        let tag = &from[1..end];
        rest = &from[end + 1..];
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            let expected = open.pop().unwrap_or_default();
            assert_eq!(
                name.trim(),
                expected,
                "{member}: </{name}> closes <{expected}>"
            );
        } else if !tag.ends_with('/') {
            let name = tag.split_whitespace().next().unwrap_or_default();
            open.push(name.to_owned());
        }
    }
    assert!(open.is_empty(), "{member}: {open:?} never closed");
}

/// Asserts the words are in the page and not in the book.
fn assert_gone(page: &str, book: &Book, furniture: &[&str]) {
    let source = fixture(page);
    let all = format!("{}{}", book.title_page, book.body);
    for words in furniture {
        assert!(source.contains(words), "{page} never said {words:?}");
        assert!(!all.contains(words), "{words:?} came through into the book");
    }
}

fn assert_kept(book: &Book, wanted: &[&str]) {
    for words in wanted {
        assert!(
            book.body.contains(words),
            "{words:?} is missing from the book"
        );
    }
}

fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

const NEWS: &str = "https://news.example.org/local/2026/03/harbour-wall";

#[test]
fn a_news_page_keeps_its_story_and_loses_every_piece_of_furniture() {
    let book = book_of("synthetic-news.html", NEWS);
    assert_gone(
        "synthetic-news.html",
        &book,
        &[
            "We use cookies",
            "Accept all",
            "Advertisement",
            "Subscribe from 1 pound a week",
            "Search the Gazette",
            "Share on X",
            "Share this story",
            "Copy link",
            "Read more:",
            "morning briefing",
            "Sign up",
            "Related stories",
            "Lifeboat crew honoured",
            "Comments (3)",
            "harbourwatcher",
            "Post comment",
            "Most read",
            "New ferry timetable",
            "Holiday cottages",
            "Privacy policy",
            "All rights reserved",
            ">Local<",
            ">Weather<",
        ],
    );
    assert_kept(
        &book,
        &[
            "The east wall of the harbour will be rebuilt",
            "<h2>What the work involves</h2>",
            "<li>Phase one: temporary sheet piling",
            "<blockquote><p>The fleet cannot move",
            "<caption>Funding for the east wall rebuild</caption>",
            "<th>Source</th>",
            "<td>8.0 million pounds</td>",
            "residents can comment on the design",
        ],
    );
}

#[test]
fn a_news_page_names_its_site_author_and_date_on_the_title_page() {
    let book = book_of("synthetic-news.html", NEWS);
    assert!(book
        .title_page
        .contains("<p class=\"site\">The Harbour Gazette</p>"));
    assert!(book
        .title_page
        .contains("<h1>Harbour wall to be rebuilt after winter storms</h1>"));
    assert!(book
        .title_page
        .contains("<p class=\"byline\">Mara Lindqvist</p>"));
    assert!(book
        .title_page
        .contains("<p class=\"date\">14 March 2026</p>"));
    assert!(book.title_page.contains(&format!("href=\"{NEWS}\"")));
    assert!(book.package.contains("<dc:language>en-GB</dc:language>"));
    assert!(book.package.contains("<dc:date>2026-03-14</dc:date>"));
    assert!(book
        .package
        .contains("<dc:creator>Mara Lindqvist</dc:creator>"));
    assert_eq!(
        book.clipping.file_name,
        "Harbour wall to be rebuilt after winter storms.epub"
    );
    // The headline is on the title page and nowhere in the body's text.
    let text = book.body.split("<body>").nth(1).unwrap_or_default();
    assert!(!text.contains("Harbour wall to be rebuilt after winter storms"));
}

#[test]
fn a_news_page_figures_keep_their_captions_and_take_the_best_copy_of_each_picture() {
    let book = book_of("synthetic-news.html", NEWS);
    assert_eq!(count(&book.body, "<figure>"), 2, "{}", book.body);
    assert_kept(
        &book,
        &[
            "<figcaption>Waves broke over the east wall during the February storms. Photograph: Ines Varga</figcaption>",
            "<figcaption>The three phases of the rebuild, from the council's planning papers.</figcaption>",
            "alt=\"Waves breaking over the east harbour wall\"",
        ],
    );
    // The WebP at panel width, not the AVIF the reader cannot decode and not
    // the small fallback; the lazy diagram's real address, not its
    // placeholder; no tracking pixel and no author photo.
    assert_eq!(
        book.asked,
        [
            "https://cdn.example.org/img/harbour-wall-1600.webp",
            "https://news.example.org/images/2026/harbour-plan-diagram.png",
        ]
    );
    // A diagram stays sharp; a photograph is a JPEG.
    assert!(book.members.contains_key("oebps/images/1.jpg"));
    assert!(book.members.contains_key("oebps/images/2.png"));
    assert!(book.clipping.notes.is_empty(), "{:?}", book.clipping.notes);
}

#[test]
fn a_picture_that_cannot_be_fetched_leaves_neither_a_broken_reference_nor_its_caption() {
    let html = fixture("synthetic-news.html").replace("harbour-plan-diagram", "missing-plan");
    let pictures = Pictures::default();
    let clipping = clip(&html, NEWS, &pictures, &options()).expect("a book");
    let archive = Archive::open(&clipping.epub).expect("a zip");
    let body = String::from_utf8(archive.read("oebps/p2.xhtml").expect("a body")).expect("UTF-8");
    assert!(!body.contains("The three phases of the rebuild"), "{body}");
    assert!(body.contains("Waves broke over the east wall"));
    assert_eq!(clipping.pictures, 1);
    assert_eq!(
        clipping.notes,
        ["Left out 1 picture that could not be fetched or used."]
    );
}

#[test]
fn without_pictures_the_figures_go_and_the_text_stays() {
    let book = book_with(
        "synthetic-news.html",
        NEWS,
        &Options {
            pictures: false,
            ..options()
        },
    );
    assert!(book.asked.is_empty());
    assert!(!book.body.contains("<figure>"));
    assert!(!book.body.contains("<figcaption>"));
    assert_kept(&book, &["The east wall of the harbour will be rebuilt"]);
}

#[test]
fn a_book_over_its_size_limit_leaves_pictures_out_and_says_so() {
    let book = book_with(
        "synthetic-news.html",
        NEWS,
        &Options {
            max_book_bytes: 30 * 1024,
            ..options()
        },
    );
    assert!(book.clipping.epub.len() <= 30 * 1024);
    assert!(book.clipping.pictures < 2);
    assert!(
        book.clipping
            .notes
            .iter()
            .any(|note| note.starts_with("Left out")
                && note.ends_with("to keep the book under 30 KB.")),
        "{:?}",
        book.clipping.notes
    );
    assert_kept(&book, &["The east wall of the harbour will be rebuilt"]);
}

#[test]
fn the_lead_picture_counts_against_the_same_picture_budget_as_the_body() {
    // Both body pictures fail, so the page's preview picture is tried next,
    // but only while the clip's count and time allow it.
    let html = fixture("synthetic-magazine.html")
        .replace("lighthouse/tower-", "lighthouse/missing-tower-")
        .replace("lighthouse/logbook-", "lighthouse/missing-logbook-");
    let page = "https://www.longform.example.com/features/last-lighthouse-keepers";
    let asked = |options: &Options| {
        let pictures = Pictures::default();
        clip(&html, page, &pictures, options).expect("a book");
        pictures.asked.take()
    };
    let lead = "https://static.longform.example.com/2026/lighthouse/hero-1600.jpg";
    assert_eq!(asked(&options()).last().map(String::as_str), Some(lead));
    assert_eq!(asked(&options()).len(), 3);
    let two = Options {
        max_pictures: 2,
        ..options()
    };
    assert_eq!(asked(&two).len(), 2, "the lead would be a third picture");
    let no_time = Options {
        pictures_deadline: std::time::Duration::ZERO,
        ..options()
    };
    assert!(asked(&no_time).is_empty());
}

#[test]
fn a_fetched_page_that_redirected_resolves_its_pictures_against_where_it_landed() {
    let pictures = Pictures {
        page: Some((
            fixture("synthetic-blog.html").replace(
                "/images/logger-wiring-diagram.png",
                "logger-wiring-diagram.png",
            ),
            "https://slipway.example.net/posts/tide-logger/".to_owned(),
        )),
        ..Pictures::default()
    };
    let clipping =
        kobo_article::clip_address("https://slipway.example.net/go", &pictures, &options())
            .expect("a book");
    assert_eq!(clipping.pictures, 1);
    // The diagram is named relative to the page, so it is found beside the
    // page the link landed on, not beside the link.
    assert_eq!(
        pictures.asked.take(),
        ["https://slipway.example.net/posts/tide-logger/logger-wiring-diagram.png"]
    );
    let archive = Archive::open(&clipping.epub).expect("a zip");
    let title = String::from_utf8(archive.read("oebps/p1.xhtml").expect("a page")).expect("UTF-8");
    assert!(
        title.contains("Saved from <a href=\"https://slipway.example.net/posts/tide-logger/\">")
    );
}

#[test]
fn a_page_whose_text_alone_is_too_large_is_refused_with_its_size() {
    let refusal = clip(
        &fixture("synthetic-news.html"),
        NEWS,
        &Pictures::default(),
        &Options {
            max_book_bytes: 4 * 1024,
            ..options()
        },
    )
    .expect_err("a 4 KB book cannot hold the article");
    assert!(
        refusal
            .to_string()
            .starts_with("The article's text alone is"),
        "{refusal}"
    );
}

#[test]
fn a_blog_post_keeps_its_code_table_and_headings_without_sidebar_or_comments() {
    let page = "https://slipway.example.net/posts/tide-logger/";
    let book = book_of("synthetic-blog.html", page);
    assert_gone(
        "synthetic-blog.html",
        &book,
        &[
            "Skip to content",
            ">Projects<",
            "Share this post",
            "Post it",
            "Tagged:",
            "Previous: Fixing the boat trailer lights",
            "12 comments",
            "Lovely build",
            "Leave a comment",
            "I mend boats",
            "September 2025</a>",
            "Get new posts by email",
            "Powered by a static site generator",
            "6 min read",
        ],
    );
    assert_kept(
        &book,
        &[
            "<h2>How it works</h2>",
            "<h3>Parts</h3>",
            "<h2>The code</h2>",
            "<pre>// Ten readings, keep the middle one.\nfn median(mut readings: [u32; 10]) -&gt; u32 {\n    readings.sort_unstable();",
            "<code>(water - air) / 98</code>",
            "<th>Difference</th>",
            "<td>+11 cm</td>",
            "<blockquote><p>The table is right about everything",
            "<li>A sealed pressure sensor rated to two bar.</li>",
            "<figcaption>Figure 1. The sensor, the logger and the battery, wired on a scrap of stripboard.</figcaption>",
        ],
    );
    // The permalink markers beside each heading are not words of it.
    assert!(!book.body.contains(">#<") && !book.body.contains("#How it works"));
    assert!(book
        .title_page
        .contains("<p class=\"byline\">Sam Okafor</p>"));
    assert!(book
        .title_page
        .contains("<p class=\"date\">21 September 2025</p>"));
    // The inline drawing cannot be shown; the diagram beside it is.
    assert_eq!(
        book.asked,
        ["https://slipway.example.net/images/logger-wiring-diagram.png"]
    );
}

#[test]
fn a_magazine_feature_loses_its_paywall_prompt_pull_quote_and_recommendations() {
    let page = "https://www.longform.example.com/features/last-lighthouse-keepers";
    let book = book_of("synthetic-magazine.html", page);
    assert_gone(
        "synthetic-magazine.html",
        &book,
        &[
            "free articles left",
            "uses cookies",
            "Listen to this article",
            "The machine keeps the light. I keep the lighthouse.",
            "Enjoying this story?",
            "Recommended for you",
            "The islands that time forgot",
            "Related:",
            "Tweet",
            "writes about the coast",
            "Up next",
            "The ferryman",
            "Sea kayaking holidays",
            "June 2, 2026",
            ">Features<",
            ">Shop<",
        ],
    );
    assert_kept(
        &book,
        &[
            "Every Tuesday morning, whatever the weather, Elspeth Gray",
            "<h2>A log that was never closed</h2>",
            "<em>Light handed over</em>",
            "<li>Sign the log.</li>",
            "<figcaption>The tower at Skerry Point, last staffed in 1989.</figcaption>",
            "<figcaption>The watch room logbook, open at the week the light was automated.</figcaption>",
            "went back down the hundred and fourteen steps",
        ],
    );
    assert!(book
        .title_page
        .contains("<p class=\"byline\">Ruth Adeyemi</p>"));
    assert!(book
        .title_page
        .contains("<p class=\"date\">2 June 2026</p>"));
    // The smallest copy at least as wide as the panel, from all the sizes
    // the picture element and its fallback offer, and the logbook from
    // behind its lazy placeholder. The hidden beacon is never asked for.
    assert_eq!(
        book.asked,
        [
            "https://static.longform.example.com/2026/lighthouse/tower-1280.jpg",
            "https://static.longform.example.com/2026/lighthouse/logbook-1400.jpg",
        ]
    );
}

#[test]
fn a_wikipedia_article_keeps_its_sections_and_figures_without_the_wiki_around_it() {
    let page = "https://en.wikipedia.org/wiki/Common_kingfisher";
    let book = book_of("wikipedia-common-kingfisher.html", page);
    assert_gone(
        "wikipedia-common-kingfisher.html",
        &book,
        &[
            "Jump to content",
            "Main menu",
            "Create account",
            "Toggle the table of contents",
            "Donate",
            "What links here",
            "Cite this page",
        ],
    );
    assert_kept(
        &book,
        &[
            "<h2>Taxonomy</h2>",
            "<h2>Description</h2>",
            "<h3>Geographical variation</h3>",
            "<h2>Behaviour</h2>",
            "<h3>Breeding</h3>",
            "The common kingfisher was first described by",
            "<figcaption>Common kingfisher in flight over",
        ],
    );
    assert!(!book.body.contains(">edit<"), "edit links came through");
    assert!(book.clipping.pictures >= 5, "{}", book.clipping.pictures);
    // The tiny edit pencil next to the classification is an icon.
    assert!(book
        .asked
        .iter()
        .all(|address| !address.contains("OOjs_UI_icon")));
    assert!(book.title_page.contains("<h1>Common kingfisher</h1>"));
}

#[test]
fn a_nasa_story_keeps_the_photograph_set_above_its_headline() {
    let page =
        "https://science.nasa.gov/missions/hubble/hubble-learns-galactic-history-from-stars/";
    let book = book_of("nasa-hubble-m30.html", page);
    assert_gone(
        "nasa-hubble-m30.html",
        &book,
        &[
            "Discover More Topics",
            "Keep Exploring",
            "Facebook",
            "Instagram",
        ],
    );
    assert_kept(
        &book,
        &[
            "This NASA/ESA Hubble Space Telescope image provides a lesson in galactic history.",
            "M30 is a <a href=\"https://science.nasa.gov/mission/hubble/science/universe-uncovered/hubble-star-clusters/\">globular cluster</a>",
            "Text credit: ESA/Hubble",
        ],
    );
    assert!(book
        .title_page
        .contains("<p class=\"site\">NASA Science</p>"));
    assert!(book
        .title_page
        .contains("<p class=\"byline\">NASA Hubble Mission Team</p>"));
    assert!(book
        .title_page
        .contains("<p class=\"date\">5 November 2026</p>"));
    // The photograph sits in the page's header, outside the block of text
    // extraction keeps; it is put back above the story, once.
    let story = book.body.split("<body>").nth(1).unwrap_or_default();
    assert!(
        story
            .trim_start()
            .starts_with("<figure><img src=\"images/1.jpg\""),
        "{story}"
    );
    assert_eq!(count(&book.body, "<img"), 1);
    assert!(!book.title_page.contains("<img"));
    assert!(book.package.contains("properties=\"cover-image\""));
}

#[test]
fn a_government_release_keeps_its_figures_and_loses_the_sign_up_and_feedback_boxes() {
    let page = "https://www.nist.gov/news-events/news/2026/10/new-chip-based-tape-measure-light-could-revolutionize-mobile-timekeeping";
    let book = book_of("nist-microcomb.html", page);
    assert_gone(
        "nist-microcomb.html",
        &book,
        &[
            "NIST in your inbox",
            "Enter Email Address",
            "Was this page helpful",
            "Webmaster",
        ],
    );
    assert_kept(
        &book,
        &[
            "Researchers at the National Institute of Standards and Technology (NIST)",
            "Close-up of a single chip that houses up to 300 tiny ring-shaped resonators",
        ],
    );
    assert_eq!(count(&book.body, "<figure>"), 2, "{}", book.body);
    assert_eq!(count(&book.body, "<figcaption>"), 2);
    // The site logo the page offers as its preview is not used: the
    // article has photographs of its own.
    assert!(!book.title_page.contains("<img"));
}

#[test]
fn a_book_chapter_keeps_the_story_and_none_of_the_distributor_boilerplate() {
    let page = "https://www.gutenberg.org/cache/epub/1952/pg1952-images.html";
    let book = book_of("gutenberg-yellow-wallpaper.html", page);
    assert_gone(
        "gutenberg-yellow-wallpaper.html",
        &book,
        &[
            "START OF THE PROJECT GUTENBERG",
            "Project Gutenberg License",
            "Updated editions will replace",
            "Release date",
        ],
    );
    assert_kept(
        &book,
        &[
            "It is very seldom that mere ordinary people like John and myself secure ancestral halls",
            "<em>perhaps</em>",
            "Now why should that man have fainted?",
        ],
    );
    assert!(book.title_page.contains("<h1>The Yellow Wallpaper</h1>"));
}

#[test]
fn the_same_page_makes_the_same_book() {
    let first = book_of(
        "synthetic-blog.html",
        "https://slipway.example.net/posts/tide-logger/",
    );
    let second = book_of(
        "synthetic-blog.html",
        "https://slipway.example.net/posts/tide-logger/",
    );
    assert_eq!(first.clipping.epub, second.clipping.epub);
}

#[test]
fn a_page_that_is_not_an_article_is_refused_in_a_sentence() {
    let refusal = clip(
        "<html><head><title>Sign in</title></head><body><nav><a href=\"/\">Home</a></nav><form><input name=\"user\"><button>Sign in</button></form></body></html>",
        "https://example.org/login",
        &Pictures::default(),
        &options(),
    )
    .expect_err("a sign-in form is not an article");
    assert!(refusal
        .to_string()
        .starts_with("No article was found on this page."));
}

#[test]
fn a_saved_page_with_an_address_that_is_not_a_web_address_is_refused() {
    let refusal = clip(
        "<p>x</p>",
        "file:///tmp/page.html",
        &Pictures::default(),
        &options(),
    )
    .expect_err("not a web address");
    assert_eq!(
        refusal.to_string(),
        "file:///tmp/page.html is not a web address. Give one starting with https://."
    );
}

#[test]
fn a_pdf_or_a_picture_given_as_a_page_is_named_for_what_it_is() {
    assert_eq!(
        kobo_article::page_text(b"%PDF-1.7 ...").map_err(|refusal| refusal.to_string()),
        Err("The address points to a PDF, not a web page.".to_owned())
    );
    assert_eq!(
        kobo_article::page_text(&photograph()).map_err(|refusal| refusal.to_string()),
        Err("The address points to a picture, not a web page.".to_owned())
    );
    for declaration in [
        "<meta charset=windows-1252>",
        "<meta charset=\"windows-1252\">",
        "<meta charset='windows-1252'>",
        "<meta charset = 'ISO-8859-1' />",
        "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\">",
        "<meta http-equiv='Content-Type' content='text/html;charset=latin1'>",
    ] {
        let mut page = declaration.as_bytes().to_vec();
        page.extend_from_slice(b"<p>caf\xe9</p>");
        assert_eq!(
            kobo_article::page_text(&page).as_deref(),
            Ok(format!("{declaration}<p>caf\u{e9}</p>").as_str()),
            "{declaration}"
        );
    }
}
