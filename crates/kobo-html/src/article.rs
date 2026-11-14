//! Finding the article inside a web page.
//!
//! A page fetched from a link on Hacker News is a web page before it is an
//! article. Ahead of the words sit a menu, a cookie notice, a search form and
//! a row of share buttons; behind them sit related stories, a newsletter
//! sign-up and a footer of legal links. Read wholesale, a typical news page
//! came out as seven pages of furniture in thirty-two before the first
//! sentence the owner had come for.
//!
//! Browsers solve this with a reader mode that scores every block on the page.
//! This is the small version of that idea, chosen because the markup already
//! says most of it: a page written in the last decade wraps its story in
//! `<article>` or `<main>`, and the furniture in `<nav>`, `<aside>`, `<footer>`
//! and `<form>`. So the cut is the marked region that holds the most
//! paragraphs, with the furniture inside it taken out. A page that marks
//! nothing is kept whole, with its furniture taken out, because furniture is
//! worse than the article but nothing is worse than both.

use crate::{attribute, element_name, skip_element};

/// Elements that are never the article, wherever they sit.
const FURNITURE: [&str; 10] = [
    "nav", "aside", "footer", "form", "button", "noscript", "dialog", "select", "template",
    "iframe",
];

/// Roles a page gives its furniture when it uses plain `div`s for everything.
///
/// A link dressed as a button is still a button: "New issue" or "Sign up" is
/// something to press in a browser, and in a reader it is a stray line.
const FURNITURE_ROLES: [&str; 11] = [
    "navigation",
    "complementary",
    "contentinfo",
    "banner",
    "search",
    "dialog",
    "button",
    "menu",
    "menubar",
    "toolbar",
    "tablist",
];

/// Words in a class or id that only ever name furniture.
///
/// Deliberately few. A page's class names are its own business and most say
/// nothing about meaning, but these three are spelled the same way on almost
/// every site that has them, and each one, left in, put a line of links or a
/// sign-up form at the top of the first page. "cookie" is not among them,
/// because a recipe is entitled to a class called `cookie-recipe`.
///
/// A "sticky header" is the title and status a page pins to the top of the
/// window as it scrolls, so it is the article's first lines said a second
/// time.
const FURNITURE_NAMES: [&str; 4] = ["breadcrumb", "newsletter", "skip-link", "sticky-header"];

/// Words in a class that hide an element from the eye and leave it for a
/// screen reader.
///
/// Pages label their regions this way for listeners ("Repository
/// navigation", "Reacted by" and forty names), and a reader that shows them
/// prints the labels of things that are no longer there. Compared with the
/// hyphens taken out, because each design system spells them its own way.
const UNSEEN_NAMES: [&str; 3] = ["sronly", "visuallyhidden", "screenreader"];

/// The fewest links a list needs before it is read as a menu.
const MENU_LINKS: usize = 3;

/// The most words a link in a menu carries.
///
/// "Archive", "Today's picture" and "About us" are menu entries. A list whose
/// links read like titles of things, "How the tide tables were first
/// printed", is a reading list, and that is the article's.
const MENU_LINK_WORDS: usize = 4;

/// The fewest words in a paragraph above the headline that make it the
/// article's own.
///
/// A standfirst or an opening sentence placed above the title is prose. What
/// else sits above a headline on a page is a label, a date, a button or a line
/// of category links, all of them shorter than this.
const PROSE_WORDS: usize = 12;

/// The most marked regions weighed against each other.
///
/// An index page can carry a hundred teaser `<article>`s. Weighing each is a
/// walk over the page's tags, and the first few dozen are where the story is
/// on any page that has one.
const MAX_CANDIDATES: usize = 48;

/// What a tag's attributes say about it, apart from its name.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Plain,
    /// Marked as the page's main content by its role or its microdata.
    Main,
    /// Marked as furniture by its role or by being hidden.
    Furniture,
}

struct Tag {
    start: usize,
    end: usize,
    name: String,
    closing: bool,
    self_closing: bool,
    mark: Mark,
}

/// The article in `html`, with the page around it and the furniture inside it
/// taken away.
///
/// The page's own style sheets are carried along ahead of the cut, because
/// the reader honours the parts of them that hide things, and a cut that lost
/// them would show every element the page had meant to keep out of sight.
#[must_use]
pub fn main_content(html: &str) -> String {
    let (tags, styles) = scan(html);
    let (from, to, whole) = match best_region(&tags) {
        Some((first, last)) => (first, last, false),
        None => (0, tags.len(), true),
    };
    let end = if whole {
        html.len()
    } else {
        tags.get(to).map_or(html.len(), |tag| tag.end)
    };
    let last = if whole {
        tags.len()
    } else {
        to.saturating_add(1).min(tags.len())
    };
    // The headline, when what sits above it is not prose, is where the
    // article starts. A page's own title bar, a status label and a row of
    // category links all live there, inside the region the page marked.
    let headline = headline(html, &tags, from, last);
    let from = headline.unwrap_or(from);
    let start = match headline {
        Some(index) => tags[index].start,
        None if whole => 0,
        None => tags[from].start,
    };
    let mut out = String::with_capacity(end - start + 256);
    for (style_start, style_end) in &styles {
        if *style_end <= start || *style_start >= end {
            out.push_str(&html[*style_start..*style_end]);
        }
    }
    let mut cursor = start;
    let mut index = from;
    while index < last {
        let tag = &tags[index];
        if tag.start < cursor {
            index += 1;
            continue;
        }
        let strip = !tag.closing
            && (tag.mark == Mark::Furniture
                || FURNITURE.contains(&tag.name.as_str())
                || (whole && tag.name == "header")
                || is_menu(html, &tags, index));
        if strip {
            out.push_str(&html[cursor..tag.start]);
            let close = if tag.self_closing {
                index
            } else {
                closing_of(&tags, index).unwrap_or(last.saturating_sub(1))
            };
            cursor = tags.get(close).map_or(end, |tag| tag.end).min(end);
            index = close + 1;
            continue;
        }
        if !tag.closing && tag.name == "img" {
            if let Some(rewritten) = lazy_image(&html[tag.start..tag.end]) {
                out.push_str(&html[cursor..tag.start]);
                out.push_str(&rewritten);
                cursor = tag.end;
            }
        }
        index += 1;
    }
    if cursor < end {
        out.push_str(&html[cursor..end]);
    }
    out
}

/// The address of every picture the markup shows, in the order it shows them,
/// spelled the way the reader will ask for them.
///
/// A scan of the tags rather than a parse of the document, because this is
/// asked when an article is being put away for later and the parse is paid
/// again when it is read. The reader takes the `src` of an `<img>` with its
/// entities decoded, and so does this.
#[must_use]
pub fn picture_sources(html: &str) -> Vec<String> {
    let (tags, _) = scan(html);
    tags.iter()
        .filter(|tag| !tag.closing && tag.name == "img")
        .filter_map(|tag| {
            let inside = &html[tag.start + 1..tag.end - 1];
            attribute(inside, "src")
                .filter(|source| !source.is_empty())
                .map(crate::decode_entities)
        })
        .collect()
}

/// Every tag in the page, and where its style sheets are.
///
/// The insides of scripts and style sheets are stepped over rather than read
/// as markup: a script that builds `"<article>"` in a string is not an
/// article, and a scanner that thought so would cut the page at the wrong
/// place.
fn scan(html: &str) -> (Vec<Tag>, Vec<(usize, usize)>) {
    let mut tags = Vec::new();
    let mut styles = Vec::new();
    let mut at = 0;
    while let Some(offset) = html[at..].find('<') {
        let start = at + offset;
        let tail = &html[start..];
        if let Some(rest) = tail.strip_prefix("<!--") {
            at = rest
                .find("-->")
                .map_or(html.len(), |close| start + 4 + close + 3);
            continue;
        }
        if let Some(rest) = tail.strip_prefix("<![CDATA[") {
            at = rest
                .find("]]>")
                .map_or(html.len(), |close| start + 9 + close + 3);
            continue;
        }
        let Some(close) = tail.find('>') else {
            break;
        };
        let inside = &tail[1..close];
        let end = start + close + 1;
        let name = element_name(inside);
        let closing = inside.starts_with('/');
        if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic()) {
            at = end;
            continue;
        }
        if !closing && (name == "script" || name == "style") {
            let after = skip_element(&html[end..], &name);
            let after_at = html.len() - after.len();
            if name == "style" {
                styles.push((start, after_at));
            }
            at = after_at;
            continue;
        }
        let role = attribute(inside, "role").map(str::to_ascii_lowercase);
        let main = role.as_deref() == Some("main")
            || attribute(inside, "itemprop").is_some_and(|prop| prop == "articleBody");
        let furniture = role
            .as_deref()
            .is_some_and(|role| FURNITURE_ROLES.contains(&role))
            || attribute(inside, "aria-hidden").is_some_and(|value| value == "true")
            || ["class", "id"].iter().any(|name| {
                attribute(inside, name).is_some_and(|value| {
                    let value = value.to_ascii_lowercase();
                    FURNITURE_NAMES.iter().any(|word| value.contains(word))
                })
            })
            || has_bare_attribute(inside, "hidden")
            || attribute(inside, "type").is_some_and(|kind| kind.eq_ignore_ascii_case("button"))
            || attribute(inside, "class").is_some_and(|value| {
                let value = value.to_ascii_lowercase().replace(['-', '_'], "");
                UNSEEN_NAMES.iter().any(|word| value.contains(word))
            });
        tags.push(Tag {
            start,
            end,
            self_closing: inside.ends_with('/'),
            name,
            closing,
            mark: if furniture {
                Mark::Furniture
            } else if main {
                Mark::Main
            } else {
                Mark::Plain
            },
        });
        at = end;
    }
    (tags, styles)
}

/// The first top-level heading between `from` and `last`, when nothing above
/// it is prose.
///
/// Only an `<h1>`, because a page has one title and many section headings,
/// and starting at a section heading would lose the story's opening.
fn headline(html: &str, tags: &[Tag], from: usize, last: usize) -> Option<usize> {
    let title = (from..last).find(|&index| !tags[index].closing && tags[index].name == "h1")?;
    // Nothing to read above it means nothing to take away, and the cut keeps
    // the region's own opening tag.
    if words_between(html, tags, from, title) == 0 && !has_picture(tags, from, title) {
        return None;
    }
    let prose_above = (from..title)
        .filter(|&index| !tags[index].closing && tags[index].name == "p")
        .any(|open| {
            let close = closing_of(tags, open).map_or(title, |close| close.min(title));
            words_between(html, tags, open, close) >= PROSE_WORDS
        });
    (!prose_above).then_some(title)
}

/// Whether the element opening at `open` is a menu: at least [`MENU_LINKS`]
/// short links and nothing else to read.
///
/// Pages that do not mark their menus with `<nav>` still write them the same
/// way, as a list or a row of links of a word or two each. Read in order they
/// are a column of "Archive", "Search", "About" above the article, or a row of
/// tags with no spaces between them. A word or two ahead of the first link is
/// the row's own label ("Labels", "Tags") and goes with it. A paragraph is
/// never a menu, because a sentence may well be mostly links.
fn is_menu(html: &str, tags: &[Tag], open: usize) -> bool {
    let tag = &tags[open];
    if tag.closing
        || tag.self_closing
        || !matches!(tag.name.as_str(), "ul" | "ol" | "div" | "span" | "section")
    {
        return false;
    }
    let name = tag.name.as_str();
    let mut depth = 1_usize;
    let mut links = 0;
    let mut label_words = 0;
    let mut index = open;
    loop {
        let tag = &tags[index];
        if index > open && tag.name == name && !tag.self_closing {
            if tag.closing {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            } else {
                depth += 1;
            }
        }
        if !tag.closing && tag.name == "img" {
            // A row of linked pictures is a gallery, not a menu.
            return false;
        }
        if !tag.closing && tag.mark == Mark::Furniture && !tag.self_closing {
            // A tooltip or a hidden label is taken out on its own, so its
            // words do not count against the row it sits in.
            let Some(close) = closing_of(tags, index) else {
                return false;
            };
            index = close + 1;
            if index >= tags.len() {
                break;
            }
            continue;
        }
        if !tag.closing && tag.name == "a" {
            let Some(close) = closing_of(tags, index) else {
                return false;
            };
            let words = words_between(html, tags, index, close);
            if words == 0 || words > MENU_LINK_WORDS || has_picture(tags, index, close) {
                return false;
            }
            links += 1;
            index = close;
        } else {
            let text_end = tags.get(index + 1).map_or(html.len(), |next| next.start);
            let text = &html[tag.end..text_end.max(tag.end)];
            if text.chars().any(char::is_alphanumeric) {
                // Text outside every link is something to read, unless it is
                // a short label ahead of the first one.
                label_words += text.split_whitespace().count();
                if links > 0 || label_words > 2 {
                    return false;
                }
            }
        }
        index += 1;
        if index >= tags.len() {
            break;
        }
    }
    links >= MENU_LINKS
}

/// Whether a picture sits between two tags.
fn has_picture(tags: &[Tag], from: usize, to: usize) -> bool {
    tags[from..to.min(tags.len())]
        .iter()
        .any(|tag| !tag.closing && tag.name == "img")
}

/// The number of words of text between two tags that a reader would see.
///
/// The words of a hidden label inside a link do not make it a long link.
fn words_between(html: &str, tags: &[Tag], from: usize, to: usize) -> usize {
    let to = to.min(tags.len());
    let mut words = 0;
    let mut index = from;
    while index < to {
        let tag = &tags[index];
        if index > from && !tag.closing && !tag.self_closing && tag.mark == Mark::Furniture {
            index = closing_of(tags, index).map_or(to, |close| close.max(index + 1));
            continue;
        }
        let text_end = tags.get(index + 1).map_or(html.len(), |next| next.start);
        words += html[tag.end..text_end.max(tag.end)]
            .split_whitespace()
            .count();
        index += 1;
    }
    words
}

/// Whether a tag carries an attribute written with no value, such as
/// `hidden`.
fn has_bare_attribute(inside: &str, wanted: &str) -> bool {
    inside
        .split(|c: char| c.is_whitespace() || c == '/')
        .skip(1)
        .any(|word| word.eq_ignore_ascii_case(wanted))
}

/// The tag that closes the one at `open`, counting nested ones of its name.
fn closing_of(tags: &[Tag], open: usize) -> Option<usize> {
    let name = tags[open].name.as_str();
    let mut depth = 0_usize;
    for (index, tag) in tags.iter().enumerate().skip(open) {
        if tag.name != name || tag.self_closing {
            continue;
        }
        if tag.closing {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(index);
            }
        } else {
            depth += 1;
        }
    }
    None
}

/// The first and last tag of the marked region holding the most paragraphs.
///
/// Paragraphs are counted rather than characters because counting them is a
/// tag comparison, and the regions that win on characters and lose on
/// paragraphs are a page's comments and its legal small print.
///
/// When one region sits inside another and holds nearly all of its
/// paragraphs, the inner one is the article and the outer one is the article
/// plus whatever the page hung around it, so the inner one wins.
fn best_region(tags: &[Tag]) -> Option<(usize, usize)> {
    let mut paragraphs = Vec::with_capacity(tags.len() + 1);
    let mut count = 0_usize;
    paragraphs.push(0);
    for tag in tags {
        if !tag.closing && tag.name == "p" {
            count += 1;
        }
        paragraphs.push(count);
    }
    let candidates: Vec<(usize, usize, usize)> = tags
        .iter()
        .enumerate()
        .filter(|(_, tag)| {
            !tag.closing && (tag.name == "article" || tag.name == "main" || tag.mark == Mark::Main)
        })
        .take(MAX_CANDIDATES)
        .map(|(open, _)| {
            // A region whose closing tag never arrived runs to the end of
            // what did, which is past the last tag.
            let close = closing_of(tags, open).unwrap_or(tags.len());
            (
                open,
                close,
                paragraphs[(close + 1).min(tags.len())] - paragraphs[open],
            )
        })
        .collect();
    let mut best = *candidates.iter().max_by(|a, b| {
        a.2.cmp(&b.2)
            // Equal counts favour the earlier region, which is the story
            // rather than the teaser for the next one.
            .then(b.0.cmp(&a.0))
    })?;
    if best.2 == 0 {
        return None;
    }
    loop {
        let inner = candidates
            .iter()
            .filter(|(open, close, score)| {
                *open > best.0 && *close <= best.1 && score * 4 >= best.2 * 3
            })
            .min_by_key(|(open, close, _)| close - open)
            .copied();
        match inner {
            Some(inner) => best = inner,
            None => return Some((best.0, best.1)),
        }
    }
}

/// An image that names its picture only in a lazy-loading attribute, written
/// so the reader can find it.
///
/// Pages that load pictures as they scroll into view put a blank placeholder
/// in `src` and the real address in `data-src`. Nothing on this device scrolls,
/// so without this every such picture was the placeholder.
fn lazy_image(tag: &str) -> Option<String> {
    let inside = tag.strip_prefix('<')?.strip_suffix('>')?;
    let source = attribute(inside, "src").unwrap_or_default();
    if !source.is_empty() && !source.starts_with("data:") {
        return None;
    }
    let real = ["data-src", "data-lazy-src", "data-original"]
        .iter()
        .find_map(|name| attribute(inside, name))
        .filter(|real| !real.is_empty() && !real.starts_with("data:"))?;
    let alt = attribute(inside, "alt").unwrap_or_default();
    Some(format!(
        "<img src=\"{}\" alt=\"{}\">",
        real.replace('"', "&quot;"),
        alt.replace('"', "&quot;")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_article_is_kept_and_the_menus_and_footer_around_it_are_not() {
        let page = "<html><body><nav><a href=/>Home</a><a href=/news>News</a></nav>\
            <header><h1>Site name</h1></header>\
            <article><h1>The story</h1><p>First.</p><p>Second.</p></article>\
            <footer><p>Copyright</p><p>Privacy</p><p>Terms</p></footer></body></html>";
        let cut = main_content(page);
        assert!(cut.contains("The story"));
        assert!(cut.contains("Second."));
        assert!(!cut.contains("Home"));
        assert!(!cut.contains("Site name"));
        assert!(!cut.contains("Copyright"));
    }

    #[test]
    fn share_buttons_and_forms_inside_the_article_are_taken_out() {
        let page = "<article><p>Words.</p><button>Share</button>\
            <form><label>Email</label><input></form><aside><p>Related</p></aside>\
            <p>More words.</p></article>";
        let cut = main_content(page);
        assert!(cut.contains("Words."));
        assert!(cut.contains("More words."));
        assert!(!cut.contains("Share"));
        assert!(!cut.contains("Email"));
        assert!(!cut.contains("Related"));
    }

    #[test]
    fn the_region_with_the_story_beats_a_teaser_for_the_next_one() {
        let page = "<article><p>Teaser.</p></article>\
            <main><article><h1>Story</h1><p>One.</p><p>Two.</p><p>Three.</p></article>\
            <section><p>A comment.</p></section></main>";
        let cut = main_content(page);
        assert!(cut.starts_with("<article><h1>Story"));
        assert!(!cut.contains("Teaser."));
        assert!(!cut.contains("A comment."));
    }

    #[test]
    fn a_page_that_marks_nothing_is_kept_whole_without_its_furniture() {
        let page = "<body><header>Masthead</header><div><p>Only words.</p></div>\
            <div role=\"navigation\">Links</div><footer>Legal</footer></body>";
        let cut = main_content(page);
        assert!(cut.contains("Only words."));
        assert!(!cut.contains("Masthead"));
        assert!(!cut.contains("Links"));
        assert!(!cut.contains("Legal"));
    }

    #[test]
    fn a_main_region_marked_only_by_its_role_is_found() {
        let page = "<div class=menu><p>Menu</p></div>\
            <div role=\"main\"><div><p>Inside.</p></div><p>Also inside.</p></div>\
            <div><p>After.</p></div>";
        let cut = main_content(page);
        assert!(cut.contains("Inside."));
        assert!(cut.contains("Also inside."));
        assert!(!cut.contains("After."));
        assert!(!cut.contains("Menu"));
    }

    #[test]
    fn markup_written_inside_a_script_is_not_mistaken_for_the_article() {
        let page = "<script>var a = '<article><p>Fake</p></article>';</script>\
            <article><p>Real.</p></article>";
        let cut = main_content(page);
        assert!(cut.contains("Real."));
        assert!(!cut.contains("Fake"));
    }

    #[test]
    fn a_page_cut_off_before_its_closing_tag_keeps_what_arrived() {
        let cut = main_content("<nav>Menu</nav><article><p>Half of a sto");
        assert!(cut.contains("Half of a sto"));
        assert!(!cut.contains("Menu"));
    }

    #[test]
    fn the_page_style_sheets_travel_with_the_cut() {
        let page = "<head><style>.hide{display:none}</style></head>\
            <body><article><p class=hide>Hidden.</p><p>Shown.</p></article></body>";
        let cut = main_content(page);
        assert!(cut.starts_with("<style>.hide{display:none}</style><article>"));
    }

    #[test]
    fn a_lazily_loaded_picture_is_given_its_real_address() {
        let page = "<article><p>Look.</p>\
            <img src=\"data:image/gif;base64,R0l\" data-src=\"/a.png\" alt=\"A chart\"></article>";
        let cut = main_content(page);
        assert!(cut.contains("<img src=\"/a.png\" alt=\"A chart\">"));
    }

    #[test]
    fn pictures_are_named_the_way_the_reader_will_ask_for_them() {
        let html = "<p>A</p><img src=\"a.png?w=1&amp;h=2\"><IMG SRC='b.png'><img alt=x>";
        assert_eq!(picture_sources(html), vec!["a.png?w=1&h=2", "b.png"]);
    }

    #[test]
    fn a_trail_of_breadcrumbs_above_the_headline_is_left_out() {
        let page = "<main><div class=\"Breadcrumbs-module__bar\"><ul><li>Docs</li>\
            <li>Release Notes</li></ul></div><h1>Version 2</h1><p>What changed.</p></main>";
        let cut = main_content(page);
        assert!(cut.contains("Version 2"));
        assert!(!cut.contains("Release Notes"));
    }

    #[test]
    fn hidden_elements_are_left_out() {
        let page = "<article><p>Seen.</p><div hidden><p>Unseen.</p></div>\
            <span aria-hidden=\"true\">Icon</span></article>";
        let cut = main_content(page);
        assert!(cut.contains("Seen."));
        assert!(!cut.contains("Unseen."));
        assert!(!cut.contains("Icon"));
    }
    #[test]
    fn a_code_host_issue_page_opens_at_its_title_without_its_buttons_or_hidden_labels() {
        // Cut down from an issue page on a code host as it is served to a
        // reader that runs no scripts: a visually hidden heading and a
        // repository menu above the title, a "New issue" link dressed as a
        // button beside it, a row of label links with tooltips between them,
        // and a sticky copy of the title for when the page scrolls.
        let page = "<html><body><main>\
            <h2 class=\"prc-src-InternalVisuallyHidden-2YaI6\">Repository navigation</h2>\
            <ul><li><a href=/code>Code</a></li><li><a href=/issues>Issues</a></li>\
            <li><a href=/pulls>Pull requests</a></li></ul>\
            <div role=region><h1><bdi>Privacy violation in User-Agent header</bdi>\
            <span> #107757</span></h1>\
            <a data-component=Button type=button href=/login><span>New issue</span></a></div>\
            <div class=\"issueMetadata\"><div>Labels</div><div>\
            <a href=/l/1><span>area:security</span></a>\
            <span role=tooltip aria-hidden=true>Security issues</span>\
            <a href=/l/2><span>bug</span><span class=sr-only>Something isn't working</span></a>\
            <a href=/l/3><span>platform:macos</span></a></div></div>\
            <div id=\"issue-viewer-sticky-header\"><a href=#top>Privacy violation in User-Agent \
            header</a><span>#107757</span></div>\
            <h2 class=sr-only>Description</h2>\
            <p>I receive dozens of scam emails a day since my address went out in a header.</p>\
            <p>Steps to reproduce follow.</p></main></body></html>";
        let cut = main_content(page);
        assert!(cut.starts_with("<h1>"), "{cut}");
        assert!(cut.contains("dozens of scam emails"));
        for chrome in [
            "Repository navigation",
            "Pull requests",
            "New issue",
            "area:security",
            "Labels",
            "Something isn't working",
            "Description",
            "#top",
        ] {
            assert!(!cut.contains(chrome), "{chrome} was kept in {cut}");
        }
    }

    #[test]
    fn a_menu_written_as_a_plain_list_of_short_links_is_left_out() {
        // A feed that carries a whole page: its section menu is a list of
        // one-word links with no `<nav>` around it.
        let page = "<div class=\"secondary-menu\"><ul>\
            <li><a href=/today>Today\u{2019}s picture</a></li><li><a href=/archive>Archive</a></li>\
            <li><a href=/submit>Submissions</a></li><li><a href=/search>Search</a></li></ul></div>\
            <h2>A farewell to a telescope</h2><p>After twenty-two years of service it came home.</p>";
        let cut = main_content(page);
        assert!(cut.contains("A farewell to a telescope"));
        assert!(cut.contains("twenty-two years"));
        assert!(!cut.contains("Archive"));
        assert!(!cut.contains("Submissions"));
    }

    #[test]
    fn a_reading_list_of_long_link_titles_and_a_sentence_full_of_links_are_kept() {
        let page = "<article><p>The fleet, <a href=/a>the tugs</a>, <a href=/b>the ferries</a> and \
            <a href=/c>the dredgers</a> all came out.</p><h2>Further reading</h2><ul>\
            <li><a href=/1>How the tide tables were first printed in the port</a></li>\
            <li><a href=/2>A history of the harbour master\u{2019}s office and its keepers</a></li>\
            <li><a href=/3>The pilot boats of the estuary from sail to diesel</a></li></ul></article>";
        let cut = main_content(page);
        assert!(cut.contains("the dredgers"));
        assert!(cut.contains("tide tables"));
        assert!(cut.contains("pilot boats"));
    }

    #[test]
    fn a_row_of_linked_pictures_is_a_gallery_and_is_kept() {
        let page = "<article><p>Photographs from the walk.</p><div>\
            <a href=/1><img src=1.jpg alt=Pier></a><a href=/2><img src=2.jpg alt=Dunes></a>\
            <a href=/3><img src=3.jpg alt=Lighthouse></a></div></article>";
        let cut = main_content(page);
        assert!(cut.contains("1.jpg"));
        assert!(cut.contains("3.jpg"));
    }

    #[test]
    fn prose_above_the_headline_keeps_the_article_from_starting_at_it() {
        let page = "<main><p>This is the opening of a column that puts its first paragraph \
            above its own title, as some do.</p><h1>The title</h1><p>The rest.</p></main>";
        let cut = main_content(page);
        assert!(cut.contains("opening of a column"));
        assert!(cut.contains("The title"));
    }

    #[test]
    fn a_date_and_a_line_of_category_links_above_the_headline_are_left_out() {
        let page = "<article><p>5 November \u{b7} <a href=/t/mac>macos</a> \
            <a href=/t/diy>diy</a></p><h1>Reborn</h1><p>It boots.</p></article>";
        let cut = main_content(page);
        assert!(cut.starts_with("<h1>Reborn"), "{cut}");
    }
}
