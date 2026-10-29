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
const FURNITURE_ROLES: [&str; 6] = [
    "navigation",
    "complementary",
    "contentinfo",
    "banner",
    "search",
    "dialog",
];

/// Words in a class or id that only ever name furniture.
///
/// Deliberately few. A page's class names are its own business and most say
/// nothing about meaning, but these three are spelled the same way on almost
/// every site that has them, and each one, left in, put a line of links or a
/// sign-up form at the top of the first page. "cookie" is not among them,
/// because a recipe is entitled to a class called `cookie-recipe`.
const FURNITURE_NAMES: [&str; 3] = ["breadcrumb", "newsletter", "skip-link"];

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
    let start = if whole { 0 } else { tags[from].start };
    let end = if whole {
        html.len()
    } else {
        tags.get(to).map_or(html.len(), |tag| tag.end)
    };
    let mut out = String::with_capacity(end - start + 256);
    for (style_start, style_end) in &styles {
        if *style_end <= start || *style_start >= end {
            out.push_str(&html[*style_start..*style_end]);
        }
    }
    let mut cursor = start;
    let mut index = from;
    let last = if whole {
        tags.len()
    } else {
        to.saturating_add(1).min(tags.len())
    };
    while index < last {
        let tag = &tags[index];
        if tag.start < cursor {
            index += 1;
            continue;
        }
        let strip = !tag.closing
            && (tag.mark == Mark::Furniture
                || FURNITURE.contains(&tag.name.as_str())
                || (whole && tag.name == "header"));
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
            || has_bare_attribute(inside, "hidden");
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
}
